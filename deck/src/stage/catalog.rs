// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! カタログ：プロファイルの全ソースの局を、出したソースに紐付けて持つ。選んだ局がいまのソースに
//! 無ければ、その局を出す最初のソースへ切り替える。同じ Mirakurun どうしは一つに合わせる。

use super::*;
use zwcore::input::source::Source as _;

/// 局一覧を先に、ほかの Mirakurun の全局の番組表は取れたものから送る（番組表は重く、局一覧を待たせない）。
#[derive(Debug)]
pub(crate) enum Built {
    Offers(Vec<Offer>),
    Guide(zwcore::guide::Guide),
}

#[derive(Debug)]
pub(crate) struct Offer {
    pub label: String,
    pub opening: Opening,
    pub channels: Vec<zwcore::input::source::Channel>,
}

fn by_name(by: zwcore::input::source::mirakurun::By) -> &'static str {
    use zwcore::input::source::mirakurun::By;
    match by {
        By::Auto => "auto",
        By::Channel => "channel",
        By::Service => "service",
    }
}

pub(crate) fn lineup_channels(
    lineup: &crate::config::state::Lineup,
) -> Vec<zwcore::input::source::Channel> {
    lineup
        .channels
        .iter()
        .map(|listed| zwcore::input::source::Channel {
            name: listed.name.clone(),
            key: Some(zwcore::guide::Key {
                network: listed.network,
                service: listed.service,
            }),
            transport: listed.transport,
            order: listed.remote,
            how: listed.how.clone(),
            container: if listed.tlv {
                zwcore::filters::Container::Tlv
            } else {
                zwcore::filters::Container::Ts
            },
        })
        .collect()
}

pub(crate) fn lineup_of(
    url: &str,
    by: &str,
    channels: &[zwcore::input::source::Channel],
) -> crate::config::state::Lineup {
    crate::config::state::Lineup {
        url: url.to_owned(),
        by: by.to_owned(),
        fetched: crate::config::state::now(),
        channels: channels
            .iter()
            .filter_map(|channel| {
                let key = channel.key?;
                Some(crate::config::state::Listed {
                    network: key.network,
                    service: key.service,
                    name: channel.name.clone(),
                    transport: channel.transport,
                    remote: channel.order,
                    how: channel.how.clone(),
                    tlv: channel.container == zwcore::filters::Container::Tlv,
                })
            })
            .collect(),
    }
}

fn at_hand(
    state: &crate::config::state::State,
    label: &str,
    opening: &Opening,
) -> Vec<zwcore::input::source::Channel> {
    match opening {
        Opening::Mirakurun { url, by } => state
            .services
            .get(label)
            .filter(|lineup| lineup.url == *url && lineup.by == by_name(*by))
            .map(lineup_channels)
            .unwrap_or_default(),
        Opening::Coaxd {
            tuners, profile, ..
        } if tuners.is_empty() => tuning::mapped_channels(None, profile),
        Opening::Coaxd { .. } => Vec::new(),
    }
}

fn asked(label: &str, opening: &Opening) -> Vec<zwcore::input::source::Channel> {
    match opening {
        Opening::Mirakurun { url, by } => {
            let fetched = zwcore::input::source::mirakurun::Mirakurun::at(url)
                .map_err(|why| why.to_string())
                .and_then(|mut source| {
                    source.set_by(*by);
                    source.channels().map_err(|why| why.to_string())
                });
            match fetched {
                Ok(channels) => {
                    let lineup = lineup_of(url, by_name(*by), &channels);
                    let label = label.to_owned();
                    crate::config::update_state(move |state| {
                        state.services.insert(label, lineup);
                    });
                    channels
                }
                Err(why) => {
                    tracing::warn!(
                        "{label}: Mirakurun からサービス一覧を取得できない（キャッシュを使う）: {why}"
                    );
                    at_hand(&crate::config::load_state(), label, opening)
                }
            }
        }
        Opening::Coaxd {
            endpoint,
            priority,
            tuners,
            profile,
        } => tuning::mapped_for(endpoint, *priority, tuners, profile),
    }
}

/// 設定にある Mirakurun の全部へ、バックグラウンドでサービス一覧を取得してキャッシュする。`skip` は重ねて取得しない物。
pub(super) fn refresh_mirakurun(file: &crate::config::File, skip: &[String]) {
    let wanted: Vec<(String, Opening)> = file
        .sources
        .iter()
        .filter(|source| source.kind == "mirakurun" && !source.url.is_empty())
        .filter(|source| !skip.contains(&source.label))
        .map(|source| {
            (
                source.label.clone(),
                Opening::Mirakurun {
                    url: source.url.clone(),
                    by: zwcore::input::source::mirakurun::By::parse(&source.by)
                        .unwrap_or(zwcore::input::source::mirakurun::By::Auto),
                },
            )
        })
        .collect();
    if wanted.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name(String::from("services"))
        .spawn(move || {
            for (label, opening) in wanted {
                let channels = asked(&label, &opening);
                tracing::info!("{label}: サービス一覧 {} 局", channels.len());
            }
        });
    if let Err(why) = spawned {
        tracing::warn!("サービス一覧を取得するスレッドを起動できない: {why}");
    }
}

pub(crate) fn listed_at_hand(
    file: &crate::config::File,
    profile: &str,
) -> Vec<zwcore::input::source::Channel> {
    let state = crate::config::load_state();
    openings(file, profile)
        .iter()
        .flat_map(|(label, opening)| at_hand(&state, label, opening))
        .collect()
}

/// ソースに問い合わせ直して選べる局。ネットワークに出るので別のスレッドで呼ぶこと。
pub(crate) fn listed_asked(
    file: &crate::config::File,
    profile: &str,
) -> Vec<zwcore::input::source::Channel> {
    openings(file, profile)
        .iter()
        .flat_map(|(label, opening)| asked(label, opening))
        .collect()
}

impl Stage {
    pub(super) fn rebuild_catalog(&mut self) {
        self.catalog_due = false;
        let file = crate::config::load_plain();
        let state = crate::config::load_state();
        let openings = openings(&file, &state.profile);
        self.catalog = openings
            .iter()
            .map(|(label, opening)| Offer {
                label: label.clone(),
                opening: opening.clone(),
                channels: at_hand(&state, label, opening),
            })
            .collect();
        self.roster_seeded = false;
        self.listed_mark = None;
        let now = self.live.as_ref().and_then(|live| live.url.clone());
        let (send, back) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(String::from("catalog"))
            .spawn(move || {
                crate::threads::background::lower();
                let offers: Vec<Offer> = openings
                    .into_iter()
                    .map(|(label, opening)| Offer {
                        channels: asked(&label, &opening),
                        label,
                        opening,
                    })
                    .collect();
                let guided: Vec<(String, String)> = offers
                    .iter()
                    .filter_map(|offer| match &offer.opening {
                        Opening::Mirakurun { url, .. }
                            if Some(offer.opening.came()) != now && !offer.channels.is_empty() =>
                        {
                            Some((offer.label.clone(), url.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                if send.send(Built::Offers(offers)).is_err() {
                    return;
                }
                for (label, url) in guided {
                    match zwcore::input::source::mirakurun::Mirakurun::at(&url)
                        .map_err(|why| why.to_string())
                        .and_then(|mut source| source.guide().map_err(|why| why.to_string()))
                    {
                        Ok(Some(guide)) => {
                            if send.send(Built::Guide(guide)).is_err() {
                                return;
                            }
                        }
                        Ok(None) => {}
                        Err(why) => tracing::warn!("{label}: 全局の番組表を取得できない: {why}"),
                    }
                }
            });
        match spawned {
            Ok(_) => self.catalog_back = Some(back),
            Err(why) => tracing::warn!("局一覧を作成するスレッドを起動できない: {why}"),
        }
    }

    pub(super) fn take_catalog(&mut self) {
        if self.catalog_due {
            self.rebuild_catalog();
        }
        let Some(back) = self.catalog_back.as_ref() else {
            return;
        };
        let mut took = Vec::new();
        loop {
            match back.try_recv() {
                Ok(built) => took.push(built),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.catalog_back = None;
                    break;
                }
            }
        }
        for built in took {
            match built {
                Built::Guide(guide) => {
                    let _ = self.guide_to.send(ToGuide::Merge(Box::new(guide)));
                }
                Built::Offers(offers) => {
                    let counts: Vec<String> = offers
                        .iter()
                        .map(|offer| format!("{} {} 局", offer.label, offer.channels.len()))
                        .collect();
                    tracing::info!("ソース別の局数: {}", counts.join("、"));
                    self.catalog = offers;
                    self.roster_seeded = false;
                    self.listed_mark = None;
                }
            }
        }
    }

    pub(super) fn live_label(&self) -> Option<String> {
        let now = self.live.as_ref()?.url.clone()?;
        self.catalog
            .iter()
            .find(|offer| offer.opening.came() == now)
            .map(|offer| offer.label.clone())
    }

    pub(super) fn best_for(&self, key: zwcore::guide::Key) -> Option<(String, Opening)> {
        self.catalog
            .iter()
            .filter(|offer| !self.tried.contains(&offer.label))
            .find(|offer| {
                offer
                    .channels
                    .iter()
                    .any(|channel| channel.key == Some(key))
            })
            .map(|offer| (offer.label.clone(), offer.opening.clone()))
    }

    pub(super) fn offer_for(&self, key: zwcore::guide::Key) -> Option<(String, Opening)> {
        let now = self.live.as_ref().and_then(|live| live.url.clone());
        self.catalog
            .iter()
            .filter(|offer| Some(offer.opening.came()) != now)
            .filter(|offer| !self.tried.contains(&offer.label))
            .find(|offer| {
                offer
                    .channels
                    .iter()
                    .any(|channel| channel.key == Some(key))
            })
            .map(|offer| (offer.label.clone(), offer.opening.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lineup_comes_back_as_the_same_channels() {
        let channels = vec![
            zwcore::input::source::Channel {
                name: String::from("ゼロウェーブ"),
                key: Some(zwcore::guide::Key {
                    network: 0x7fe6,
                    service: 1072,
                }),
                order: Some(7),
                transport: None,
                how: String::from("/api/channels/GR/23/stream"),
                container: zwcore::filters::Container::Ts,
            },
            zwcore::input::source::Channel {
                name: String::from("名前の無いストリーム"),
                key: None,
                order: None,
                transport: None,
                how: String::from("/api/channels/GR/99/stream"),
                container: zwcore::filters::Container::Ts,
            },
        ];
        let lineup = lineup_of("http://tvrec:40772", "auto", &channels);
        assert_eq!(lineup.channels.len(), 1);
        assert_eq!(lineup_channels(&lineup), channels[..1]);
    }
}
