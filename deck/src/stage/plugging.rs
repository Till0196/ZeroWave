// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインに現在の状態（番組と映像の形式）を 0.5 秒ごとに渡し、通知（`zw_ui_v1`）をトーストにする。

use super::*;

const PLUGIN_EVERY: Duration = Duration::from_millis(500);
const LATE_WINDOW: Duration = Duration::from_secs(3);

impl Stage {
    pub(super) fn refresh_plugins(&mut self) {
        for notice in crate::config::plugins::take_notices() {
            self.notice(notice);
        }
        if self.plugin_checked.elapsed() < PLUGIN_EVERY {
            return;
        }
        self.plugin_checked = Instant::now();
        self.tell_channels();
        self.tell_now();
        let video = self.player.video_stats();
        let now = Instant::now();
        if self
            .plugin_late
            .back()
            .is_some_and(|(_, late)| *late > video.late)
        {
            self.plugin_late.clear();
        }
        self.plugin_late.push_back((now, video.late));
        while self
            .plugin_late
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > LATE_WINDOW)
        {
            self.plugin_late.pop_front();
        }
        let late_recent = self
            .plugin_late
            .front()
            .map_or(0, |(_, late)| video.late - late);
        let shared = crate::plugin_ui::Shared {
            program: self.plugin_program(),
            format: self.plugin_format(),
            report: self.player.filter_report(),
            drawn: video.drawn,
            late: video.late,
            late_recent,
        };
        let latency = shared
            .report
            .as_ref()
            .map_or(0.0, |report| report.latency_ms);
        if (latency - self.filter_latency).abs() > 0.5 {
            self.filter_latency = latency;
            self.say_video_latency();
        }
        // レンダラーは作り直すと忘れるので毎回渡す。
        self.player.set_filter_program(shared.program.clone());
        let menus = self.plugin_ui.menus(&shared);
        if menus != self.plugin_menus {
            self.plugin_menus = menus;
            self.plugin_menus_serial += 1;
        }
        if shared != self.plugin_shared {
            self.plugin_shared = shared.clone();
            self.plugin_shared_told = Arc::new(shared.clone());
            self.panes.say(ToPanes::Plugins(Box::new(shared)));
        }
    }

    /// いま見ている番組。MPEG-2 でピクチャー構造が決まらないうちも `None`。
    fn plugin_program(&self) -> Option<zwsdk::video::Program> {
        if self.player.structure_pending() {
            return None;
        }
        let tables = self.player.tables();
        let programme = tables.now.as_ref()?;
        let mut program =
            crate::plugin_ui::program_of(&self.guide, programme, self.player.is_recording());
        program.structure = match self.player.coded_structure() {
            Some(zwcore::player::CodedStructure::Frame) => zwsdk::video::Structure::Frame,
            Some(zwcore::player::CodedStructure::Field) => zwsdk::video::Structure::Field,
            None => zwsdk::video::Structure::Unknown,
        };
        if let Some(key) = tables.watching {
            program.network = key.network;
            program.service = key.service;
            program.band = crate::plugin_ui::band_name(key.network);
            if let Some(service) = self.service(key) {
                program.channel = service.name.clone();
            }
        }
        Some(program)
    }

    fn plugin_format(&self) -> Option<zwsdk::video::Format> {
        let format = self.player.picture_format()?;
        Some(zwsdk::video::Format {
            width: format.width,
            height: format.height,
            depth: format.depth,
            interlaced: !format.progressive,
            aspect: [format.aspect.0, format.aspect.1],
            transfer: String::new(),
        })
    }

    pub(super) fn plugin_pick(&mut self, stem: &str, id: &str) {
        let shared = self.plugin_shared.clone();
        self.plugin_ui.pick(stem, id, &shared);
        self.plugin_checked = Instant::now() - PLUGIN_EVERY;
    }
}

#[derive(Debug, Default)]
pub(super) struct Noticed {
    programme: Option<(u16, u16, u16)>,
    shown: Vec<(String, Vec<String>)>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Heed {
    Show,
    Skip,
}

impl Noticed {
    /// `programme` が分からない（録画の位置を動かした直後など）ときは、前の番組のままとみる。
    pub(super) fn heed(
        &mut self,
        programme: Option<(u16, u16, u16)>,
        notice: &zwsdk::ui::Notice,
        current: Option<(&str, bool)>,
    ) -> Heed {
        if programme.is_some() && programme != self.programme {
            self.programme = programme;
            self.shown.clear();
        }
        if notice.seconds == Some(0.0) {
            return if current.is_some_and(|(title, _)| title == notice.title) {
                Heed::Show
            } else {
                Heed::Skip
            };
        }
        let seen = (notice.title.clone(), notice.details.clone());
        if self.shown.contains(&seen) {
            // 出ているのが準備中（進み具合つき）なら、それを片付けるために出す。
            return if current.is_some_and(|(title, progress)| progress && title != notice.title) {
                Heed::Show
            } else {
                Heed::Skip
            };
        }
        // 番組が替わらないまま違う通知が続いてもため続けない。
        if self.shown.len() >= 64 {
            self.shown.remove(0);
        }
        self.shown.push(seen);
        Heed::Show
    }
}

impl Stage {
    fn tell_channels(&self) {
        let only = self.receivable.as_deref();
        let mut services: Vec<&zwcore::guide::Service> = self
            .guide
            .channels()
            .filter(|service| only.is_none_or(|only| only.contains(&service.key())))
            .collect();
        services.sort_by_key(|service| (service.band(), service.order()));
        let channels = services
            .into_iter()
            .map(|service| zwsdk::control::Channel {
                band: service.band().name(),
                remote: service.remote_key,
                name: service.name.clone(),
                nid: service.network,
                sid: service.service,
            })
            .collect();
        crate::config::plugins::set_channels(zwsdk::control::Channels {
            channels,
            watching: self
                .player
                .tables()
                .watching
                .map(|key| (key.network, key.service)),
            actions: Vec::new(),
        });
    }

    /// リモコン API の `now` と `logo`。局名は番組表、無ければストリームの SDT から。
    fn tell_now(&self) {
        let tables = self.player.tables();
        let key = tables.watching;
        let station = key.map(|key| {
            let service = self.service(key);
            let sdt = tables
                .neighbours
                .iter()
                .find(|told| told.network == key.network && told.service == key.service);
            zwsdk::control::Station {
                network: key.network,
                transport: sdt
                    .map(|told| told.transport)
                    .or_else(|| tables.identity.map(|identity| identity.transport))
                    .unwrap_or_default(),
                service: key.service,
                name: service
                    .map(|service| service.name.clone())
                    .filter(|name| !name.is_empty())
                    .or_else(|| sdt.map(|told| told.name.clone()))
                    .unwrap_or_default(),
                provider: sdt.map(|told| told.provider.clone()).unwrap_or_default(),
                band: crate::plugin_ui::band_name(key.network),
                remote: service.and_then(|service| service.remote_key),
                logo_id: service.and_then(|service| service.logo_id),
            }
        });
        let now = zwsdk::control::Now {
            station,
            event: tables.now.as_ref().map(event_of),
            next: tables.next.as_ref().map(event_of),
            recording: self.player.is_recording(),
        };
        let logos = key.map(|key| {
            let logos = zwcore::guide::logo::PREFERRED
                .iter()
                .filter_map(|logo_type| self.guide.logo(key, *logo_type).cloned())
                .collect();
            ((key.network, key.service), logos)
        });
        crate::config::plugins::set_now(now, logos);
    }
}

fn event_of(programme: &zwcore::guide::Programme) -> zwsdk::control::Event {
    // 番組表の時刻は壁時計をゾーン無しで秒にしたもの。
    let unix = |moment: i64| moment - i64::from(zwcore::guide::JST_OFFSET);
    zwsdk::control::Event {
        id: programme.event,
        name: programme.title.clone(),
        text: programme.detail.clone(),
        extended: programme.extended.clone(),
        genres: programme
            .genres
            .iter()
            .map(|genre| (genre.large, genre.middle))
            .collect(),
        start: programme.start.map(unix),
        duration: programme.seconds,
    }
}

#[cfg(test)]
mod noticed_tests {
    use super::*;

    fn notice(title: &str, seconds: Option<f32>) -> zwsdk::ui::Notice {
        zwsdk::ui::Notice {
            title: title.to_owned(),
            seconds,
            ..zwsdk::ui::Notice::default()
        }
    }

    const A: Option<(u16, u16, u16)> = Some((1, 2, 3));
    const B: Option<(u16, u16, u16)> = Some((1, 2, 4));

    #[test]
    fn the_same_notice_is_shown_once_per_programme() {
        let mut noticed = Noticed::default();
        let preset = notice("映像フィルター：見本", None);
        assert_eq!(noticed.heed(A, &preset, None), Heed::Show);
        assert_eq!(noticed.heed(A, &preset, None), Heed::Skip);
        assert_eq!(noticed.heed(None, &preset, None), Heed::Skip);
        assert_eq!(noticed.heed(B, &preset, None), Heed::Show);
    }

    /// 出し直さなかった準備中を消すと、別のトーストが消える。
    #[test]
    fn a_withdrawal_only_clears_its_own_toast() {
        let mut noticed = Noticed::default();
        let gone = notice("準備しています", Some(0.0));
        assert_eq!(noticed.heed(A, &gone, Some(("音量 50", false))), Heed::Skip);
        assert_eq!(
            noticed.heed(A, &gone, Some(("準備しています", true))),
            Heed::Show
        );
    }

    #[test]
    fn a_repeat_replaces_a_progress_toast() {
        let mut noticed = Noticed::default();
        let preset = notice("映像フィルター：見本", None);
        assert_eq!(noticed.heed(A, &preset, None), Heed::Show);
        assert_eq!(
            noticed.heed(A, &preset, Some(("準備しています", true))),
            Heed::Show
        );
        assert_eq!(
            noticed.heed(A, &preset, Some(("音量 50", false))),
            Heed::Skip
        );
    }
}
