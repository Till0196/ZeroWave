// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 待たない通信（`zw_net_v1` の `open`・`ws`・`poll`・`send`・`close`）。HTTP は応答の本文を届いた分ずつ、
//! WebSocket はメッセージごとに渡す。何も無ければ `poll` は待たずに 0 を返す。
//! `request` と同じく、`net` を宣言して真のときだけで、内部ネットワークへは `net_local` のときだけ。
//! 記録の形は [`crate::net::conn`]。

use wasmtime::{Caller, Linker};

use crate::abi;
use crate::host::io::header_lines;
use crate::host::{HostState, Level, guest_bytes, guest_text};
use crate::net::conn::{Arrived, Open, Outgoing, send as send_kind};
use crate::slots::Slots;

/// 一回の `poll` で渡す上限。
const POLL_MOST: usize = 4 << 20;

pub(crate) type Conns = Slots<Conn>;

#[derive(Debug)]
pub(crate) struct Conn {
    conn: Box<dyn crate::net::Conn>,
    /// 終わりを渡した。
    ended: bool,
    /// 次の `poll` に回す（上限を超えた分）。
    held: Option<Arrived>,
}

impl Conn {
    fn poll(&mut self) -> Option<Vec<u8>> {
        if self.ended {
            return None;
        }
        let mut out = Vec::new();
        let mut size = 0;
        while let Some(arrived) = self.held.take().or_else(|| self.conn.next()) {
            if !out.is_empty() && size + arrived.len() > POLL_MOST {
                self.held = Some(arrived);
                break;
            }
            size += arrived.len();
            arrived.encode(&mut out);
            if arrived.is_last() {
                self.ended = true;
                break;
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

fn open(state: &mut HostState, open: Open) -> u32 {
    if !state.io.net_allowed() {
        state.log(Level::Warning, "zw_net_v1: net が有効でないので接続しない");
        return 0;
    }
    let conn = Conn {
        conn: state.net.open(open),
        ended: false,
        held: None,
    };
    let handle = state.conns.put(conn);
    if handle == 0 {
        state.log(Level::Warning, "zw_net_v1: 同時に開ける接続の上限を超えた");
    }
    handle
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::NET,
        "open",
        |mut caller: Caller<'_, HostState>,
         method_ptr: u32,
         method_len: u32,
         url_ptr: u32,
         url_len: u32,
         headers: u32,
         body: u32|
         -> u32 {
            let method = guest_text(&mut caller, method_ptr, method_len);
            let url = guest_text(&mut caller, url_ptr, url_len);
            let state = caller.data_mut();
            let request = Open {
                method,
                url,
                headers: header_lines(state, headers),
                body: state.bufs.get(body).cloned(),
                protocols: None,
                inner: state.io.net_inner(),
            };
            open(state, request)
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "ws",
        |mut caller: Caller<'_, HostState>,
         url_ptr: u32,
         url_len: u32,
         protocols_ptr: u32,
         protocols_len: u32,
         headers: u32|
         -> u32 {
            let url = guest_text(&mut caller, url_ptr, url_len);
            let protocols = guest_text(&mut caller, protocols_ptr, protocols_len);
            let state = caller.data_mut();
            let request = Open {
                method: String::from("GET"),
                url,
                headers: header_lines(state, headers),
                body: None,
                protocols: Some(protocols),
                inner: state.io.net_inner(),
            };
            open(state, request)
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "poll",
        |mut caller: Caller<'_, HostState>, handle: u32| -> u32 {
            let state = caller.data_mut();
            match state.conns.get_mut(handle).and_then(Conn::poll) {
                Some(bytes) => state.put_buf(bytes),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "send",
        |mut caller: Caller<'_, HostState>, handle: u32, kind: u32, ptr: u32, len: u32| -> u32 {
            let Some(bytes) = guest_bytes(&mut caller, ptr, len) else {
                return 0;
            };
            let outgoing = match kind {
                send_kind::TEXT => Outgoing::Text(bytes),
                send_kind::BINARY => Outgoing::Binary(bytes),
                send_kind::CLOSE => {
                    let code = bytes
                        .get(..2)
                        .map_or(1000, |c| u16::from_be_bytes([c[0], c[1]]));
                    Outgoing::Close(code, bytes.get(2..).unwrap_or(&[]).to_vec())
                }
                _ => return 0,
            };
            caller
                .data_mut()
                .conns
                .get_mut(handle)
                .is_some_and(|conn| conn.conn.send(outgoing))
                .into()
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "close",
        |mut caller: Caller<'_, HostState>, handle: u32| {
            caller.data_mut().conns.take(handle);
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Failure;

    #[derive(Debug)]
    struct Script(std::collections::VecDeque<Arrived>);

    impl crate::net::Conn for Script {
        fn next(&mut self) -> Option<Arrived> {
            self.0.pop_front()
        }

        fn send(&mut self, _: Outgoing) -> bool {
            true
        }
    }

    fn conn(all: Vec<Arrived>) -> Conn {
        Conn {
            conn: Box::new(Script(all.into())),
            ended: false,
            held: None,
        }
    }

    #[test]
    fn a_poll_gives_what_arrived_up_to_its_limit_and_nothing_after_the_end() {
        let big = vec![0u8; POLL_MOST];
        let mut conn = conn(vec![
            Arrived::Head {
                status: 200,
                headers: Vec::new(),
            },
            Arrived::Data(big.clone()),
            Arrived::Data(vec![1]),
            Arrived::Fail(Failure::Disconnected.code()),
        ]);
        let first = crate::net::conn::decode(&conn.poll().unwrap());
        assert_eq!(first.len(), 2, "上限を超える分は次へ");
        let second = crate::net::conn::decode(&conn.poll().unwrap());
        assert_eq!(
            second,
            [
                Arrived::Data(vec![1]),
                Arrived::Fail(Failure::Disconnected.code())
            ]
        );
        assert_eq!(conn.poll(), None);
    }
}
