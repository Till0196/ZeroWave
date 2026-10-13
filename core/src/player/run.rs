// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 再生のスレッドの中身：フェッチ（`fill`）、読み取り（`read` と `drain`）、デコード（`decode`）。

use super::names::{describe, picture_names, say_pictures, say_sounds, sound_choices};
use super::*;

/// What the flags asked for, handed over since it can only be acted on once the map arrives.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Wanted {
    pub(super) audio: Option<usize>,
    pub(super) video: Option<usize>,
}

/// エンジンの番組表に残す、現在より前の時間（秒）。
const GUIDE_KEPT_BEHIND: crate::guide::Moment = 3 * 3600;

#[allow(clippy::too_many_arguments)]
pub(super) fn read(
    chain: Option<zwsdk::host::Chain>,
    file: &mut crate::input::buffer::Reading,
    engine: &mut Engine,
    access: &SyncSender<Out>,
    rest: &SyncSender<Out>,
    dropped: &AtomicU64,
    newest: &AtomicU64,
    oldest: &AtomicU64,
    queued: &AtomicI64,
    newest_sound: &AtomicU64,
    skip_to: &AtomicI64,
    stop: &AtomicBool,
    shared: &Shared,
    live: bool,
    stamped: Stamped,
    spent: &Reading,
    asks: &Receiver<Ask>,
    told: &SyncSender<Tables>,
    guided: &SyncSender<Guide>,
    wanted: Wanted,
    data: Option<&Feed>,
) {
    let Wanted {
        audio: mut wanted,
        video: mut wanted_picture,
    } = wanted;
    let mut chunk = vec![0; CHUNK];
    let mut said: Vec<u16> = Vec::new();
    let mut said_pictures: Vec<u16> = Vec::new();
    let mut told_sound = crate::sound::sounding::NO_SOUND;
    let mut resuming = Resuming::default();
    let mut held = Held::default();
    let mut missed = 0;
    let mut by_stamp = ByStamp::default();
    let mut chain = chain.unwrap_or_else(|| zwsdk::host::Chain::new(Vec::new()));
    if !chain.is_empty() {
        let container = match engine.container() {
            crate::filters::Container::Ts => zwsdk::host::Container::Ts,
            crate::filters::Container::Tlv => zwsdk::host::Container::Tlv,
        };
        for refused in chain.start(container, engine.wanted()) {
            tracing::info!("プラグイン: {refused}");
        }
    }
    let mut failed = Vec::new();
    let mut sent_guide: Option<u64> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        held.release(shared, data);
        for ask in asks.try_iter() {
            match ask {
                Ask::NextPicture => {
                    if let Some(next) = engine.next_picture() {
                        let stand_in = if engine.picture_is_stand_in() {
                            "（降雨対応）"
                        } else {
                            ""
                        };
                        tracing::info!("映像を 0x{:04x}{stand_in} に切り替えた", next.id);
                    }
                }
                Ask::Sound(at) => {
                    if let Some(stream) = engine.sounds().get(at)
                        && engine.play_sound(stream.id)
                    {
                        tracing::info!("音声を {} に切り替えた", describe(stream));
                    }
                }
                Ask::Picture(at) => {
                    if let Some(stream) = engine.pictures().get(at)
                        && engine.play_picture(stream.id)
                    {
                        tracing::info!("映像を 0x{:04x} に切り替えた", stream.id);
                    }
                }
                Ask::DualMono(mode) => {
                    engine.set_dual_mono(mode);
                    tracing::info!("デュアルモノを {} に切り替えた", mode.name());
                }
                // 容量一つのチャネルなので、詰まっていたら送らない。
                Ask::Tables => {
                    let _ = told.try_send(gather(engine));
                }
                Ask::Guide => {
                    let guide = engine.guide_mut();
                    if let Some(now) = guide.now() {
                        guide.forget_before(now - GUIDE_KEPT_BEHIND);
                    }
                    let revision = engine.guide().revision();
                    if sent_guide != Some(revision)
                        && guided.try_send(engine.guide().clone()).is_ok()
                    {
                        sent_guide = Some(revision);
                    }
                }
            }
        }
        // Asked for again rather than kept: map updates add and remove sound streams.
        let now: Vec<u16> = engine.sounds().iter().map(|stream| stream.id).collect();
        if now != said {
            said = now;
            if let Some(at) = wanted
                && let Some(stream) = engine.sounds().get(at - 1)
            {
                let id = stream.id;
                if engine.play_sound(id) {
                    wanted = None;
                }
            }
            say_sounds(engine);
        }
        tell_chosen(engine, shared, &mut told_sound);
        let now: Vec<u16> = engine.pictures().iter().map(|stream| stream.id).collect();
        if now != said_pictures {
            said_pictures = now;
            if let Some(at) = wanted_picture
                && let Some(stream) = engine.pictures().get(at - 1)
            {
                let id = stream.id;
                if engine.play_picture(id) {
                    wanted_picture = None;
                }
            }
            say_pictures(engine);
        }
        if !live {
            pace(shared, newest, oldest, spent, &stamped, &mut by_stamp);
        }
        let began = Instant::now();
        let read = match file.read(&mut chunk) {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!("ソースを読み込めない: {error}");
                0
            }
        };
        let came = Instant::now();
        if !live && read > 0 {
            stamped.note(engine.container(), &chunk[..read]);
        }
        let taken = chain.run(&chunk[..read], &mut failed);
        crate::record::feed(taken);
        for why in failed.drain(..) {
            tracing::warn!("プラグインを外した: {why}");
        }
        let got = Instant::now();
        spent
            .source
            .fetch_add((got - began).as_micros() as u64, Ordering::Relaxed);
        spent.longest[0].fetch_max((came - began).as_micros() as u64, Ordering::Relaxed);
        spent.longest[1].fetch_max((got - came).as_micros() as u64, Ordering::Relaxed);
        if file.missed() != missed {
            tracing::info!(
                "リングバッファがあふれ、{} ブロック欠落した",
                file.missed() - missed
            );
            missed = file.missed();
            spent.missed.store(missed, Ordering::Relaxed);
        }
        if read == 0 {
            drain(
                engine,
                access,
                rest,
                dropped,
                newest,
                oldest,
                queued,
                newest_sound,
                skip_to,
                &mut resuming,
                &mut held,
                data,
                &shared.unreadable,
                &shared.viewer_off,
            );
            return;
        }
        engine.push(taken);
        // チャネルへ流す前に行う（でないと最初の run は音声のスレッドが捨てる）。
        tell_chosen(engine, shared, &mut told_sound);
        let apart = Instant::now();
        spent
            .apart
            .fetch_add((apart - got).as_micros() as u64, Ordering::Relaxed);
        spent.longest[2].fetch_max((apart - got).as_micros() as u64, Ordering::Relaxed);
        let carried = !drain(
            engine,
            access,
            rest,
            dropped,
            newest,
            oldest,
            queued,
            newest_sound,
            skip_to,
            &mut resuming,
            &mut held,
            data,
            &shared.unreadable,
            &shared.viewer_off,
        );
        held.release(shared, data);
        let onward = apart.elapsed().as_micros() as u64;
        spent.onward.fetch_add(onward, Ordering::Relaxed);
        spent.longest[3].fetch_max(onward, Ordering::Relaxed);
        if carried {
            return;
        }
    }
}

pub(super) fn tell_chosen(engine: &Engine, shared: &Shared, told: &mut u32) {
    let chosen = engine
        .chosen()
        .1
        .map_or(crate::sound::sounding::NO_SOUND, u32::from);
    if chosen != *told {
        shared.sound.store(chosen, Ordering::Relaxed);
        *told = chosen;
    }
}

/// 録画のとき、読み込みを表示中の時刻に合わせて待つ。時計を先に見るのは、
/// デコードが間に合わない環境でも音声を止めないため。
pub(super) fn pace(
    shared: &Shared,
    newest: &AtomicU64,
    oldest: &AtomicU64,
    spent: &Reading,
    stamped: &Stamped,
    by_stamp: &mut ByStamp,
) {
    let began = Instant::now();
    while began.elapsed() < PACE_MOST {
        let now = shared.with_clock(|clock| clock.now());
        let arrived = newest.load(Ordering::Relaxed);
        // 何もデコードできていなければ、伝送路の時刻と実時間でペースを合わせる。
        let far = if arrived == 0 && now.is_none() {
            by_stamp.too_far(stamped, shared)
        } else {
            too_far(arrived, now, oldest.load(Ordering::Relaxed))
        };
        if !far {
            break;
        }
        std::thread::sleep(PACE_NAP);
    }
    spent
        .pacing
        .fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
}

/// 読み取りスレッドが見た伝送路の時刻（PCR / NTP）。何もデコードできない録画でもペースを合わせられる。
#[derive(Debug, Clone)]
pub(super) struct Stamped {
    pub latest: Arc<AtomicI64>,
    pub first: Arc<AtomicI64>,
}

impl Stamped {
    pub fn note(&self, container: crate::filters::Container, bytes: &[u8]) {
        let Some(last) = crate::input::span::stamps(container, bytes)
            .last()
            .map(|s| s.1)
        else {
            return;
        };
        if self.first.load(Ordering::Relaxed) == i64::MIN {
            self.first.store(last, Ordering::Relaxed);
        }
        self.latest.store(last, Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
pub(super) struct ByStamp {
    origin: Option<(i64, Instant)>,
}

impl ByStamp {
    fn too_far(&mut self, stamped: &Stamped, shared: &Shared) -> bool {
        let latest = stamped.latest.load(Ordering::Relaxed);
        if latest == i64::MIN {
            return false;
        }
        if shared.with_clock(|clock| clock.is_paused()) {
            return true;
        }
        let (origin, since) = *self.origin.get_or_insert((latest, Instant::now()));
        let allowed = origin + (since.elapsed().as_secs_f64() * 90_000.0) as i64;
        latest - allowed > AHEAD_MOST
    }
}

pub(super) fn too_far(newest: u64, now: Option<i64>, oldest: u64) -> bool {
    if newest == 0 {
        return false;
    }
    let Some(now) = now.or(match oldest {
        0 => None,
        oldest => Some(oldest as i64),
    }) else {
        return false;
    };
    newest as i64 - now > AHEAD_MOST
}

/// ソースから読んでリングバッファへ書き込むだけ。下流を待たない（待つ間に来た分をソースが捨てる）。
pub(super) fn fill(
    bytes: &mut dyn Read,
    buffer: &crate::input::buffer::Buffer,
    counted: &AtomicU64,
    spent: &Filling,
    stop: &AtomicBool,
) {
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let began = Instant::now();
        let read = match bytes.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if crate::input::source::NotYet::is(&error) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            Err(error) => {
                tracing::warn!("ソースを読み込めない: {error}");
                0
            }
        };
        let got = Instant::now();
        spent
            .waiting
            .fetch_add((got - began).as_micros() as u64, Ordering::Relaxed);
        if read == 0 {
            break;
        }
        counted.fetch_add(read as u64, Ordering::Relaxed);
        let pushed = buffer.push(&chunk[..read]);
        spent
            .pushing
            .fetch_add(got.elapsed().as_micros() as u64, Ordering::Relaxed);
        spent.blocks.fetch_add(1, Ordering::Relaxed);
        if !pushed {
            return;
        }
    }
    buffer.end();
}

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    decoding: &mut Decoding,
    access: &Receiver<Out>,
    pictures: &SyncSender<Out>,
    skip_to: &AtomicI64,
    video_first: &AtomicI64,
    video_decoded: &AtomicU64,
    video_expected: &AtomicBool,
    dropped: &AtomicU64,
    oldest: &AtomicU64,
    queued: &AtomicI64,
    spent: &Decodes,
) {
    let mut outs = VecDeque::new();
    // 古い符号化データを全部デコードしてから捨てると負荷が高すぎるので、ここでも読み飛ばす。
    let mut resuming = Resuming::default();
    let mut coded = None;
    let mut generation = 0;
    loop {
        let began = Instant::now();
        let got = match access.recv() {
            Ok(out) => out,
            Err(_) => {
                decoding.flush(&mut outs);
                for out in outs.drain(..) {
                    let _ = pictures.send(out);
                }
                return;
            }
        };
        let waited = Instant::now();
        spent
            .waiting
            .fetch_add((waited - began).as_micros() as u64, Ordering::Relaxed);
        if matches!(got, Out::Access(_)) {
            queued.fetch_sub(1, Ordering::Relaxed);
            video_expected.store(true, Ordering::Relaxed);
        }
        // チャネル内のいちばん古い時刻はここで決まる。捨てる符号化データでも記録する。
        if let Out::Access(unit) = &got
            && let Some(at) = unit.at
        {
            oldest.store(at as u64, Ordering::Relaxed);
        }
        if let Out::Access(unit) = &got
            && resuming.leave(unit, skip_to.load(Ordering::Relaxed))
        {
            dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if matches!(got, Out::Switched) {
            video_first.store(i64::MIN, Ordering::Relaxed);
            video_decoded.store(0, Ordering::Relaxed);
            video_expected.store(false, Ordering::Relaxed);
            resuming = Resuming::default();
        }
        if let Out::Access(unit) = &got {
            let at = match unit.picture.coding {
                mediadecoder::es::Coding::I => Some(0),
                mediadecoder::es::Coding::P => Some(1),
                mediadecoder::es::Coding::B => Some(2),
                mediadecoder::es::Coding::Unknown => None,
            };
            if let Some(at) = at {
                spent.frames[at].fetch_add(1, Ordering::Relaxed);
            }
            if unit.picture.repeat_first_field {
                spent.repeats.fetch_add(1, Ordering::Relaxed);
            }
            if unit.codec == mediadecoder::es::Codec::Mpeg2 {
                let field = usize::from(unit.picture.structure.field());
                spent.structures[field].fetch_add(1, Ordering::Relaxed);
            }
            let now = Some((unit.codec, unit.format));
            if coded != now {
                coded = now;
                if let Ok(mut about) = spent.about.lock() {
                    about.coded = now;
                }
            }
        }
        // 壊れた符号化データでデコーダーが異常終了しても、スレッドごと止めず、次のデータで開き直す。
        let pushed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decoding.push(got, &mut outs);
        }));
        if pushed.is_err() {
            tracing::error!("デコーダーが異常終了した。開き直して続ける");
            decoding.forget();
        }
        if decoding.generation() != generation {
            generation = decoding.generation();
            if let Ok(mut about) = spent.about.lock() {
                about.decoder = decoding.decoder();
                about.deinterlaced = decoding.deinterlaces();
            }
        }
        let troubles = outs
            .iter()
            .filter(|out| matches!(out, Out::Trouble { .. }))
            .count();
        if troubles > 0 {
            spent.troubles.fetch_add(troubles as u64, Ordering::Relaxed);
        }
        let done = Instant::now();
        spent
            .decoding
            .fetch_add((done - waited).as_micros() as u64, Ordering::Relaxed);
        for out in outs.drain(..) {
            if let Out::Picture { at, .. } = &out {
                let _ = video_first.compare_exchange(
                    i64::MIN,
                    *at,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                video_decoded.fetch_add(1, Ordering::Relaxed);
            }
            if pictures.send(out).is_err() {
                return;
            }
        }
        spent
            .onward
            .fetch_add(done.elapsed().as_micros() as u64, Ordering::Relaxed);
    }
}

/// 視聴中の番組の番号。選択中の映像（無ければ音声）を含む番組で、指定された番組が含んでいればそれ。
/// 同じ映像を複数の番組が含むことがあるので、単に番号の小さい番組を選ぶことはしない。
fn watched_program(
    programs: &std::collections::BTreeMap<u16, crate::filters::Program>,
    wanted: Option<u16>,
    played: Option<u16>,
) -> Option<u16> {
    let carries =
        |program: &crate::filters::Program, pid: u16| program.streams.iter().any(|s| s.id == pid);
    played
        .and_then(|pid| {
            wanted
                .filter(|wanted| programs.get(wanted).is_some_and(|p| carries(p, pid)))
                .or_else(|| {
                    programs
                        .iter()
                        .find(|(_, program)| carries(program, pid))
                        .map(|(number, _)| *number)
                })
        })
        .or_else(|| {
            let wanted = wanted?;
            let program = programs.get(&wanted)?;
            let carries_av = program.streams.iter().any(|stream| stream.kind.played());
            (!program.streams.is_empty() && !carries_av).then_some(wanted)
        })
}

pub(super) fn gather(engine: &Engine) -> Tables {
    let guide = engine.guide();
    let si = engine.si();
    let identity = si.identity();
    let services: Vec<Service> = guide
        .services()
        .filter(|service| {
            identity.is_none_or(|identity| {
                service.network == identity.network && service.transport == identity.transport
            })
        })
        .cloned()
        .collect();
    let pids: Vec<(u16, Counted)> = engine
        .identifiers()
        .into_iter()
        .filter_map(|pid| engine.counted(pid).map(|counted| (pid, counted)))
        .collect();
    let carried: Vec<(u16, u16, String)> = engine
        .programs()
        .iter()
        .flat_map(|(number, program)| {
            program
                .streams
                .iter()
                .map(move |stream| (stream.id, *number, stream.declared.clone()))
        })
        .collect();
    let (video_id, sound_id) = engine.chosen();
    let sound_at = engine.sound_at();
    let picture_at = engine.picture_at();
    let (choices, choice_at) = sound_choices(engine);
    let pictures = picture_names(engine);
    let programs: Vec<crate::filters::Program> = engine.programs().values().cloned().collect();
    let neighbours: Vec<ServiceInfo> = si.services().cloned().collect();
    let watching = watched_program(engine.programs(), engine.wanted(), video_id.or(sound_id))
        .zip(identity)
        .map(|(service, identity)| crate::guide::Key {
            network: identity.network,
            service,
        });
    let sound = sound_id.and_then(|id| engine.sounds().into_iter().find(|stream| stream.id == id));
    let video = video_id.and_then(|id| {
        engine
            .streams()
            .into_iter()
            .find(|(stream, _)| *stream == id)
            .map(|(_, kind)| kind)
    });
    // 現在の番組は放送の時刻で探す。PC の時計だと録画で違う番組が出る。
    let now = watching.and_then(|key| guide.at(key, guide.now()?).cloned());
    let next = watching.and_then(|key| guide.after(key, guide.now()?).cloned());
    let mut emergency: Vec<isdb::nitdescriptors::EmergencyService> =
        si.emergency().cloned().collect();
    for warning in programs.iter().flat_map(|program| &program.emergency) {
        if !emergency.contains(warning) {
            emergency.push(warning.clone());
        }
    }
    Tables {
        identity,
        watching,
        sound,
        sound_at,
        dual_mono: engine.sound_is_dual_mono(),
        he_aac: sound_id.is_some_and(|id| engine.sound_is_he_aac(id)),
        sound_rate: sound_id.and_then(|id| engine.sound_rate(id)),
        choices,
        choice_at,
        picture_at,
        pictures,
        programs,
        neighbours,
        frame_header: si.frame_header().cloned(),
        video,
        now,
        next,
        networks: si.networks().cloned().collect(),
        broadcasters: si
            .broadcasters()
            .filter(|info| identity.is_none_or(|identity| info.network == identity.network))
            .cloned()
            .collect(),
        emergency,
        listed: engine.listed(),
        emm: si.emm().to_vec(),
        downloads: si.downloads().cloned().collect(),
        logos: si.logos().cloned().collect(),
        partial: si.partial().cloned(),
        packages: si.packages().to_vec(),
        layouts: si.layouts().to_vec(),
        applications: si
            .applications()
            .iter()
            .map(|(packet_id, count)| (*packet_id, *count))
            .collect(),
        counts: engine.counts(),
        container: engine.container(),
        services,
        pids,
        carried,
        clock: guide.now(),
    }
}

/// Says whether the player is still there.
#[allow(clippy::too_many_arguments)]
pub(super) fn drain(
    engine: &mut Engine,
    access: &SyncSender<Out>,
    rest: &SyncSender<Out>,
    dropped: &AtomicU64,
    newest: &AtomicU64,
    oldest: &AtomicU64,
    queued: &AtomicI64,
    newest_sound: &AtomicU64,
    skip_to: &AtomicI64,
    resuming: &mut Resuming,
    held: &mut Held,
    data: Option<&Feed>,
    unreadable: &AtomicBool,
    viewer_off: &AtomicBool,
) -> bool {
    while let Some(out) = engine.next() {
        if viewer_off.load(Ordering::Relaxed)
            && matches!(
                out,
                Out::Access(_) | Out::Sound { .. } | Out::Caption { .. }
            )
        {
            if matches!(out, Out::Access(_)) {
                resuming.skipping = true;
            }
            continue;
        }
        match &out {
            Out::Access(unit) => {
                if let Some(at) = unit.at {
                    newest.store(at as u64, Ordering::Relaxed);
                }
            }
            Out::Sound { at, .. } => newest_sound.store(*at as u64, Ordering::Relaxed),
            _ => {}
        }
        // 時刻が飛んだら（録画のドロップ）、番組が切り替わったときと同じように始め直す。
        if let Out::Sound { at, stream, .. } = &out
            && resuming.jumped(*stream, *at)
        {
            tracing::info!(
                "時刻が {:+.1} 秒飛んだため、そこから再開する",
                resuming.jump as f64 / crate::engine::clock::HZ as f64
            );
            skip_to.store(0, Ordering::Relaxed);
            newest.store(0, Ordering::Relaxed);
            oldest.store(0, Ordering::Relaxed);
            resuming.skipping = false;
            resuming.leading = false;
            let _ = rest.send(Out::Switched);
            if access.send(Out::Switched).is_err() {
                return false;
            }
        }
        match out {
            // A coded picture that will not fit is given up: blocking would stop the sound.
            // 捨てるなら次のランダムアクセスポイントまでまとめて捨てる（参照先の無い後続のピクチャーではデコーダーが壊れたフレームを返す）。
            Out::Access(unit) => {
                if resuming.leave(&unit, skip_to.load(Ordering::Relaxed)) {
                    dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let held = oldest.load(Ordering::Relaxed);
                if held != 0
                    && queued.load(Ordering::Relaxed) > 0
                    && unit
                        .at
                        .is_some_and(|at| at.saturating_sub(held as i64) >= SPAN_MOST)
                {
                    dropped.fetch_add(1, Ordering::Relaxed);
                    resuming.skipping = true;
                    continue;
                }
                match access.try_send(Out::Access(unit)) {
                    Ok(()) => {
                        queued.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Full(_)) => {
                        dropped.fetch_add(1, Ordering::Relaxed);
                        resuming.skipping = true;
                    }
                    Err(TrySendError::Disconnected(_)) => return false,
                }
            }
            // 字幕は表示時刻まで保留する（読み取りは先まで読んでいる）。
            Out::Caption { at: Some(_), .. } => held.keep(out),
            out @ (Out::DataStreams(_)
            | Out::ApplicationService(_)
            | Out::Data { .. }
            | Out::Item { .. }
            | Out::CaptionStreams(_)
            | Out::Caption { .. }) => {
                if let Some(data) = data {
                    data.send(out);
                }
            }
            // 先に `skip_to` を 0 に戻す（新しい番組の符号化データを読み飛ばさないように）。
            Out::Switched => {
                skip_to.store(0, Ordering::Relaxed);
                resuming.skipping = false;
                resuming.last_sound.clear();
                let _ = rest.send(Out::Switched);
                if access.send(Out::Switched).is_err() {
                    return false;
                }
            }
            // Everything else waits: losing the sound is what this arrangement exists to avoid.
            other => {
                if let Out::Trouble { what, .. } = &other
                    && what.starts_with(crate::engine::NOT_READABLE)
                {
                    unreadable.store(true, Ordering::Relaxed);
                }
                if rest.send(other).is_err() {
                    return false;
                }
            }
        }
    }
    true
}

#[derive(Default)]
pub(super) struct Held {
    captions: Vec<Out>,
}

const HELD_MOST: usize = 64;

impl Held {
    pub(super) fn keep(&mut self, out: Out) {
        self.captions.push(out);
    }

    pub(super) fn release(&mut self, shared: &Shared, data: Option<&Feed>) {
        if self.captions.is_empty() {
            return;
        }
        let Some(data) = data else {
            self.captions.clear();
            return;
        };
        let now = shared.with_clock(|clock| clock.now());
        let mut kept = Vec::new();
        for out in self.captions.drain(..) {
            let at = match &out {
                Out::Caption { at: Some(at), .. } => *at,
                _ => i64::MIN,
            };
            match now {
                Some(now) if at <= now => data.send(out),
                _ => kept.push(out),
            }
        }
        if kept.len() > HELD_MOST {
            let over = kept.len() - HELD_MOST;
            for out in kept.drain(..over) {
                data.send(out);
            }
        }
        self.captions = kept;
    }
}

/// 符号化データを飛ばしたあと、どこからデコードを再開するか。CRA 直後の RASL は捨てる（H.265 8.1.3）。
#[derive(Debug, Default)]
pub(super) struct Resuming {
    pub(super) skipping: bool,
    pub(super) last_sound: std::collections::BTreeMap<u16, i64>,
    pub(super) jump: i64,
    pub(super) leading: bool,
    pub(super) entry: Option<i64>,
}

const JUMP_AHEAD: i64 = 3 * crate::engine::clock::HZ;
const JUMP_BACK: i64 = crate::engine::clock::HZ / 2;

impl Resuming {
    pub(super) fn jumped(&mut self, stream: u16, at: i64) -> bool {
        let last = self.last_sound.insert(stream, at);
        let Some(last) = last else {
            return false;
        };
        let jumped = at - last > JUMP_AHEAD || last - at > JUMP_BACK;
        if jumped {
            self.jump = at - last;
            self.last_sound.clear();
            self.last_sound.insert(stream, at);
        }
        jumped
    }

    /// `want` に追いついたランダムアクセスポイントで再開する（最初に見つけたポイントで再開すると遅れが残る）。
    pub(super) fn leave(&mut self, unit: &crate::engine::AccessUnit, want: i64) -> bool {
        let old = want > 0 && unit.at.is_some_and(|at| at < want);
        if old {
            self.skipping = true;
        }
        if self.skipping && !old && unit.picture.random_access {
            self.skipping = false;
            self.leading = true;
            self.entry = None;
        }
        if self.skipping {
            return true;
        }
        if self.leading {
            return self.trailing(unit);
        }
        false
    }

    /// H.264 の先行ピクチャーは NAL ユニットタイプで見分けられないので、表示時刻で判断する。
    pub(super) fn trailing(&mut self, unit: &crate::engine::AccessUnit) -> bool {
        match self.entry {
            None => {
                self.entry = unit.at.or(Some(i64::MIN));
                false
            }
            Some(entry) => {
                if unit.picture.leading || unit.at.is_some_and(|at| at < entry) {
                    return true;
                }
                if unit.at.is_some_and(|at| at > entry) {
                    self.leading = false;
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AHEAD_MOST, too_far};

    #[test]
    fn pending_ends_when_stopped() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::time::Duration;

        let buffer = crate::input::buffer::Buffer::new(4, false);
        let mut reader = buffer.reader();
        let stop = Arc::new(AtomicBool::new(false));
        let (filling, stopping) = (Arc::clone(&buffer), Arc::clone(&stop));
        let filler = std::thread::spawn(move || {
            let mut bytes = crate::input::source::pending();
            super::fill(
                &mut *bytes,
                &filling,
                &AtomicU64::new(0),
                &super::super::meter::Filling::default(),
                &stopping,
            );
        });
        let (told, ended) = std::sync::mpsc::channel();
        let read = std::thread::spawn(move || {
            assert!(reader.next().is_none(), "データが来ていないのに何か読めた");
            let _ = told.send(());
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!filler.is_finished(), "止める前に終わった");
        stop.store(true, Ordering::SeqCst);
        ended
            .recv_timeout(Duration::from_secs(2))
            .expect("停止しても読み取りのスレッドが終わらない");
        read.join().unwrap();
        filler.join().unwrap();
    }

    #[test]
    fn nothing_through_yet_is_not_too_far() {
        assert!(!too_far(0, None, 0));
        assert!(!too_far(0, Some(0), 0));
    }

    #[test]
    fn with_no_yardstick_it_reads() {
        assert!(!too_far(90_000 * 60, None, 0));
    }

    #[test]
    fn the_clock_wins_over_the_decoder() {
        let newest = 10 * 90_000;
        let now = Some(10 * 90_000 - AHEAD_MOST / 2);
        assert!(!too_far(newest as u64, now, 1));
        assert!(too_far(
            newest as u64,
            Some(10 * 90_000 - AHEAD_MOST * 2),
            newest as u64
        ));
    }

    #[test]
    fn before_the_clock_the_decoder_stands_in() {
        let oldest = 5 * 90_000u64;
        assert!(!too_far(oldest + AHEAD_MOST as u64 / 2, None, oldest));
        assert!(too_far(oldest + AHEAD_MOST as u64 * 2, None, oldest));
    }

    #[test]
    fn the_aim_itself_is_not_too_far() {
        assert!(!too_far(AHEAD_MOST as u64 + 1, Some(1), 1));
        assert!(too_far(AHEAD_MOST as u64 + 2, Some(1), 1));
    }
}

#[cfg(test)]
mod watching_tests {
    use std::collections::BTreeMap;

    use super::watched_program;
    use crate::filters::{Audio, Kind, Program, Stream};

    fn stream(id: u16, kind: Kind) -> Stream {
        Stream {
            id,
            kind,
            framing: Default::default(),
            declared: String::new(),
            tag: None,
            component: None,
            name: None,
            hierarchy: None,
            data: None,
            decode: None,
        }
    }

    fn program(number: u16, streams: Vec<Stream>) -> (u16, Program) {
        (
            number,
            Program {
                number,
                map: 0x1f0,
                streams,
                pcr: 0x1ff,
                ecm: Vec::new(),
                application: None,
                region: None,
                emergency_news: None,
                emergency: Vec::new(),
            },
        )
    }

    fn shared_av() -> BTreeMap<u16, Program> {
        let video = || stream(0x100, Kind::Video(mediadecoder::es::Codec::Mpeg2));
        let sound = || stream(0x110, Kind::Audio(Audio::Adts));
        BTreeMap::from([
            program(1, vec![video(), sound(), stream(0x140, Kind::Data)]),
            program(20, vec![video(), sound(), stream(0x140, Kind::Data)]),
            program(21, vec![video(), sound(), stream(0x160, Kind::Data)]),
        ])
    }

    #[test]
    fn a_data_service_sharing_the_picture_is_the_one_watched() {
        let programs = shared_av();
        assert_eq!(watched_program(&programs, Some(20), Some(0x100)), Some(20));
        assert_eq!(watched_program(&programs, Some(21), Some(0x110)), Some(21));
        assert_eq!(watched_program(&programs, Some(1), Some(0x100)), Some(1));
    }

    #[test]
    fn otherwise_the_programme_carrying_the_picture() {
        let programs = shared_av();
        assert_eq!(watched_program(&programs, None, Some(0x100)), Some(1));
        assert_eq!(watched_program(&programs, Some(999), Some(0x100)), Some(1));
    }

    #[test]
    fn a_data_only_service_is_the_wanted_one() {
        let programs = BTreeMap::from([program(30, vec![stream(0x170, Kind::Data)])]);
        assert_eq!(watched_program(&programs, Some(30), None), Some(30));
        assert_eq!(watched_program(&shared_av(), Some(20), None), None);
    }
}

#[cfg(test)]
mod jump_tests {
    use super::{JUMP_AHEAD, JUMP_BACK, Resuming};

    #[test]
    fn a_leap_in_the_sound_clock_is_a_jump() {
        let mut resuming = Resuming::default();
        assert!(!resuming.jumped(1, 0), "最初は比較の対象が無い");
        assert!(!resuming.jumped(1, 1_920));
        assert!(!resuming.jumped(2, 1_000), "別のストリームは別に数える");
        assert!(resuming.jumped(1, 1_920 + JUMP_AHEAD + 1));
        assert_eq!(resuming.jump, JUMP_AHEAD + 1);
        assert!(!resuming.jumped(2, 1_920 + JUMP_AHEAD + 500));
        assert!(!resuming.jumped(1, 1_920 + JUMP_AHEAD + 1 + 1_920));
        assert!(resuming.jumped(1, 1_920 + JUMP_AHEAD + 1 + 1_920 - JUMP_BACK - 1));
        assert!(resuming.jump < 0);
    }
}
