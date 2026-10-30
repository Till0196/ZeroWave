// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 選局。番組表からキーを取得して、ソースに要求し、返ってきたストリームを再生する。

use super::*;
use zwcore::input::source::Source as _;

pub(super) fn open_mirakurun(
    url: &str,
    by: zwcore::input::source::mirakurun::By,
) -> Result<Live, String> {
    let mut source =
        zwcore::input::source::mirakurun::Mirakurun::at(url).map_err(|why| why.to_string())?;
    source.set_by(by);
    let version = source.version().map_err(|why| why.to_string())?;
    let channels = source.channels().map_err(|why| why.to_string())?;
    tracing::info!(
        "{} バージョン {version}  {} 局",
        source.name(),
        channels.len()
    );
    let (send, fetched) = std::sync::mpsc::channel();
    let far = url.to_owned();
    std::thread::Builder::new()
        .name(String::from("guide"))
        .spawn(move || {
            crate::threads::background::lower();
            let Ok(mut source) = zwcore::input::source::mirakurun::Mirakurun::at(&far) else {
                return;
            };
            if let Err(why) = source.guide_in_parts(|part| {
                let _ = send.send(part);
            }) {
                tracing::warn!("全局の番組表を取得できない: {why}");
            }
        })
        .ok();
    Ok(Live {
        supply: Supply::new(Box::new(source)),
        channels,
        fetched: Some(fetched),
        url: Some(url.to_owned()),
        starting: None,
    })
}

/// coaxd を開いて [`Live`] にする。全局の番組表は無い（放送波から埋まる）。
pub(super) fn open_coaxd(
    endpoint: &str,
    priority: i32,
    tuners: &[String],
    profile: &str,
) -> Result<Live, String> {
    use zwcore::input::source::coaxd::Coaxd;
    let mut source = Coaxd::written(endpoint, priority).with_tuners(tuners.to_vec());
    let version = source.version().map_err(|why| why.to_string())?;
    let mapped = mapped_for(endpoint, priority, tuners, profile);
    let channels = if mapped.is_empty() {
        source.channels().map_err(|why| why.to_string())?
    } else {
        mapped
    };
    tracing::info!(
        "{} バージョン {version}  {} 局",
        source.name(),
        channels.len()
    );
    Ok(Live {
        supply: Supply::new(Box::new(source)),
        channels,
        fetched: None,
        url: Some(came_of_coaxd(endpoint, tuners, profile)),
        starting: None,
    })
}

/// プロファイルのチャンネルマップの局をチューナーで絞る。決めていない・チューナーの一覧が取れないなら全部。
pub(crate) fn mapped_for(
    endpoint: &str,
    priority: i32,
    tuners: &[String],
    profile: &str,
) -> Vec<zwcore::input::source::Channel> {
    let lines = if tuners.is_empty() {
        None
    } else {
        match zwcore::input::source::coaxd::boards(endpoint, priority) {
            Ok(boards) => {
                let listed: Vec<&zwcore::input::source::coaxd::DaemonTuner> = boards
                    .iter()
                    .flat_map(|board| &board.tuners)
                    .map(|held| &held.tuner)
                    .collect();
                match lines_for_tuners(&crate::config::load_plain(), tuners, &listed) {
                    // チューナーが無いか方式を返さない。空だと絞り込みが外れ all の局まで並ぶ。
                    Some(lines) if lines.is_empty() => Some(own_lines(profile)),
                    lines => lines,
                }
            }
            // 方式が分からない。all は足さない（受けられない方式の局が並ぶ）。
            Err(why) => {
                tracing::warn!(
                    "チューナーの一覧を取得できないため、このプロファイルのスキャン結果だけを使う: {why}"
                );
                Some(own_lines(profile))
            }
        }
    };
    mapped_channels(lines.as_deref(), profile)
}

fn own_lines(profile: &str) -> Vec<String> {
    crate::config::load_channels_of(profile)
        .lines
        .into_keys()
        .collect()
}

/// プロファイルのチューナーで受ける経路。決めていなければ `None`（全部）。
/// 並びはチューナーの順、同じチューナーの中は方式の順（同じ局を試す順）。
pub(crate) fn lines_for_tuners(
    _file: &crate::config::File,
    patterns: &[String],
    listed: &[&zwcore::input::source::coaxd::DaemonTuner],
) -> Option<Vec<String>> {
    if patterns.is_empty() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for pattern in patterns {
        let mine: Vec<&&zwcore::input::source::coaxd::DaemonTuner> = listed
            .iter()
            .filter(|tuner| tuner.matches(pattern))
            .collect();
        for plan in crate::ui::panes::settings::PLANS {
            if out.iter().any(|had| had == plan) {
                continue;
            }
            let (_, system) = crate::ui::panes::settings::offered(plan);
            if mine.iter().any(|tuner| serves(tuner, system)) {
                out.push((*plan).to_owned());
            }
        }
    }
    Some(out)
}

pub(crate) fn serves(tuner: &zwcore::input::source::coaxd::DaemonTuner, system: &str) -> bool {
    !tuner.deliveries.is_empty()
        && tuner.receives(system)
        && zwcore::input::source::coaxd::tuner_setting(&tuner.id).allows(system)
}

pub(crate) fn line_order(channels: &crate::config::channels::Channels) -> Vec<String> {
    crate::ui::panes::settings::PLANS
        .iter()
        .map(|plan| (*plan).to_owned())
        .chain(
            channels
                .lines
                .keys()
                .filter(|key| !crate::ui::panes::settings::PLANS.contains(&key.as_str()))
                .cloned(),
        )
        .collect()
}

/// チャンネルマップと state.toml の記録から選べる局をビルドする。経路の並びが選局の順。
pub(crate) fn mapped_channels(
    lines: Option<&[String]>,
    profile: &str,
) -> Vec<zwcore::input::source::Channel> {
    use zwcore::filters::Container;
    let channels = crate::config::load_channels_used(profile);
    if channels.lines.is_empty() {
        return Vec::new();
    }
    let state = crate::config::load_state();
    let order: Vec<String> = match lines {
        Some(lines) => lines.to_vec(),
        None => line_order(&channels),
    };
    let mut out = Vec::new();
    for label in &order {
        let Some(line) = channels.lines.get(label) else {
            continue;
        };
        let plan = label.clone();
        for (stream, carriers) in &line.streams {
            let Some(carrier) = carriers.first() else {
                continue;
            };
            let how = how_of(&carrier.tune_words(&plan));
            // ISDB-S3 は TLV。スロットが `tlv:n` でも TLV。
            let system = carrier.system().unwrap_or(&plan).to_ascii_lowercase();
            let container = if system.contains("isdbs3")
                || system.contains("4k")
                || carrier.slot_parts().is_some_and(|(tlv, _)| tlv)
            {
                Container::Tlv
            } else {
                Container::Ts
            };
            let known = state.streams.get(stream);
            let services = known.map(|known| &known.services);
            match services.filter(|services| !services.is_empty()) {
                Some(services) => {
                    for (service, name) in services {
                        out.push(zwcore::input::source::Channel {
                            name: name.clone(),
                            key: Some(zwcore::guide::Key {
                                network: stream.network,
                                service: *service,
                            }),
                            transport: Some(stream.transport),
                            order: known.and_then(|known| known.remote),
                            how: how.clone(),
                            container,
                        });
                    }
                }
                None => out.push(zwcore::input::source::Channel {
                    name: format!("{label} {stream}"),
                    key: None,
                    transport: Some(stream.transport),
                    order: None,
                    how: how.clone(),
                    container,
                }),
            }
        }
    }
    out
}

/// チャンネルマップの局と探し直し（`verify`）が同じ形を使う（比べるので）。
pub(crate) fn how_of(words: &[(String, String)]) -> String {
    words
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

pub(crate) fn said_of(why: &zwcore::input::source::Error) -> String {
    let text = zwcore::input::source::coaxd::explain(&why.to_string());
    if text.contains("no tuner can reach") {
        return String::from(
            "この放送を受けられるチューナーがありません（チューナーが挿さっているか確かめてください）",
        );
    }
    if text.contains("busy") || text.contains("in use") || text.contains("leased") {
        return format!("チューナーが使用中です（{text}）");
    }
    text
}

fn refused(name: &str, why: &zwcore::input::source::Error) -> String {
    let title = "チャンネルを変更できません。";
    let text = zwcore::input::source::coaxd::explain(&why.to_string());
    if text.contains("no tuner can reach") {
        return format!(
            "{title}\n受信できるチューナーがありません。\nチューナーの接続を確認してください。（{name}）"
        );
    }
    if text.contains("busy") || text.contains("in use") || text.contains("leased") {
        return format!("{title}\nチューナーが使用中です。（{name}）");
    }
    let (head, _) = zwcore::input::source::coaxd::explain_apart(&why.to_string());
    let head = head.trim();
    if head.is_empty() {
        return title.to_owned();
    }
    let closed = if head.is_ascii() || head.ends_with('。') {
        head.to_owned()
    } else {
        format!("{head}。")
    };
    format!("{title}\n{closed}（{name}）")
}

pub(crate) fn came_of_coaxd(endpoint: &str, tuners: &[String], profile: &str) -> String {
    // チューナーの絞り方とプロファイルも含める（変われば開き直す）。
    let profile = crate::config::channels_profile(profile);
    if tuners.is_empty() {
        format!("coaxd:{endpoint}|{profile}")
    } else {
        format!("coaxd:{endpoint}|{profile}|{}", tuners.join(","))
    }
}

impl Stage {
    pub(super) fn band(&self) -> Band {
        self.band
            .or_else(|| {
                self.player
                    .tables()
                    .watching
                    .map(|key| Band::of(key.network))
            })
            .unwrap_or_default()
    }

    pub(super) fn bands(&self) -> Vec<Band> {
        guide::bands_of(self.stations(), self.band(), self.receivable.as_deref())
    }

    pub(super) fn keys_of(&self, band: Band) -> Vec<zwcore::guide::Key> {
        let only = self.receivable.as_deref();
        let mut services: Vec<&zwcore::guide::Service> = self
            .stations()
            .channels()
            .filter(|service| service.band() == band)
            .filter(|service| only.is_none_or(|only| only.contains(&service.key())))
            .collect();
        services.sort_by_key(|service| service.order());
        services.into_iter().map(|service| service.key()).collect()
    }

    /// いまのソースで選べる局。ソースが開けていなくてもカタログで絞る
    /// （絞らないと番組表にたまった別のプロファイルの局まで並ぶ）。
    fn receivable_now(&self) -> Option<Arc<std::collections::BTreeSet<zwcore::guide::Key>>> {
        if self.player.is_recording() || (self.live.is_none() && self.player_mode) {
            return None;
        }
        let mut keys: std::collections::BTreeSet<zwcore::guide::Key> = self
            .live
            .iter()
            .flat_map(|live| &live.channels)
            .chain(self.catalog.iter().flat_map(|offer| &offer.channels))
            .filter_map(|channel| channel.key)
            .collect();
        if keys.is_empty() && self.live.is_some() {
            return None;
        }
        // ソースが一覧に出さないラジオなども選べる。
        for service in &self.player.tables().services {
            if service.watchable() {
                keys.insert(service.key());
            }
        }
        keys.retain(|key| !self.hidden.iter().any(|pattern| pattern.matches(*key)));
        Some(Arc::new(keys))
    }

    /// 録画の再生中は録画に含まれる情報だけの番組表を使わせ、番組表のファイルには混ぜない。
    pub(super) fn tell_recording(&mut self) {
        let recording = self.player.is_recording();
        if recording != self.recording_guide {
            self.start_recording_guide(recording);
        }
    }

    fn start_recording_guide(&mut self, on: bool) {
        self.recording_guide = on;
        let _ = self.guide_to.send(ToGuide::Recording(on));
        // 新しい番組表が届くまで前の物を見せない（全局の局が一瞬並ぶ）。
        if on {
            self.guide = Arc::new(zwcore::guide::Guide::new());
            self.guide_version += 1;
            self.listed_mark = None;
            self.panes.say(ToPanes::Guide(Arc::clone(&self.guide)));
        }
    }

    pub(super) fn tell_receivable(&mut self) {
        let now = self.receivable_now();
        if now != self.receivable {
            self.receivable = now;
            let _ = self
                .guide_to
                .send(ToGuide::Receivable(self.receivable.clone()));
            self.panes.say(ToPanes::Receivable(self.receivable.clone()));
            self.listed_mark = None;
        }
    }

    pub(super) fn refresh_listing(&mut self) {
        if !self.mini {
            return;
        }
        let now = self.guide.now_or_machine();
        let from = now - now.rem_euclid(3600) + i64::from(self.mini_shift) * 3600;
        let hours = self.mini_rows;
        let band = self.band();
        let watching = self.player.tables().watching;
        let asked = Listed {
            band,
            from,
            hours,
            watching,
            guide: self.guide_version,
        };
        if self.listed_mark == Some(asked) {
            return;
        }
        self.listed_mark = Some(asked);
        self.listing = Arc::new(ui::guide::epg::Listing {
            channels: guide::channels_of(
                self.stations(),
                &self.guide,
                band,
                from,
                hours,
                self.receivable.as_deref(),
                watching,
            ),
            from,
            hours,
            now: Some(now),
            band,
            bands: self.bands(),
            watching,
            jump: None,
            search: None,
        });
    }

    pub(super) fn take_fetched(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let Some(fetched) = live.fetched.as_ref() else {
            return;
        };
        // 全局の番組表は幾つかに分けて届く。送り終えたら受け口を閉じる。
        let mut parts = Vec::new();
        loop {
            match fetched.try_recv() {
                Ok(guide) => parts.push(guide),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    live.fetched = None;
                    break;
                }
            }
        }
        for guide in parts {
            self.learn_stations(&guide);
            // エンジンへは渡さない（追加する間止まり、音に穴が開く）。
            let _ = self.guide_to.send(ToGuide::Merge(Box::new(guide)));
        }
    }

    pub(super) fn take_merged(&mut self) {
        let Some(guide) = self.guide_back.try_iter().last() else {
            return;
        };
        self.guide = guide;
        self.guide_version += 1;
        self.stations_behind();
        self.write_logo_map();
        self.panes.say(ToPanes::Guide(Arc::clone(&self.guide)));
    }

    fn write_logo_map(&mut self) {
        let mut told = self.guide.logo_map();
        let written = self
            .logos_written
            .get_or_insert_with(|| crate::config::load_state().logos);
        // 録画の再生中は対応の無い局だけ（録画の頃のロゴ番号で上書きしない）。
        if self.recording_guide {
            told.retain(|key, _| !written.contains_key(key));
        }
        if told
            .iter()
            .all(|(key, logo)| written.get(key) == Some(logo))
        {
            return;
        }
        written.extend(told.clone());
        crate::config::update_state(move |state| state.logos.extend(told));
    }

    pub(super) fn take_data_tune(&mut self) {
        let Some((network, transport, service)) = zwcore::data::take_tune() else {
            return;
        };
        tracing::info!("データ放送から選局 {network:04x}.{transport:04x}.{service:04x}");
        self.tune(zwcore::guide::Key { network, service });
    }

    pub(super) fn tune(&mut self, key: zwcore::guide::Key) {
        if self.asked_key != Some(key)
            && let Some(done) = zwcore::record::stop()
        {
            self.toast(&super::capture::stopped(
                "チャンネルを変更したため、録画を停止しました。",
                &done,
            ));
        }
        if self.oneseg.is_some_and(|(_, to)| to != key) {
            self.oneseg = None;
        }
        if self.asked_key != Some(key) {
            self.tried.clear();
            self.tried_ways.clear();
        }
        let before = self.asked_key;
        self.asked_key = Some(key);
        self.reception
            .reset(Some(key.service), std::time::Instant::now());
        // 抜けようとしているとき（`leaving`）は録画の中で替えない（抜けられなくなる）。
        let within_recording =
            self.leaving.is_none()
                && self.player.is_recording()
                && self.player.tables().services.iter().any(|service| {
                    service.network == key.network && service.service == key.service
                });
        if (self.live.is_none() && self.player.is_recording()) || within_recording {
            let name = self
                .service(key)
                .map(|service| service.name.clone())
                .unwrap_or_else(|| format!("サービス {}", super::id_text(key.service)));
            tracing::info!("{name} に切り替える（録画を開き直す）");
            if self.player.switch_program(key.service) {
                self.switched_within(key, name);
                let _ = self.broadcast.try_send(ToBroadcast::Sought);
                self.told_clock = None;
                let _ = self.guide_to.send(ToGuide::Clock(None));
            }
            return;
        }
        // 再生モード中の選局は、抜けたときの局にするだけ。
        if self.player_mode && self.leaving.is_none() {
            self.asked_key = before;
            self.live_key = Some(key);
            let name = self
                .service(key)
                .map(|service| service.name.clone())
                .unwrap_or_else(|| format!("サービス {}", super::id_text(key.service)));
            self.toast(&format!("再生モード終了後 {name} を選局します。"));
            return;
        }
        // プロファイルで一番上のソースで開く（下へ移っても上に戻る）。
        let now = self.live.as_ref().and_then(|live| live.url.clone());
        if let Some((label, opening)) = self.best_for(key)
            && Some(opening.came()) != now
        {
            tracing::info!("{label} で開く（プロファイルの優先順）");
            let name = self
                .service(key)
                .map(|service| service.name.clone())
                .unwrap_or_else(|| format!("サービス {}", super::id_text(key.service)));
            self.leave_watching(&name);
            self.switching_to = Some(label);
            self.switch_source(opening);
            return;
        }
        let watching = self.player.tables().watching;
        let known = self.service(key).map(|service| service.name.clone());
        let Some(live) = self.live.as_mut() else {
            self.complain_not_tuned(
                "選局できるソースがありません。ソースの設定を確認してください。",
            );
            return;
        };
        // 一覧に無い局は、現在のストリームと同じ要求の仕方でキーだけ替えて要求する。拒否された経路は飛ばす。
        let tried_ways = &self.tried_ways;
        let listed = live
            .channels
            .iter()
            .filter(|channel| channel.key == Some(key))
            .find(|channel| !tried_ways.contains(&channel.how))
            .or_else(|| {
                live.channels
                    .iter()
                    .find(|channel| channel.key == Some(key))
            })
            .cloned();
        let same_stream = || {
            live.channels
                .iter()
                .find(|channel| live.supply.carries(channel))
                .or_else(|| {
                    let watching = watching?;
                    live.channels
                        .iter()
                        .find(|channel| channel.key == Some(watching))
                })
                .cloned()
                .map(|mut channel| {
                    channel.key = Some(key);
                    channel.name = known
                        .clone()
                        .unwrap_or_else(|| format!("サービス {}", super::id_text(key.service)));
                    channel
                })
        };
        let Some(channel) = listed.or_else(same_stream) else {
            let said = format!(
                "現在のソースでは該当チャンネルに選局できません。\nSID {}・NID {}",
                super::id_text(key.service),
                super::id_text(key.network)
            );
            self.complain_not_tuned(&said);
            return;
        };
        self.verifying = None;
        self.send_tune(channel, key);
    }

    pub(super) fn send_tune(
        &mut self,
        channel: zwcore::input::source::Channel,
        key: zwcore::guide::Key,
    ) {
        if self.live.is_none() {
            return;
        }
        // 再生スレッドで待つと映像も音声も止まるので、開くのは別のスレッド。
        let name = channel.name.clone();
        let named = self
            .service(key)
            .map(|service| service.name.clone())
            .unwrap_or_else(|| name.clone());
        // 要求したことを伝えてから切る（伝えないと録画の終わりと判断して deck ごと終わる）。
        // 前の局は応答を待たずに下ろす（休止中の局は応答が来ず残り続ける）。
        self.leave_watching(&named);
        let Some(live) = self.live.as_mut() else {
            return;
        };
        self.tuning_way = Some(channel.how.clone());
        live.supply.tune(channel, key);
        self.asked_tables = Instant::now() - TABLES_EVERY;
        let said = format!("{name} を選局しています");
        tracing::info!("{said}");
        self.said = Some(said);
    }

    /// 新しい局の応答を待たずに前の局を下ろす。録画から抜けようとしているときは録画を残す。
    fn leave_watching(&mut self, name: &str) {
        if !self.player.is_recording() {
            let flowing = self.player.read_bytes() > 0 || self.player.tables().watching.is_some();
            if flowing {
                self.retuned();
                self.player.await_nothing();
                self.told_clock = None;
                let _ = self.guide_to.send(ToGuide::Clock(None));
            }
            self.status.channel = Some(name.to_owned());
            self.status.sound = None;
            self.status.picture = None;
        }
        self.player.hold();
    }

    /// チャンネルスキャンや更新のために視聴中のチューナーを空ける・戻す（`None`）。生で見ているときだけ。
    pub(super) fn yield_tuner(&mut self, why: Option<&'static str>) {
        let Some(why) = why else {
            if let Some(key) = self.yielded.take() {
                tracing::info!("チューナーが戻ったため、視聴していた局を再選局する");
                self.tune(key);
            }
            return;
        };
        // Mirakurun の視聴を止めても coaxd のチューナーは空かない。
        let from_coaxd = self.live.as_ref().is_some_and(|live| {
            live.url
                .as_deref()
                .is_some_and(|url| url.starts_with("coaxd:"))
        });
        if !from_coaxd || self.player_mode || self.player.is_recording() {
            return;
        }
        // 戻る先が分からなくても手放す（相手はもう待っている）。
        let key = self.asked_key.or(self.player.tables().watching);
        // バイト列を捨てるだけではソースが接続を保持したまま。
        if let Some(live) = self.live.as_mut() {
            live.supply.release();
        }
        self.osd.hide();
        self.retuned();
        self.player.await_nothing();
        self.yielded = key;
        self.status.tuner = None;
        self.status.cnr = None;
        self.toast(why);
        tracing::info!("{why}（視聴中のチューナーを閉じた）");
    }

    pub(super) fn scanned(&mut self, services: Vec<zwcore::guide::Service>) {
        self.catalog_due = true;
        if !services.is_empty() {
            let mut seeded = zwcore::guide::Guide::new();
            for service in services {
                seeded.tell_service(service);
            }
            self.learn_stations(&seeded);
            let _ = self.guide_to.send(ToGuide::Merge(Box::new(seeded)));
        }
        self.remap_coaxd();
        self.roster_seeded = false;
        self.listed_mark = None;
    }

    /// 一覧に無い選べる局を TV として入れる。ある局には触れない。ソースごとに一度。
    pub(super) fn seed_roster(&mut self) {
        if self.roster_seeded {
            return;
        }
        let Some(live) = self.live.as_ref() else {
            return;
        };
        self.roster_seeded = true;
        let own: &[zwcore::input::source::Channel] = if live
            .url
            .as_deref()
            .is_some_and(|url| url.starts_with("coaxd:"))
        {
            &live.channels
        } else {
            &[]
        };
        let mut seeded = zwcore::guide::Guide::new();
        let mut count = 0usize;
        let mut told = std::collections::BTreeSet::new();
        for channel in own
            .iter()
            .chain(self.catalog.iter().flat_map(|offer| &offer.channels))
        {
            let Some(key) = channel.key else {
                continue;
            };
            if self.service(key).is_some() || channel.name.is_empty() || !told.insert(key) {
                continue;
            }
            seeded.tell_service(zwcore::guide::Service {
                network: key.network,
                service: key.service,
                name: channel.name.clone(),
                kind: 0x01,
                remote_key: channel.order.and_then(|order| u8::try_from(order).ok()),
                ..zwcore::guide::Service::default()
            });
            count += 1;
        }
        if count > 0 {
            tracing::info!("番組表に無い {count} 局をソースの一覧から追加した");
            self.learn_stations(&seeded);
            let _ = self.guide_to.send(ToGuide::Merge(Box::new(seeded)));
        }
    }

    /// ワンセグへ切り替える（`playback.oneseg`）。PAT にワンセグ（PMT 0x1FC8〜0x1FCF）があるときだけ、
    /// 同じ局では一度だけ。
    pub(super) fn watch_oneseg(&mut self) {
        let way = self.choices.knobs.oneseg;
        let Some(watching) = self.player.tables().watching else {
            return;
        };
        if self.player.unreadable() && self.said_unreadable != Some(watching) {
            self.said_unreadable = Some(watching);
            if way == crate::choice::OneSeg::Never {
                self.toast("映像・音声を再生できません。");
            }
        }
        if way == crate::choice::OneSeg::Never {
            return;
        }
        let in_stream = self
            .player
            .tables()
            .programs
            .iter()
            .find(|program| program.one_seg())
            .map(|program| program.number);
        let Some(seg) = in_stream else {
            if self.player.unreadable() && self.oneseg.is_none_or(|(from, _)| from != watching) {
                self.oneseg = Some((watching, watching));
                self.toast("映像・音声を再生できません。");
            }
            return;
        };
        if seg == watching.service {
            return;
        }
        let wanted = match way {
            crate::choice::OneSeg::Always => true,
            crate::choice::OneSeg::Fallback => self.player.unreadable(),
            crate::choice::OneSeg::Never => false,
        };
        if !wanted || self.oneseg.is_some_and(|(from, _)| from == watching) {
            return;
        }
        let to = zwcore::guide::Key {
            network: watching.network,
            service: seg,
        };
        self.oneseg = Some((watching, to));
        if way != crate::choice::OneSeg::Always {
            self.toast("ワンセグに切り替えました。");
        }
        tracing::info!(
            "ワンセグへ: サービス {} → {}（{:?}）",
            watching.service,
            seg,
            way
        );
        self.tune(to);
    }

    /// データ放送は選局で放送の識別情報を破棄するので、同じ局へ選び直しても通知するよう記録も消す。
    fn retuned(&mut self) {
        zwcore::data::retuned();
        let _ = self.broadcast.try_send(ToBroadcast::Retuned);
        self.told_broadcast = None;
    }

    fn switched_within(&mut self, key: zwcore::guide::Key, name: String) {
        self.watched(key);
        self.retuned();
        self.band = Some(Band::of(key.network));
        self.status.channel = self
            .service(key)
            .map(|service| service.name.clone())
            .or(Some(name));
        self.status.sound = None;
        self.said = None;
        self.seeking = false;
        self.seek_settling = false;
        self.osd
            .show_on(self.osd_prefs, ui::overlay::osd::Why::Channel);
        self.osded = None;
    }

    /// 要求した選局の応答が返らない。スレッドは殺さない（遅れた応答はチケット違いで捨てる）。
    pub(super) fn give_up_tuning(&mut self) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        let Some(tuning) = live.supply.tuning() else {
            return;
        };
        if tuning.began.elapsed() < TUNE_PATIENCE {
            return;
        }
        let said = format!(
            "チャンネルを変更できません。\n応答がありません。（{}）",
            tuning.name
        );
        if let Some(live) = self.live.as_mut() {
            live.supply.settle();
        }
        self.said = None;
        self.complain_not_tuned(&said);
    }

    pub(super) fn tuned(&mut self, answer: Answer) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        match answer {
            Answer::State { tuner, cnr } => {
                self.status.tuner = tuner;
                self.status.cnr = cnr;
            }
            Answer::Refused { ticket, name, why } => {
                if !live.supply.is_current(ticket) {
                    return;
                }
                live.supply.settle();
                if self.verify_refused() {
                    tracing::warn!("  選局を拒否された（{}）", said_of(&why));
                    return;
                }
                // 要求は取り下げない（前のストリームは切ってあり、取り下げると deck が終わる）。
                if let Some(key) = self.asked_key {
                    if let Some(way) = self.tuning_way.take()
                        && !self.tried_ways.contains(&way)
                    {
                        self.tried_ways.push(way);
                    }
                    let another = self.live.as_ref().is_some_and(|live| {
                        live.channels.iter().any(|channel| {
                            channel.key == Some(key) && !self.tried_ways.contains(&channel.how)
                        })
                    });
                    if another {
                        tracing::warn!(
                            "{name} を選局できない（{}）。別の経路で再試行する",
                            said_of(&why)
                        );
                        self.tune(key);
                        return;
                    }
                    if let Some(label) = self.live_label()
                        && !self.tried.contains(&label)
                    {
                        self.tried.push(label);
                    }
                    if let Some((label, opening)) = self.offer_for(key) {
                        tracing::warn!(
                            "{name} を選局できない（{}）。{label} で開き直す",
                            said_of(&why)
                        );
                        self.switching_to = Some(label);
                        self.switch_source(opening);
                        return;
                    }
                }
                tracing::warn!("{name} を選局できない: {}", said_of(&why));
                if let Some(code) = zwcore::input::source::coaxd::refusal_code(&why.to_string()) {
                    self.reception.refused(&code);
                } else if why
                    .to_string()
                    .contains(zwcore::input::source::mirakurun::NOTHING_CAME)
                {
                    self.reception.refused("nothing_came");
                }
                let said = refused(&name, &why);
                self.said = None;
                self.complain_not_tuned(&said);
            }
            Answer::Opened {
                ticket,
                name,
                key,
                how,
                bytes,
                container,
            } => {
                if !live.supply.is_current(ticket) {
                    return;
                }
                live.supply.settle();
                let asked_how = how.clone();
                live.supply.opened(how);
                tracing::info!("{name} を選局した");
                self.watched(key);
                // データ放送のスレッドは通信を同期で待つので、先に止めて通知が届くようにする。
                self.retuned();
                self.player.leave_recording();
                self.player_mode = false;
                self.leaving = None;
                self.profile_pending = false;
                if !self.player.retune(bytes, container, Some(key.service)) {
                    return;
                }
                self.player.start();
                self.verify_opened(key, &asked_how, &name, container);
                self.told_clock = None;
                let _ = self.guide_to.send(ToGuide::Clock(None));
                // ためていた番組表は渡さない（追加する間読めず音に穴が開く）。
                self.band = Some(Band::of(key.network));
                self.status.channel = self
                    .service(key)
                    .map(|service| service.name.clone())
                    .or(Some(name));
                self.status.sound = None;
                self.status.picture = None;
                self.said = None;
            }
        }
    }

    pub(super) fn play_file(&mut self, path: &std::path::Path) {
        let Some(text) = path.to_str() else {
            self.complain("ファイル名を読み取れません。");
            return;
        };
        self.stop_recording_for_switch();
        if let Some(live) = self.live.as_mut() {
            live.supply.release();
        }
        self.leaving = None;
        if !self.player_mode {
            self.live_key = self.asked_key.or(self.player.tables().watching);
        }
        self.retuned();
        if let Err(why) = self.player.play_file(text) {
            self.complain(&why);
            return;
        }
        let shown = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(text)
            .to_owned();
        tracing::info!("再生モード: {shown}");
        self.start_recording_guide(true);
        self.player_mode = true;
        self.osd.hide();
        self.osded = None;
        self.told_clock = None;
        let _ = self.guide_to.send(ToGuide::Clock(None));
        self.band = None;
        self.status.channel = Some(shown);
        self.status.tuner = None;
        self.status.cnr = None;
        self.status.sound = None;
        self.status.picture = None;
        self.said = None;
    }

    pub(super) fn toggle_player_mode(&mut self) {
        self.stop_recording_for_switch();
        if self.player_mode {
            self.back_to_live();
            return;
        }
        if let Some(live) = self.live.as_mut() {
            live.supply.release();
        }
        self.osd.hide();
        self.osded = None;
        self.retuned();
        // 起動時に開いた局は `tune` を通っていない。
        self.live_key = self.asked_key.or(self.player.tables().watching);
        self.player.leave_recording();
        self.player.await_nothing();
        self.player_mode = true;
        tracing::info!("再生モード（録画ファイルを開くか、ウィンドウへドロップ）");
        self.told_clock = None;
        let _ = self.guide_to.send(ToGuide::Clock(None));
        self.band = None;
        self.status.channel = None;
        self.status.tuner = None;
        self.status.cnr = None;
        self.status.sound = None;
        self.status.picture = None;
        self.said = None;
    }

    pub(super) fn back_to_live(&mut self) {
        if self.live.is_none() {
            self.toast(
                "放送ストリームに切り替えられません。\n録画ファイルの再生から起動しています。",
            );
            return;
        }
        if self.profile_pending {
            let file = crate::config::load_plain();
            let profile = crate::config::load_state().profile;
            let now = self.live.as_ref().and_then(|live| live.url.clone());
            if let Some(wanted) = super::first_source(&file, &profile)
                && Some(wanted.came()) != now
            {
                if self.leaving.is_none() {
                    self.leaving = Some(self.asked_key);
                }
                self.asked_key = self.live_key.or(self.asked_key);
                self.switch_source(wanted);
                return;
            }
        }
        let Some(key) = self.live_key.or(self.asked_key) else {
            self.toast("選局できるチャンネルがありません。");
            return;
        };
        // 再生モードを抜けるフラグを先に立てる（立っていないと再生モード中の選局として扱われる）。
        if self.leaving.is_none() {
            self.leaving = Some(self.asked_key);
        }
        self.tune(key);
    }

    pub(super) fn stay_in_player_mode(&mut self) -> bool {
        let Some(before) = self.leaving.take() else {
            return false;
        };
        self.asked_key = before;
        self.player.unhold();
        true
    }

    pub(super) fn complain_not_tuned(&mut self, said: &str) {
        if self.stay_in_player_mode() {
            self.complain(&format!("{said}\n再生モードのままです。"));
        } else {
            self.complain(said);
        }
    }

    pub(super) fn toast(&mut self, text: &str) {
        self.show_toast(text, 1);
    }

    pub(super) fn complain(&mut self, text: &str) {
        let repeated = self
            .toast
            .as_ref()
            .map_or(1, |before| super::repeat_of(before, text));
        self.show_toast(text, repeated);
    }

    fn show_toast(&mut self, text: &str, repeated: u32) {
        if let Some(toast) = self.toast.take_if(|toast| {
            toast.shown.progress.is_some() && toast.at.elapsed().as_secs_f32() < toast.hold
        }) {
            self.toast_behind = Some(toast);
        }
        self.toasts += 1;
        self.toast = Some(super::Toast::of(text, repeated, self.toasts));
    }

    pub(super) fn notice(&mut self, notice: zwsdk::ui::Notice) {
        let programme = self
            .player
            .tables()
            .now
            .as_ref()
            .map(|now| (now.network, now.service, now.event));
        let current = self.toast.as_ref().and_then(|toast| {
            (toast.at.elapsed().as_secs_f32() < toast.hold)
                .then(|| (toast.said.as_str(), toast.shown.progress.is_some()))
        });
        // 取り下げた準備中が、ほかのトーストの後ろに回っていれば戻さない。
        if notice.seconds == Some(0.0) {
            self.toast_behind
                .take_if(|behind| behind.said == notice.title);
        }
        if self.noticed.heed(programme, &notice, current) == super::plugging::Heed::Skip {
            return;
        }
        self.toast_behind = None;
        // 準備がすぐ済んでも [`super::TOAST_LEAST`] は残し、進み具合を満たしてから薄れさせる。
        if notice.seconds == Some(0.0)
            && let Some(toast) = self.toast.as_mut()
        {
            toast.hold = toast.at.elapsed().as_secs_f32().max(super::TOAST_LEAST);
            if toast.shown.progress.is_some() {
                toast.shown.progress = Some(1.0);
            }
            self.toasts += 1;
            toast.serial = self.toasts;
            return;
        }
        let lines = notice.details.len() as f32;
        let hold = notice.seconds.unwrap_or(if notice.progress.is_some() {
            super::TOAST_PROGRESS_HOLD
        } else {
            super::TOAST_FOR + lines * super::TOAST_PER_LINE
        });
        tracing::info!(
            "{}",
            std::iter::once(notice.title.as_str())
                .chain(notice.details.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" / ")
        );
        self.toasts += 1;
        self.toast = Some(super::Toast {
            said: notice.title.clone(),
            shown: crate::draw::scene::Toasted {
                title: notice.title,
                details: notice.details,
                tone: notice.tone,
                progress: notice.progress,
            },
            at: std::time::Instant::now(),
            serial: self.toasts,
            hold,
        });
    }

    pub(super) fn tell(&mut self, said: &str) {
        tracing::info!("{said}");
        self.said = Some(said.to_owned());
        self.osd.tell(self.osd_prefs);
    }

    pub(super) fn step_channel(&mut self, forward: bool) {
        let Some(watching) = self.player.tables().watching else {
            return;
        };
        let from = self.asked_key.unwrap_or(watching);
        let now = self.guide.now_or_machine();
        let next = if self.live.is_some() {
            let band = Band::of(watching.network);
            let only = self.receivable.as_deref();
            let services: Vec<&zwcore::guide::Service> = self
                .guide
                .channels()
                .filter(|service| service.band() == band)
                .filter(|service| only.is_none_or(|only| only.contains(&service.key())))
                .collect();
            super::stepping::next(&services, from, forward, self.stepping, |key| {
                self.guide.at(key, now)
            })
        } else {
            let tables = self.player.tables();
            let services: Vec<&zwcore::guide::Service> = tables
                .services
                .iter()
                .filter(|service| service.watchable())
                .collect();
            super::stepping::next(&services, from, forward, self.stepping, |key| {
                self.guide.at(key, now)
            })
        };
        if let Some(next) = next {
            self.tune(next);
        }
    }
}

impl Stage {
    /// 同じ番号の局が複数あれば押すたびに次へ回る（同じ地域の局を優先、`zwcore::region`）。
    pub(super) fn tune_remote(&mut self, band: Band, number: u8) {
        let mut candidates: Vec<zwcore::guide::Key> = if self.live.is_some() {
            let only = self.receivable.as_deref();
            let mut services: Vec<&zwcore::guide::Service> = self
                .guide
                .channels()
                .filter(|service| service.band() == band)
                .filter(|service| only.is_none_or(|only| only.contains(&service.key())))
                .filter(|service| service.remote_key == Some(number))
                .collect();
            services.sort_by_key(|service| service.order());
            services.into_iter().map(|service| service.key()).collect()
        } else {
            let tables = self.player.tables();
            let mut services: Vec<&zwcore::guide::Service> = tables
                .services
                .iter()
                .filter(|service| service.watchable())
                .filter(|service| service.remote_key == Some(number))
                .collect();
            services.sort_by_key(|service| service.order());
            services.into_iter().map(|service| service.key()).collect()
        };
        candidates.dedup();
        if candidates.is_empty() {
            // 先頭は E204 と同じ文言（ARIB TR-B14 表 6-3）。
            self.toast(&format!(
                "このチャンネルはありません。（E204）\n{} の {number} にはチャンネルがありません。",
                band.name()
            ));
            return;
        }
        let from = self.asked_key.or(self.player.tables().watching);
        let at = from.and_then(|from| candidates.iter().position(|key| *key == from));
        let next = at.map_or(0, |at| (at + 1) % candidates.len());
        self.tune(candidates[next]);
    }

    pub(super) fn choose_band(&mut self, band: Band) {
        self.band = Some(band);
    }

    /// 形が分かったら OSD を出す（分からない間はウィンドウの幅で出てしまう）。途中の変化では出さない。
    pub(super) fn settle_format(&mut self) {
        let now = self
            .player
            .picture_format()
            .map(|format| shown_size(&format))
            .or_else(|| (self.player.audio_only() || self.player.data_only()).then_some((0, 0)));
        if now == self.formatted {
            return;
        }
        let was = self.formatted;
        self.formatted = now;
        if now.is_some() {
            if self.seeking {
                self.seeking = false;
            } else if was.is_none() {
                self.osd
                    .show_on(self.osd_prefs, ui::overlay::osd::Why::Channel);
            }
            self.osded = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zwcore::input::source::coaxd::DaemonTuner;

    fn tuner(id: &str, model: &str, deliveries: &[&str]) -> DaemonTuner {
        DaemonTuner {
            id: id.to_owned(),
            deliveries: deliveries.iter().map(|one| (*one).to_owned()).collect(),
            leased: false,
            model: model.to_owned(),
            name: String::new(),
        }
    }

    #[test]
    fn a_profile_keeps_the_routes_its_tuners_receive() {
        let file = crate::config::File::default();
        let pt3 = tuner("dvb:adapter0.frontend0:0", "PT3", &["ISDBS"]);
        let s270 = tuner("s270:p1-13.3:0", "MyGica S270", &["ISDBT", "DVBT"]);
        let tbs = tuner(
            "dvb:adapter4.frontend0:0",
            "TBS 6205SE",
            &["ISDBC", "DVBC/ANNEX_A"],
        );
        let listed = [&pt3, &s270, &tbs];
        let lines = |patterns: &[&str]| {
            let patterns: Vec<String> = patterns.iter().map(|one| (*one).to_owned()).collect();
            lines_for_tuners(&file, &patterns, &listed)
                .map(|lines| lines.into_iter().collect::<Vec<_>>())
        };
        assert_eq!(lines(&[]), None, "決めていなければ全部");
        assert_eq!(lines(&["model:PT3*"]).unwrap(), ["isdbs"]);
        assert_eq!(lines(&["s270:*"]).unwrap(), ["isdbt"]);
        assert_eq!(lines(&["model:TBS 6205SE*"]).unwrap(), ["isdbc"]);
        assert_eq!(
            lines(&["model:PT3*", "s270:*"]).unwrap(),
            ["isdbs", "isdbt"],
            "プロファイルのチューナーの順（方式の順ではない）"
        );
        let unsaid = tuner("bda:1", "BDA", &[]);
        assert!(
            lines_for_tuners(&file, &[String::from("bda:1")], &[&unsaid])
                .unwrap()
                .is_empty(),
            "方式の分からないチューナーは、どの経路にも入れない"
        );
        assert!(lines(&["nobody:*"]).unwrap().is_empty());
    }
}
