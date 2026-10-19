// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! チャンネルの上下で送る先。

use zwcore::guide::{Key, Programme, Service};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Stepping {
    /// サービス ID の順に送る（`operation.channel_order = "service"`）。偽ならリモコン番号の順。
    pub(super) by_service: bool,
    /// 同じ TS で同じ番組を放送しているサービスは飛ばす。
    pub(super) skip_sub: bool,
}

impl Default for Stepping {
    fn default() -> Self {
        Self::of(&crate::config::file::Operation::default())
    }
}

impl Stepping {
    pub(super) fn of(operation: &crate::config::file::Operation) -> Self {
        Self {
            by_service: operation.channel_order == "service",
            skip_sub: operation.channel_step_skips_sub,
        }
    }
}

/// `from` から `forward` の向きに送る先。`programme` はいまの番組（分からなければ `None`）。
pub(super) fn next<'a>(
    services: &[&Service],
    from: Key,
    forward: bool,
    stepping: Stepping,
    programme: impl Fn(Key) -> Option<&'a Programme>,
) -> Option<Key> {
    let mut services = services.to_vec();
    if stepping.by_service {
        services.sort_by_key(|service| (service.network, service.service));
    } else {
        services.sort_by_key(|service| service.order());
    }
    let count = services.len();
    if count < 2 {
        return None;
    }
    let at = services.iter().position(|service| service.key() == from)?;
    let step = |at: usize| {
        if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        }
    };
    let mut next = step(at);
    if !stepping.skip_sub {
        return Some(services[next].key());
    }
    let current = services[at];
    // 異なる番組を放送している場合以外はサブチャンネルを飛ばす。
    let mut differs = false;
    loop {
        if next == at {
            return None;
        }
        let candidate = services[next];
        if !same_stream(current, candidate) {
            break;
        }
        if different(programme(current.key()), programme(candidate.key())) {
            differs = true;
            break;
        }
        next = step(next);
    }
    if !forward && !differs {
        // 戻るときは、同じ番組を放送しているサービスの最初へ。
        let landed = services[next];
        while next > 0 {
            let before = services[next - 1];
            if !same_stream(landed, before)
                || different(programme(landed.key()), programme(before.key()))
            {
                break;
            }
            next -= 1;
        }
    }
    Some(services[next].key())
}

fn same_stream(a: &Service, b: &Service) -> bool {
    a.network == b.network && a.transport == b.transport
}

/// どちらの番組も分かっていて、違う番組か。イベント共有記述子で同じ番組を指していれば同じ。
fn different(a: Option<&Programme>, b: Option<&Programme>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return false;
    };
    let shared = b.shared.contains(&(a.service, a.event))
        || a.shared.contains(&(b.service, b.event))
        || a.shared.iter().any(|one| b.shared.contains(one));
    let same = (a.service, a.event) == (b.service, b.event);
    !(same || shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(network: u16, transport: u16, id: u16, remote: u8) -> Service {
        Service {
            network,
            transport,
            service: id,
            remote_key: Some(remote),
            ..Service::default()
        }
    }

    fn programme(service: u16, event: u16, shared: &[(u16, u16)]) -> Programme {
        Programme {
            shared: shared.to_vec(),
            ..Programme::scheduled(
                Key {
                    network: 0x7fe0,
                    service,
                },
                event,
                None,
                None,
            )
        }
    }

    /// 総合は 1024・1025、教育は 1032・1033。リモコン番号は 1 と 2、別の局が 3。
    fn services() -> Vec<Service> {
        vec![
            service(0x7fe0, 0x7fe0, 1024, 1),
            service(0x7fe0, 0x7fe0, 1025, 1),
            service(0x7fe1, 0x7fe1, 1032, 2),
            service(0x7fe1, 0x7fe1, 1033, 2),
            service(0x7fe8, 0x7fe8, 1048, 3),
        ]
    }

    fn key(network: u16, service: u16) -> Key {
        Key { network, service }
    }

    const PLAIN: Stepping = Stepping {
        by_service: false,
        skip_sub: false,
    };

    #[test]
    fn channels_go_round_in_remote_order() {
        let all = services();
        let list: Vec<&Service> = all.iter().collect();
        let none = |_: Key| None;
        assert_eq!(
            next(&list, key(0x7fe0, 1025), true, PLAIN, none),
            Some(key(0x7fe1, 1032))
        );
        assert_eq!(
            next(&list, key(0x7fe0, 1024), false, PLAIN, none),
            Some(key(0x7fe8, 1048))
        );
    }

    /// サービスの順なら、リモコン番号ではなくネットワーク ID とサービス ID の順。
    #[test]
    fn channels_can_go_in_service_order() {
        let mut all = services();
        all[4].remote_key = Some(1);
        let list: Vec<&Service> = all.iter().collect();
        let none = |_: Key| None;
        assert_eq!(
            next(&list, key(0x7fe0, 1025), true, PLAIN, none),
            Some(key(0x7fe8, 1048)),
            "同じ 1 番の局が続く"
        );
        let by_service = Stepping {
            by_service: true,
            ..PLAIN
        };
        assert_eq!(
            next(&list, key(0x7fe0, 1025), true, by_service, none),
            Some(key(0x7fe1, 1032))
        );
    }

    #[test]
    fn sub_channels_on_the_same_programme_are_skipped() {
        let all = services();
        let list: Vec<&Service> = all.iter().collect();
        let skip = Stepping {
            skip_sub: true,
            ..PLAIN
        };
        let programmes = [
            programme(1024, 1, &[]),
            programme(1025, 9, &[(1024, 1)]),
            programme(1032, 2, &[]),
            programme(1033, 3, &[]),
        ];
        let now = |key: Key| programmes.iter().find(|p| p.service == key.service);
        assert_eq!(
            next(&list, key(0x7fe0, 1024), true, skip, now),
            Some(key(0x7fe1, 1032)),
            "1025 は 1024 の番組を共有しているので飛ばす"
        );
        assert_eq!(
            next(&list, key(0x7fe1, 1032), true, skip, now),
            Some(key(0x7fe1, 1033)),
            "1033 は違う番組なので止まる"
        );
        assert_eq!(
            next(&list, key(0x7fe1, 1032), false, skip, now),
            Some(key(0x7fe0, 1024)),
            "戻るときは同じ番組の最初のサービスへ"
        );
    }

    /// 番組が分からなければ、同じ TS のサービスは飛ばす。
    #[test]
    fn sub_channels_are_skipped_without_the_guide() {
        let all = services();
        let list: Vec<&Service> = all.iter().collect();
        let skip = Stepping {
            skip_sub: true,
            ..PLAIN
        };
        let none = |_: Key| None;
        assert_eq!(
            next(&list, key(0x7fe0, 1024), true, skip, none),
            Some(key(0x7fe1, 1032))
        );
        assert_eq!(
            next(&list, key(0x7fe8, 1048), true, skip, none),
            Some(key(0x7fe0, 1024))
        );
    }
}
