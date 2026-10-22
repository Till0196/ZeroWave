// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 局一覧。ソースの一覧・放送の SDT と NIT・Mirakurun の番組表の局をここで合わせ、番組表のスレッドを待たずに並べる。
//! 番組表のスレッドにしか無い局（ファイルから読んだ局）は、届いたら足す。

use std::sync::Arc;

use zwcore::guide::{Guide, Key, Logo, Prefs, Service};

use super::Stage;
use crate::threads::panes::ToPanes;

#[derive(Debug, Default)]
pub(super) struct Stations {
    /// 設定の名前・リモコン番号は当てていない。
    own: Guide,
    prefs: Prefs,
    shown: Arc<Guide>,
    due: bool,
}

impl Stations {
    pub(super) fn shown(&self) -> &Guide {
        &self.shown
    }
}

impl Stage {
    pub(super) fn learn_stations(&mut self, from: &Guide) {
        self.stations.own.merge(from.stations());
        self.stations.due = true;
    }

    pub(super) fn station_prefs(&mut self, prefs: Prefs) {
        if self.stations.prefs != prefs {
            self.stations.prefs = prefs;
            self.stations.due = true;
        }
    }

    pub(super) fn stations_behind(&mut self) {
        self.stations.due = true;
    }

    /// 録画の再生中は録画の局だけ（番組表のスレッドが録画から組み立てる）。
    pub(super) fn stations(&self) -> &Guide {
        if self.recording_guide {
            &self.guide
        } else {
            self.stations.shown()
        }
    }

    pub(super) fn service(&self, key: Key) -> Option<&Service> {
        service_in(self.stations(), &self.guide, key)
    }

    pub(super) fn logo_of(&self, key: Key) -> Option<&Arc<Logo>> {
        logo_in(self.stations(), &self.guide, key)
    }

    pub(super) fn refresh_stations(&mut self) {
        if !self.stations.due {
            return;
        }
        self.stations.due = false;
        let mut shown = self.stations.own.clone();
        for service in self.guide.services() {
            if shown.service(service.key()).is_none() {
                shown.tell_service(service.clone());
            }
        }
        shown.lend_logos(&self.guide);
        if let Some(map) = &self.logos_written {
            shown.fill_logos(map);
        }
        shown.apply_prefs(&self.stations.prefs);
        self.stations.shown = Arc::new(shown);
        self.guide_version += 1;
        self.listed_mark = None;
        self.panes
            .say(ToPanes::Stations(Arc::clone(&self.stations.shown)));
    }
}

pub(super) fn service_in<'a>(
    stations: &'a Guide,
    guide: &'a Guide,
    key: Key,
) -> Option<&'a Service> {
    stations.service(key).or_else(|| guide.service(key))
}

pub(super) fn logo_in<'a>(
    stations: &'a Guide,
    guide: &'a Guide,
    key: Key,
) -> Option<&'a Arc<Logo>> {
    stations.logo_of(key).or_else(|| guide.logo_of(key))
}
