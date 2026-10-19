// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 録画を開く・選局する・スレッドを起動する。

use super::*;

impl Player {
    /// 録画を1本開く。シークできるので先頭を読んで番組を探せる（[`Player::look`]）。
    pub fn open(
        path: &str,
        sound: Option<Device>,
        options: Options,
        plugins: Option<Arc<Loaded>>,
    ) -> Result<Self, String> {
        let (container, file, recording) = open_recording(path)?;
        let mut player = Self::empty(container, sound, options, plugins);
        player.file = Some(BufReader::with_capacity(CHUNK, Growing::new(file)));
        player.recording = Some(recording);
        Ok(player)
    }

    /// ソースから来ているバイト列で開く。シークできない（[`Player::look_live`]）。
    pub fn from_source(
        bytes: crate::input::source::Bytes,
        container: Container,
        sound: Option<Device>,
        options: Options,
        plugins: Option<Arc<Loaded>>,
    ) -> Self {
        let mut player = Self::empty(container, sound, options, plugins);
        player.live = Some(bytes);
        player
    }

    fn empty(
        container: Container,
        sound: Option<Device>,
        options: Options,
        plugins: Option<Arc<Loaded>>,
    ) -> Self {
        let rate = sound.as_ref().map_or(48_000, Device::rate);
        let has_sound = sound.is_some();
        let passthrough = sound
            .as_ref()
            .map_or(crate::sound::Passthrough::Off, Device::passthrough_mode);
        // 音声のスレッドはここで起動する。チャネルは選局のたびに [`Player::start`] で作る。
        let shared = Arc::new(Shared::new(rate));
        crate::decode::set_api(options.api);
        let extra = i64::from(options.audio_latency) * i64::from(rate) / 1000;
        let say = crate::sound::sounding::spawn(sound, Arc::clone(&shared), extra, options.mix);
        if let Some(say) = say.as_ref() {
            let _ = say.send(crate::sound::sounding::Say::Gain(
                crate::sound::volume::gain(options.volume, options.muted),
            ));
        }
        Self {
            options,
            plugins,
            said_no_hardware: false,
            skip_to: Arc::new(AtomicI64::new(0)),
            video_first: Arc::new(AtomicI64::new(i64::MIN)),
            video_decoded: Arc::new(AtomicU64::new(0)),
            video_expected: Arc::new(AtomicBool::new(false)),
            video_ready: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            hidden: Arc::new(AtomicBool::new(false)),
            unseen: 0,
            left_at: None,
            left: 0,
            away: away::Away::default(),
            video_latency: 0,
            video_held: 0,
            engine: Some(Engine::new(container, rate)),
            container,
            file: None,
            recording: None,
            live: None,
            live_source: false,
            pictures: None,
            say,
            shared,
            has_sound,
            passthrough,
            rate,
            tables: None,
            last_tables: Tables::default(),
            carry: Wanted::default(),
            paused: false,
            display_mhz: 0,
            told_speed: 1.0,
            buffering: false,
            pace: 1.0,
            guides: None,
            asking: None,
            ready_to_start: None,
            video: None,
            format: None,
            settings: Settings::default(),
            ready: VecDeque::new(),
            due: VecDeque::new(),
            done: false,
            told: 0,
            holding: false,
            tuned: Instant::now(),
            first: First::default(),
            first_picture: None,
            flushed: false,
            drawn: 0,
            tuned_counts: (0, 0),
            late: 0,
            numbered: None,
            redrawn: 0,
            #[cfg(all(feature = "wgpu", feature = "plugins"))]
            filter_host: None,
            since: 0,
            stalled: false,
            drew: false,
            drew_at: None,
            refused: false,
            took: 0,
            meter: Meter::new(),
            read_bytes: Arc::new(AtomicU64::new(0)),
            hold: Arc::new(AtomicU32::new(HOLD_MS)),
            pile_most: PILE_MOST,
            read_stamp: Arc::new(AtomicI64::new(i64::MIN)),
            read_stamp_first: Arc::new(AtomicI64::new(i64::MIN)),
            dropped_access: Arc::new(AtomicU64::new(0)),
            newest_access: Arc::new(AtomicU64::new(0)),
            oldest_access: Arc::new(AtomicU64::new(0)),
            queued_access: Arc::new(AtomicI64::new(0)),
            newest_sound: Arc::new(AtomicU64::new(0)),
            spent_reading: Arc::new(Reading::default()),
            spent_filling: Arc::new(Filling::default()),
            ring: None,
            spent_decoding: Arc::new(Decodes::default()),
            spent_meter: Spent::default(),
            spent_total: Spent::default(),
            rate_sample: (Instant::now(), 0),
            data_to: None,
            inset: None,
            pan_scan: crate::output::video::pan_scan::PanScan::default(),
            bitrate: 0.0,
        }
    }

    /// 別のバイト列で始め直す（選局）。音声の出力と GPU のサーフェスはそのまま使う。
    pub fn retune(
        &mut self,
        bytes: crate::input::source::Bytes,
        container: Container,
        wanted: Option<u16>,
    ) -> bool {
        self.begin_again(container);
        self.live = Some(bytes);
        self.look_live(wanted)
    }

    fn begin_again(&mut self, container: Container) {
        self.drain_spent();
        self.spent_total = Spent::default();
        self.tuned_counts = (self.drawn, self.late);
        self.pictures = None;
        self.asking = None;
        self.tables = None;
        self.guides = None;
        let rate = self.rate;
        // チャネルは一つなので、`Stop` はあとの `Play` より先に届く。
        self.tell_sound(Say::Stop { rate });
        self.buffering = false;
        self.engine = Some(Engine::new(container, rate));
        self.container = container;
        self.ready.clear();
        self.due.clear();
        self.done = false;
        self.holding = false;
        self.tuned = Instant::now();
        self.first = First::default();
        self.first_picture = None;
        self.flushed = false;
        self.format = None;
        self.video = None;
        self.skip_to = Arc::new(AtomicI64::new(0));
        self.video_first = Arc::new(AtomicI64::new(i64::MIN));
        self.video_decoded = Arc::new(AtomicU64::new(0));
        self.video_expected = Arc::new(AtomicBool::new(false));
        self.video_ready = Arc::new(AtomicBool::new(false));
        self.stop.store(true, Ordering::SeqCst);
        self.stop = Arc::new(AtomicBool::new(false));
        self.left_at = None;
        self.shared.picture.store(false, Ordering::Relaxed);
        self.shared.unreadable.store(false, Ordering::Relaxed);
        self.since = 0;
        self.stalled = false;
        self.drew = false;
        self.refused = false;
        self.last_tables = Tables::default();
        self.told += 1;
        self.read_bytes = Arc::new(AtomicU64::new(0));
        self.read_stamp = Arc::new(AtomicI64::new(i64::MIN));
        self.read_stamp_first = Arc::new(AtomicI64::new(i64::MIN));
        self.dropped_access = Arc::new(AtomicU64::new(0));
        self.newest_access = Arc::new(AtomicU64::new(0));
        self.oldest_access = Arc::new(AtomicU64::new(0));
        self.queued_access = Arc::new(AtomicI64::new(0));
        self.newest_sound = Arc::new(AtomicU64::new(0));
        self.spent_reading = Arc::new(Reading::default());
        self.spent_filling = Arc::new(Filling::default());
        self.ring = None;
        self.spent_decoding = Arc::new(Decodes::default());
        self.rate_sample = (Instant::now(), 0);
        self.bitrate = 0.0;
        self.file = None;
        self.live = None;
        self.ready_to_start = None;
    }

    pub fn play_file(&mut self, path: &str) -> Result<(), String> {
        let (container, file, recording) = open_recording(path)?;
        self.begin_again(container);
        self.file = Some(BufReader::with_capacity(CHUNK, Growing::new(file)));
        self.recording = Some(recording);
        // 何も選ばずに始めると何も流れない（`Selector::follow`）。
        if !self.look(None) {
            self.recording = None;
            return Err(format!("再生できる番組がありません。\n{path}"));
        }
        self.paused = false;
        self.start();
        Ok(())
    }

    pub fn await_nothing(&mut self) {
        self.begin_again(self.container);
        self.recording = None;
        self.live = Some(crate::input::source::pending());
        self.look_live(None);
        self.start();
    }

    /// 録画が追記されていれば長さを測り直す。二秒に一度まで。
    pub fn refresh_recording(&mut self) {
        let Some(recording) = self.recording.as_mut() else {
            return;
        };
        if recording.measured.elapsed() < Duration::from_secs(2) {
            return;
        }
        recording.measured = Instant::now();
        let Ok(mut file) = File::open(&recording.path) else {
            return;
        };
        let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
        if bytes <= recording.bytes && recording.span.is_some() {
            return;
        }
        let container = self.container;
        if let Some(span) = crate::input::span::Span::measure(container, &mut file) {
            if recording.span.is_none() {
                tracing::info!(
                    "録画の長さ {}（再計測）",
                    crate::input::span::clock_text(span.length())
                );
                if recording.opened_at == 0 {
                    recording.opened_at = span.first;
                }
            }
            recording.bytes = bytes;
            recording.span = Some(span);
        }
    }

    pub fn leave_recording(&mut self) {
        self.recording = None;
        if self.paused || self.buffering {
            self.paused = false;
            self.buffering = false;
            self.tell_sound(Say::Pause(false));
        }
        self.tell_speed();
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// 現在のストリームでソースから読んだバイトの累計。選局し直すと 0 から。
    pub fn read_bytes(&self) -> u64 {
        self.read_bytes.load(Ordering::Relaxed)
    }

    /// 映像が一秒間レンダリングできていない（表示できない、または届かない）。
    pub fn stalled(&self) -> bool {
        self.stalled
            || self
                .drew_at
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
    }

    /// 現在のビットレート（Mbps）。一秒ごとに計算し直す。
    pub fn bitrate(&mut self) -> f64 {
        let now = Instant::now();
        let (since, then) = self.rate_sample;
        let seconds = now.duration_since(since).as_secs_f64();
        if seconds >= 1.0 {
            let bytes = self.read_bytes.load(Ordering::Relaxed);
            self.bitrate = (bytes.saturating_sub(then) as f64) * 8.0 / seconds / 1_000_000.0;
            self.rate_sample = (now, bytes);
        }
        self.bitrate
    }

    /// Reads the head of the recording with an engine of its own, and picks a programme.
    pub fn look(&mut self, wanted: Option<u16>) -> bool {
        let Some(file) = self.file.as_mut() else {
            return false;
        };
        let mut head = vec![0; HEAD];
        let mut filled = 0;
        while filled < head.len() {
            match file.read(&mut head[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) => {
                    tracing::warn!("録画を読み込めない: {error}");
                    break;
                }
            }
        }
        head.truncate(filled);
        let mut looking = Engine::new(self.container, self.rate);
        looking.push(&head);
        let chosen = wanted.or_else(|| looking.best());
        for (number, program) in looking.programs() {
            let mark = if Some(*number) == chosen { " ←" } else { "" };
            let seg = if program.one_seg() {
                " ワンセグ"
            } else {
                ""
            };
            let what: Vec<String> = program
                .streams
                .iter()
                .map(|stream| format!("0x{:04x} {:?}", stream.id, stream.kind))
                .collect();
            tracing::info!("番組 {number}{seg}{mark}  {}", what.join("  "));
        }
        let Some(program) = chosen else {
            tracing::info!("再生できる番組がない");
            return false;
        };
        let (Some(mut engine), Some(mut file)) = (self.engine.take(), self.file.take()) else {
            return false;
        };
        engine.want(program);
        if let Err(error) = file.rewind() {
            tracing::warn!("録画の先頭に戻れない: {error}");
            return false;
        }
        self.live_source = false;
        if let Some(recording) = self.recording.as_mut() {
            recording.program = Some(program);
        }
        self.ready_to_start = Some((engine, Box::new(file)));
        true
    }

    /// 読み取りスレッドが見た伝送路の時刻を、再生の時計の基準で返す。
    fn stamped_now(&self) -> Option<i64> {
        let latest = self.read_stamp.load(Ordering::Relaxed);
        let first = self.read_stamp_first.load(Ordering::Relaxed);
        if latest == i64::MIN || first == i64::MIN {
            return None;
        }
        Some(match self.container {
            Container::Ts => latest,
            Container::Tlv => latest - first,
        })
    }

    pub fn progress(&self) -> Option<Progress> {
        let recording = self.recording.as_ref()?;
        let span = recording.span?;
        let length = span.length();
        if length <= 0 {
            return None;
        }
        // 時計がまだ無ければ伝送路の時刻、それも無ければ開いた位置。
        let now = self
            .now()
            .or_else(|| self.stamped_now())
            .unwrap_or(match self.container {
                Container::Ts => recording.opened_at,
                Container::Tlv => 0,
            });
        let position = span.position(recording.opened_at, now);
        Some(Progress {
            fraction: position as f32 / length as f32,
            position,
            length,
            paused: self.paused,
            buffering: self.buffering,
            pace: self.pace,
        })
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.recording.is_none() || self.paused == paused {
            return;
        }
        self.paused = paused;
        self.tell_sound(Say::Pause(paused || self.buffering));
    }

    pub fn buffering(&self) -> bool {
        self.buffering
    }

    /// 録画の読み込みが詰まったら一時停止し、追いついたら再開する。追っかけ再生の末尾では止めない。
    pub(super) fn watch_reading(&mut self) {
        let lead = self.reading_lead();
        let Some(ring) = self.ring.as_ref().filter(|_| self.recording.is_some()) else {
            self.set_buffering(false);
            return;
        };
        if self.buffering {
            if lead.is_some_and(|lead| lead >= LEAD_RESUME) || ring.ended() {
                self.set_buffering(false);
            }
        } else if ring.starved().is_some_and(|waited| waited >= STARVED_LEAST)
            && lead.is_some_and(|lead| lead < LEAD_LOW)
        {
            self.set_buffering(true);
        }
    }

    fn reading_lead(&self) -> Option<i64> {
        let now = self.now()?;
        match self.newest_sound.load(Ordering::Relaxed) {
            0 => self.stamped_now().map(|read| read - now),
            newest => Some(newest as i64 - now),
        }
    }

    fn set_buffering(&mut self, buffering: bool) {
        if self.buffering == buffering {
            return;
        }
        self.buffering = buffering;
        tracing::info!(
            "{}",
            if buffering {
                "録画の読み込みが追いつきません。一時停止します"
            } else {
                "録画の読み込みが追いつきました。再開します"
            }
        );
        if !self.paused {
            self.tell_sound(Say::Pause(buffering));
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_pace(&mut self, pace: f64) {
        if self.recording.is_none() {
            return;
        }
        let pace = if pace.is_finite() {
            pace.clamp(0.25, 8.0)
        } else {
            1.0
        };
        if (self.pace - pace).abs() < 1e-6 {
            return;
        }
        self.pace = pace;
        self.tell_speed();
    }

    pub fn set_display(&mut self, refresh: u32) {
        if self.display_mhz != refresh {
            self.display_mhz = refresh;
            if let Some(video) = self.video.as_mut() {
                video.set_display_rate(refresh);
            }
            self.tell_speed();
        }
    }

    /// 画面のリフレッシュレートに合わせる再生速度の倍率。
    /// 録画だけ（ライブ再生ではバッファを使い切ってしまう）。
    fn display_sync(&self) -> f64 {
        let Some(format) = self.format else {
            return 1.0;
        };
        if self.recording.is_none()
            || self.passes_sound_through()
            || self.display_mhz == 0
            || format.rate.0 == 0
            || format.rate.1 == 0
        {
            return 1.0;
        }
        let fps = f64::from(format.rate.0) / f64::from(format.rate.1);
        let ratio = f64::from(self.display_mhz) / 1000.0 / fps;
        let multiple = (ratio * 2.0).round() / 2.0;
        if !(1.0..=4.0).contains(&multiple) {
            return 1.0;
        }
        let factor = ratio / multiple;
        if (factor - 1.0).abs() > 0.01 {
            return 1.0;
        }
        factor
    }

    pub(super) fn tell_speed(&mut self) {
        let sync = self.display_sync();
        let speed = if self.recording.is_some() {
            self.pace * sync
        } else {
            1.0
        };
        if (speed - self.told_speed).abs() < 1e-9 {
            return;
        }
        let was_synced = (self.told_speed / self.pace.max(1e-6) - 1.0).abs() > 1e-6;
        if (sync - 1.0).abs() > 1e-9 && !was_synced {
            tracing::info!(
                "画面に合わせて {sync:.4} 倍速で再生（{:.3} Hz、{:.3} fps）",
                f64::from(self.display_mhz) / 1000.0,
                self.format.map_or(0.0, |format| f64::from(format.rate.0)
                    / f64::from(format.rate.1.max(1))),
            );
        }
        self.told_speed = speed;
        self.tell_sound(Say::Speed(speed));
    }

    pub fn pace(&self) -> f64 {
        self.pace
    }

    /// 録画のこの位置（0〜1）から読み直す。
    pub fn seek_to(&mut self, fraction: f32) -> bool {
        self.reopen(fraction, None)
    }

    /// 同じ録画の別の番組へ。全部を新しく始める（一部だけ元に戻すと戻し忘れが出る）。
    pub fn switch_program(&mut self, service: u16) -> bool {
        let Some(progress) = self.progress() else {
            return false;
        };
        self.reopen(progress.fraction, Some(service))
    }

    fn reopen(&mut self, fraction: f32, wanted: Option<u16>) -> bool {
        let Some(recording) = self.recording.as_ref() else {
            return false;
        };
        let Some(span) = recording.span else {
            return false;
        };
        let mut file = match File::open(&recording.path) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!("{}: {error}", recording.path);
                return false;
            }
        };
        // ビットレートが一定でないので、目標の時刻の半秒以内に入るまでバイト位置で近づける。
        let target = span.first + (span.length() as f64 * f64::from(fraction)) as i64;
        let per_tick = recording.bytes as f64 / span.length().max(1) as f64;
        let mut offset = span.offset(fraction, recording.bytes);
        let mut stamp = None;
        for _ in 0..4 {
            let mut look =
                vec![0; crate::input::span::LOOK.min((recording.bytes - offset) as usize)];
            let found = file
                .seek(std::io::SeekFrom::Start(offset))
                .ok()
                .and_then(|_| file.read_exact(&mut look).ok())
                .and_then(|_| crate::input::span::first_stamp(self.container, &look))
                .map(|found| span.unwrapped(found));
            let Some(found) = found else {
                break;
            };
            stamp = Some(found);
            let error = found - target;
            if error.abs() <= crate::engine::clock::HZ / 2 {
                break;
            }
            let step = (error as f64 * per_tick) as i64;
            let moved = (offset as i64 - step).clamp(0, recording.bytes as i64 - 1) as u64;
            if moved == offset {
                break;
            }
            offset = moved;
        }
        let stamp = stamp.unwrap_or(target);
        if let Err(error) = file.seek(std::io::SeekFrom::Start(offset)) {
            tracing::warn!("{}: シークできない: {error}", recording.path);
            return false;
        }
        self.carry = if wanted.is_some() {
            Wanted::default()
        } else {
            Wanted {
                audio: self.last_tables.sound_at.map(|at| at + 1),
                video: self.last_tables.picture_at.map(|at| at + 1),
            }
        };
        let program = wanted
            .or_else(|| self.tables().watching.map(|key| key.service))
            .or(recording.program);
        tracing::info!(
            "録画の {} にシーク（{} MB、{:.0}%）",
            crate::input::span::clock_text(stamp - span.first),
            offset >> 20,
            fraction * 100.0
        );
        let container = self.container;
        if !self.retune(
            Box::new(BufReader::with_capacity(CHUNK, Growing::new(file))),
            container,
            program,
        ) {
            return false;
        }
        self.live_source = false;
        if let Some(recording) = self.recording.as_mut() {
            recording.opened_at = stamp;
            if let Some(program) = program {
                recording.program = Some(program);
            }
        }
        self.start();
        if self.paused {
            self.tell_sound(Say::Pause(true));
        }
        true
    }

    /// 番組表を先に渡しておく。開いたあとは渡す手段が無い。
    pub fn seed_guide(&mut self, guide: Guide) {
        let Some(engine) = self.engine.as_mut() else {
            tracing::debug!("再生開始後のため番組表を渡せない");
            return;
        };
        *engine.guide_mut() = guide;
    }

    /// ソースから来ているバイト列で始める。[`Engine::want`] でそのサービスが現れた瞬間に選ばせる。
    pub fn look_live(&mut self, wanted: Option<u16>) -> bool {
        let (Some(mut engine), Some(bytes)) = (self.engine.take(), self.live.take()) else {
            tracing::info!("ソースが開かれていない");
            return false;
        };
        match wanted {
            Some(service) => {
                tracing::info!("サービス {service} を待っている");
                engine.want(service);
            }
            None => tracing::info!("サービスが指定されていない"),
        }
        self.live_source = true;
        self.ready_to_start = Some((engine, bytes));
        true
    }

    /// Starts decoding. Only once the GPU is open: the decoder depends on the device opened
    /// (D3D12 output cannot be drawn on Vulkan).
    pub fn start(&mut self) {
        let Some((mut engine, mut file)) = self.ready_to_start.take() else {
            return;
        };
        let (access_tx, access_rx) = sync_channel::<Out>(ACCESS);
        let (rest_tx, rest) = sync_channel::<Out>(REST);
        let (pictures_tx, pictures) = sync_channel::<Out>(HANDOVER);
        let (asking, asks) = std::sync::mpsc::channel();
        let (told, tables) = sync_channel(1);
        let (guided, guides) = sync_channel(1);
        engine.set_dual_mono(self.options.dual_mono);
        // The device was opened first, since its rate is what the clock counts in.
        engine.set_passthrough(self.passthrough);
        let wanted = Wanted {
            audio: self.carry.audio.or(self.options.audio),
            video: self.carry.video.or(self.options.video),
        };
        self.carry = Wanted::default();
        self.asking = Some(asking);
        let bytes = Arc::clone(&self.read_bytes);
        let stamped = Stamped {
            latest: Arc::clone(&self.read_stamp),
            first: Arc::clone(&self.read_stamp_first),
        };
        let dropped = Arc::clone(&self.dropped_access);
        let newest = Arc::clone(&self.newest_access);
        let newest_sound = Arc::clone(&self.newest_sound);
        let skip_to = Arc::clone(&self.skip_to);
        let stop = Arc::clone(&self.stop);
        let reading = Arc::clone(&self.spent_reading);
        let oldest = Arc::clone(&self.oldest_access);
        let queued = Arc::clone(&self.queued_access);
        let live = self.live_source;
        let data = self.data_to.clone().map(Feed::new);
        let paced = Arc::clone(&self.shared);
        // 録画ではライターが待つ（待たずに読み進めると上書きしてしまう）。
        let buffer = if live {
            crate::input::buffer::Buffer::holding(Arc::clone(&self.hold))
        } else {
            crate::input::buffer::Buffer::new(BLOCKS_RECORDING, true)
        };
        let mut taking = buffer.reader().reading();
        let filling = Arc::clone(&buffer);
        let filled = Arc::clone(&self.spent_filling);
        let stopped = Arc::clone(&self.stop);
        let filler = std::thread::Builder::new()
            .name(String::from("fill"))
            .spawn(move || fill(&mut *file, &filling, &bytes, &filled, &stopped));
        if let Err(error) = filler {
            tracing::warn!("フェッチスレッドを起動できない: {error}");
        }
        let chain = crate::plugins::newest(&self.plugins).map(|plugins| plugins.stream_chain());
        let reader = std::thread::Builder::new()
            .name(String::from("read"))
            .spawn(move || {
                read(
                    chain,
                    &mut taking,
                    &mut engine,
                    &access_tx,
                    &rest_tx,
                    &dropped,
                    &newest,
                    &oldest,
                    &queued,
                    &newest_sound,
                    &skip_to,
                    &stop,
                    &paced,
                    live,
                    stamped,
                    &reading,
                    &asks,
                    &told,
                    &guided,
                    wanted,
                    data.as_ref(),
                )
            });
        if let Err(error) = reader {
            tracing::warn!("読み取りスレッドを起動できない: {error}");
        }
        let mut decoding =
            Decoding::with_deinterlace_by(self.options.prefer(), self.options.deinterlace_by);
        let decoding_spent = Arc::clone(&self.spent_decoding);
        let decoding_skip = Arc::clone(&self.skip_to);
        let decoding_first = Arc::clone(&self.video_first);
        let decoding_decoded = Arc::clone(&self.video_decoded);
        let decoding_expected = Arc::clone(&self.video_expected);
        let decoding_dropped = Arc::clone(&self.dropped_access);
        let decoding_oldest = Arc::clone(&self.oldest_access);
        let decoding_queued = Arc::clone(&self.queued_access);
        let decoder = std::thread::Builder::new()
            .name(String::from("decode"))
            .spawn(move || {
                decode(
                    &mut decoding,
                    &access_rx,
                    &pictures_tx,
                    &decoding_skip,
                    &decoding_first,
                    &decoding_decoded,
                    &decoding_expected,
                    &decoding_dropped,
                    &decoding_oldest,
                    &decoding_queued,
                    &decoding_spent,
                );
            });
        if let Err(error) = decoder {
            tracing::warn!("デコードスレッドを起動できない: {error}");
        }
        self.ring = Some(Arc::clone(&buffer));
        self.pictures = Some(pictures);
        self.tell_sound(Say::Play {
            rest,
            skip_to: Arc::clone(&self.skip_to),
            video_first: Arc::clone(&self.video_first),
            video_decoded: Arc::clone(&self.video_decoded),
            video_expected: Arc::clone(&self.video_expected),
            video_ready: Arc::clone(&self.video_ready),
            live: self.live_source,
        });
        self.tables = Some(tables);
        self.guides = Some(guides);
    }
}

/// 録画を開いて長さを測り、先頭へ戻す。エラーの文は一行目が見出し、改行の後ろが詳細。
fn open_recording(path: &str) -> Result<(Container, File, Recording), String> {
    // 録画はチューナーを貸さない（`plugins::lend`）。
    #[cfg(feature = "plugins")]
    crate::plugins::lend(None);
    let container = Container::of_file(path).ok_or_else(|| {
        format!(
            "このファイルは再生できません。\n対応していない形式です（対応形式: TS・MMT/TLV ）。\n{path}"
        )
    })?;
    let mut file =
        File::open(path).map_err(|error| format!("ファイルを開けません。\n{error}\n{path}"))?;
    let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    let span = crate::input::span::Span::measure(container, &mut file);
    file.rewind().map_err(|error| {
        format!("ファイルを開けません。\nファイル先頭へ移動できません。{error}\n{path}")
    })?;
    match span {
        Some(span) => tracing::info!(
            "録画の長さ {}（{} MB）",
            crate::input::span::clock_text(span.length()),
            bytes >> 20
        ),
        None => tracing::warn!("録画の長さを取得できない（シークバーは表示しない）"),
    }
    let recording = Recording {
        path: path.to_owned(),
        bytes,
        span,
        opened_at: span.map_or(0, |span| span.first),
        program: None,
        measured: Instant::now(),
    };
    Ok((container, file, recording))
}
