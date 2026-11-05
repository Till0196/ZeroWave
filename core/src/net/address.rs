// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 宛先の確かめ。

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use super::Error;

/// loopback・LAN・CGNAT・マルチキャストなど。
pub fn is_inner(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            // 240.0.0.0/4（予約）
            a >= 240
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || a == 0
                // 100.64.0.0/10（CGNAT）
                || (a == 100 && (b & 0xc0) == 64)
                || (a == 198 && (b & 0xfe) == 18)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_inner(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            let embedded = |high: u16, low: u16| {
                let [a, b] = high.to_be_bytes();
                let [c, d] = low.to_be_bytes();
                IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d))
            };
            // NAT64（64:ff9b::/96）と 6to4（2002::/16）は中の IPv4 へ届く。
            if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return is_inner(embedded(segments[6], segments[7]));
            }
            if segments[0] == 0x2002 && is_inner(embedded(segments[1], segments[2])) {
                return true;
            }
            let first = segments[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first & 0xffc0) == 0xfec0
        }
    }
}

/// 名前を引いたアドレスのうち、繋いでよい物。そのアドレスに直接繋ぐので、確かめた後で替えられることは無い。
pub fn addresses_of(host: &str, port: u16, inner: bool) -> Result<Vec<SocketAddr>, Error> {
    let found: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| Error::Name)?
        .collect();
    if found.is_empty() {
        return Err(Error::Name);
    }
    let kept: Vec<SocketAddr> = found
        .into_iter()
        .filter(|address| inner || !is_inner(address.ip()))
        .collect();
    if kept.is_empty() {
        tracing::debug!("内部ネットワークの宛先なので接続しない: {host}");
        return Err(Error::Inner);
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_addresses_are_told_apart() {
        for inner in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::c0a8:101",
            "2002:c0a8:101::1",
            "240.0.0.1",
        ] {
            assert!(is_inner(inner.parse().unwrap()), "{inner}");
        }
        for outer in [
            "192.0.2.1",
            "203.0.113.1",
            "2001:db8::1",
            "::ffff:198.51.100.1",
            "64:ff9b::c633:6401",
            "2002:c633:6401::1",
        ] {
            assert!(!is_inner(outer.parse().unwrap()), "{outer}");
        }
    }

    #[test]
    fn inner_addresses_are_refused_unless_allowed() {
        assert_eq!(addresses_of("127.0.0.1", 9, false), Err(Error::Inner));
        assert!(addresses_of("127.0.0.1", 9, true).is_ok());
    }
}
