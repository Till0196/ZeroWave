// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 用途を限らない API：時計、設定、フォント、通信。通信は同期で、guest が待つ間本体のスレッドも待つ。

use std::collections::{BTreeMap, BTreeSet};

use wasmtime::{Caller, Linker};

use crate::abi;
use crate::host::{HostState, guest_text, since_epoch};

/// 設定は上書き、共有のテーブル、受像機の設定、既定の順に探す。
#[derive(Default, Clone)]
pub struct Io {
    pub cfg: BTreeMap<String, String>,
    pub live: Option<Live>,
    pub defaults: BTreeMap<String, String>,
    pub fonts: BTreeMap<String, std::sync::Arc<Vec<u8>>>,
    pub store: Option<Store>,
    /// ユーザーが入れたパスだけが起動でき、guest の `set` では書けない。
    pub programs: BTreeSet<String>,
    /// guest の `set` は書けない（`DYLD_INSERT_LIBRARIES` などで別の物を読ませられるため）。
    pub envs: BTreeSet<String>,
    pub sockets: BTreeSet<String>,
    /// `zw_files_v1` が読めるのはこれらが指すフォルダーの下だけ。
    pub folders: BTreeSet<String>,
    /// guest が `set` で書いても共有のテーブルに置くだけで、ファイルには残さない。
    pub states: BTreeSet<String>,
    /// guest の `set` は書けない（好きなファイルを読めてしまうので）。
    pub font_keys: BTreeSet<String>,
    pub home: Option<std::path::PathBuf>,
    pub receiver: Option<Live>,
    /// hooks に `"tuners"` を宣言したプラグインにだけ入れる。
    pub lender: Option<crate::host::tuner::LenderOf>,
    pub notify: Option<Notify>,
    /// hooks に `"control"` を宣言したプラグインにだけ入れる。
    pub control: Option<Control>,
}

pub trait Controls: Send + Sync {
    fn act(&self, name: &str) -> bool;
    fn tune(&self, network: u16, service: u16) -> bool;
    fn channels(&self) -> crate::control::Channels;
}

pub type Control = std::sync::Arc<dyn Controls>;

pub type Notify = std::sync::Arc<dyn Fn(crate::ui::Notice) + Send + Sync>;

pub type Store = std::sync::Arc<dyn Fn(&BTreeMap<String, String>) + Send + Sync>;

pub type Live = std::sync::Arc<std::sync::RwLock<BTreeMap<String, String>>>;

impl Io {
    pub fn get(&self, name: &str) -> Option<String> {
        self.chosen(name)
            .or_else(|| {
                self.receiver.as_ref().and_then(|receiver| {
                    receiver
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(name)
                        .cloned()
                })
            })
            .or_else(|| self.defaults.get(name).cloned())
    }

    pub fn on(&self, name: &str) -> bool {
        self.get(name).is_some_and(|value| truthy(&value))
    }

    /// `net` のキーを宣言し、それが真のときだけ。`no_net` が立っていれば全部「繋がらない」。
    pub fn net_allowed(&self) -> bool {
        self.get("net").is_some_and(|value| truthy(&value)) && !self.on("no_net")
    }

    pub fn net_closed(&self) -> bool {
        self.get("net").is_some_and(|value| !truthy(&value)) || self.on("no_net")
    }

    /// ユーザーが `net_local` を入れたときだけで、宣言の既定値は見ない。
    pub fn net_inner(&self) -> bool {
        self.chosen(NET_INNER).is_some_and(|value| truthy(&value))
    }

    /// 宣言の既定値は見ない（プラグインが自分で起動先を決められないように）。
    pub fn program(&self, key: &str) -> Option<String> {
        let value = self.declared(&self.programs, key)?;
        // Windows の「パスのコピー」は前後に `"` を付ける。
        let value = value
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .unwrap_or(&value)
            .trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    /// （名前、展開する前の値）。宣言の既定値は見ず、外した項目は渡さない。
    pub fn env(&self, key: &str) -> Vec<(String, String)> {
        self.declared(&self.envs, key)
            .map(|text| crate::config::map_on(&text))
            .unwrap_or_default()
    }

    /// `zw_font_v1` の `get`。`sans`・`mono`、`kind = "font"` のキー、OS のフォントフォルダー以下のパス、フォント名の順。
    pub fn font(&self, name: &str) -> Option<Vec<u8>> {
        if let Some(lent) = self.fonts.get(name) {
            return Some(lent.to_vec());
        }
        if self.font_keys.contains(name) {
            let value = self.declared(&self.font_keys, name)?;
            return crate::host::fonts::lend_chosen(&value);
        }
        crate::host::fonts::lend(name)
    }

    fn declared(&self, kind: &BTreeSet<String>, key: &str) -> Option<String> {
        if !kind.contains(key) {
            return None;
        }
        let value = self.chosen(key)?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    pub(crate) fn chosen_value(&self, key: &str) -> Option<String> {
        self.chosen(key)
    }

    fn chosen(&self, key: &str) -> Option<String> {
        self.cfg.get(key).cloned().or_else(|| {
            self.live.as_ref().and_then(|live| {
                live.read()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(key)
                    .cloned()
            })
        })
    }

    pub fn all(&self) -> BTreeMap<String, String> {
        let mut all = self.defaults.clone();
        if let Some(live) = &self.live {
            all.extend(
                live.read()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
        }
        all.extend(self.cfg.iter().map(|(k, v)| (k.clone(), v.clone())));
        all
    }

    fn set(&mut self, name: String, value: String) -> bool {
        // 起動するプログラムのパス、環境変数、接続先はユーザーだけが決める。
        if self.programs.contains(&name)
            || self.envs.contains(&name)
            || self.sockets.contains(&name)
            || self.folders.contains(&name)
            || self.font_keys.contains(&name)
            || NET_KEYS.contains(&name.as_str())
        {
            return false;
        }
        let state = self.states.contains(&name);
        let snapshot = match &self.live {
            Some(live) => {
                let mut table = live.write().unwrap_or_else(|e| e.into_inner());
                if table.get(&name) == Some(&value) {
                    return true;
                }
                table.insert(name, value);
                table.clone()
            }
            None => {
                if self.cfg.get(&name) == Some(&value) {
                    return true;
                }
                self.cfg.insert(name, value);
                self.cfg.clone()
            }
        };
        if state {
            return true;
        }
        let mut snapshot = snapshot;
        snapshot.retain(|key, _| !self.states.contains(key));
        match &self.store {
            Some(store) => {
                store(&snapshot);
                true
            }
            None => false,
        }
    }
}

pub use crate::abi::truthy;

pub const NET_INNER: &str = "net_local";

const NET_KEYS: [&str; 3] = ["net", "no_net", NET_INNER];

impl std::fmt::Debug for Io {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Io")
            .field("cfg", &self.cfg)
            .field("live", &self.live.is_some())
            .field("defaults", &self.defaults)
            .field("fonts", &self.fonts.keys().collect::<Vec<_>>())
            .field("store", &self.store.is_some())
            .field("programs", &self.programs)
            .field("sockets", &self.sockets)
            .field("envs", &self.envs)
            .field("folders", &self.folders)
            .field("font_keys", &self.font_keys)
            .field("states", &self.states)
            .field("home", &self.home)
            .field("lender", &self.lender.is_some())
            .field("notify", &self.notify.is_some())
            .finish()
    }
}

fn deliver(caller: &Caller<'_, HostState>, notice: crate::ui::Notice) {
    match &caller.data().io.notify {
        Some(notify) => notify(notice),
        None => caller.data().log(crate::host::Level::Info, &notice.title),
    }
}

fn give(caller: &mut Caller<'_, HostState>, bytes: Option<Vec<u8>>) -> u32 {
    bytes.map_or(0, |bytes| caller.data_mut().put_buf(bytes))
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(abi::import::CLOCK, "unix_ms", || -> f64 {
        since_epoch().as_secs_f64() * 1000.0
    })?;
    linker.func_wrap(
        abi::import::CLOCK,
        "monotonic_ms",
        |caller: Caller<'_, HostState>| -> f64 {
            caller.data().began.elapsed().as_secs_f64() * 1000.0
        },
    )?;
    linker.func_wrap(
        abi::import::CFG,
        "get",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
            let name = guest_text(&mut caller, ptr, len);
            let value = caller.data().io.get(&name).map(String::into_bytes);
            give(&mut caller, value)
        },
    )?;
    linker.func_wrap(
        abi::import::CFG,
        "set",
        |mut caller: Caller<'_, HostState>,
         name: u32,
         name_len: u32,
         value: u32,
         value_len: u32|
         -> u32 {
            let name = guest_text(&mut caller, name, name_len);
            if name.is_empty() {
                return 0;
            }
            let value = guest_text(&mut caller, value, value_len);
            u32::from(caller.data_mut().io.set(name, value))
        },
    )?;
    linker.func_wrap(
        abi::import::CFG,
        "list",
        |mut caller: Caller<'_, HostState>| -> u32 {
            let written = caller
                .data()
                .io
                .all()
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("\n");
            give(&mut caller, Some(written.into_bytes()))
        },
    )?;
    linker.func_wrap(
        abi::import::UI,
        "notify",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| {
            let text = guest_text(&mut caller, ptr, len);
            deliver(&caller, crate::ui::Notice::of_text(&text));
        },
    )?;
    linker.func_wrap(
        abi::import::UI,
        "notice",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| {
            let text = guest_text(&mut caller, ptr, len);
            match crate::ui::from_json::<crate::ui::Notice>(text.as_bytes()) {
                Ok(notice) => deliver(&caller, notice),
                Err(why) => caller.data().log(
                    crate::host::Level::Warning,
                    &format!("通知が読めない: {why}"),
                ),
            }
        },
    )?;
    linker.func_wrap(
        abi::import::CONTROL,
        "act",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
            let name = guest_text(&mut caller, ptr, len);
            let control = caller.data().io.control.clone();
            u32::from(control.is_some_and(|control| control.act(&name)))
        },
    )?;
    linker.func_wrap(
        abi::import::CONTROL,
        "tune",
        |caller: Caller<'_, HostState>, network: u32, service: u32| -> u32 {
            let control = caller.data().io.control.clone();
            u32::from(control.is_some_and(|control| control.tune(network as u16, service as u16)))
        },
    )?;
    linker.func_wrap(
        abi::import::CONTROL,
        "channels",
        |mut caller: Caller<'_, HostState>| -> u32 {
            let control = caller.data().io.control.clone();
            let answer = control.map(|control| crate::ui::to_json(&control.channels()));
            give(&mut caller, answer)
        },
    )?;
    linker.func_wrap(
        abi::import::FONT,
        "get",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
            let name = guest_text(&mut caller, ptr, len);
            let value = caller.data().io.font(&name);
            give(&mut caller, value)
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "has_ip",
        |caller: Caller<'_, HostState>| -> u32 {
            u32::from(caller.data().io.net_allowed() && crate::net::has_ip())
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "resolve",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
            let host = guest_text(&mut caller, ptr, len);
            if !caller.data().io.net_allowed() {
                return 0;
            }
            let found = crate::net::resolve(&host).map(String::into_bytes);
            give(&mut caller, found)
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "reach",
        |mut caller: Caller<'_, HostState>,
         ptr: u32,
         len: u32,
         port: u32,
         timeout_ms: u32|
         -> u32 {
            let host = guest_text(&mut caller, ptr, len);
            if !caller.data().io.net_allowed() {
                return 0;
            }
            let found = crate::net::reach_from(
                &host,
                port as u16,
                std::time::Duration::from_millis(u64::from(timeout_ms)),
                caller.data().io.net_inner(),
            )
            .map(|(address, ms)| format!("{address} {ms}").into_bytes());
            give(&mut caller, found)
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "request",
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
            let headers: Vec<(String, String)> = caller
                .data()
                .bufs
                .get(headers)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default()
                .lines()
                .filter_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    Some((k.trim().to_owned(), v.trim().to_owned()))
                })
                .collect();
            let body = caller.data().bufs.get(body).cloned();
            let state = caller.data();
            let reply = if state.io.net_allowed() {
                state.net.send(
                    &method,
                    &url,
                    &headers,
                    body.as_deref(),
                    state.io.net_inner(),
                )
            } else {
                Err(crate::net::Failure::Connect)
            };
            let encoded = crate::plug::host::encode_reply(reply.as_ref().map_err(|f| f.code()));
            give(&mut caller, Some(encoded))
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "add_root_certificate",
        |caller: Caller<'_, HostState>, der: u32| -> u32 {
            let state = caller.data();
            let der = state.bufs.get(der).map_or(&[][..], Vec::as_slice);
            u32::from(state.net.add_root_certificate(der))
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_receiver_settings_are_seen_as_they_change() {
        let receiver: Live = std::sync::Arc::default();
        let io = Io {
            receiver: Some(std::sync::Arc::clone(&receiver)),
            ..Io::default()
        };
        assert_eq!(io.get("receiver.zipcode"), None);
        receiver
            .write()
            .unwrap()
            .insert(String::from("receiver.zipcode"), String::from("1000001"));
        assert_eq!(io.get("receiver.zipcode").as_deref(), Some("1000001"));
    }
    use super::*;

    fn shared(pairs: &[(&str, &str)]) -> Live {
        std::sync::Arc::new(std::sync::RwLock::new(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        ))
    }

    #[test]
    fn overrides_then_the_shared_table_then_the_defaults() {
        let mut io = Io {
            live: Some(shared(&[("model_id", "ZW-1"), ("trace", "false")])),
            ..Io::default()
        };
        io.defaults
            .insert(String::from("net"), String::from("true"));
        io.defaults.insert(String::from("model_id"), String::new());
        assert_eq!(
            io.get("model_id").as_deref(),
            Some("ZW-1"),
            "テーブルが既定より先"
        );
        assert_eq!(
            io.get("net").as_deref(),
            Some("true"),
            "どこにも無ければ既定"
        );
        io.cfg.insert(String::from("model_id"), String::from("ENV"));
        assert_eq!(io.get("model_id").as_deref(), Some("ENV"), "上書きが一番");
        assert_eq!(io.get("nothing"), None);
    }

    #[test]
    fn a_false_flag_is_off() {
        let io = Io {
            live: Some(shared(&[("trace", "false")])),
            ..Io::default()
        };
        assert!(!io.on("trace"));
        let io = Io {
            cfg: [(String::from("trace"), String::from("1"))].into(),
            ..Io::default()
        };
        assert!(io.on("trace"), "環境変数の ZW_TRACE=1 も入っている");
    }

    #[test]
    fn the_network_is_closed_by_either_switch() {
        let table = shared(&[]);
        let io = Io {
            live: Some(std::sync::Arc::clone(&table)),
            ..Io::default()
        };
        assert!(!io.net_allowed(), "宣言しなければ接続しない");
        assert!(!io.net_closed());
        table
            .write()
            .unwrap()
            .insert(String::from("net"), String::from("true"));
        assert!(io.net_allowed(), "ユーザーが許可すれば接続する");
        table
            .write()
            .unwrap()
            .insert(String::from("net"), String::from("false"));
        assert!(
            !io.net_allowed(),
            "ウィンドウで無効にしたら次の呼び出しから"
        );
        assert!(io.net_closed());
        table
            .write()
            .unwrap()
            .insert(String::from("net"), String::from("true"));
        let mut io = io;
        io.cfg.insert(String::from("no_net"), String::from("1"));
        assert!(!io.net_allowed(), "ZW_NO_NET でも止まる");
    }

    #[test]
    fn only_the_user_opens_the_network() {
        let table = shared(&[]);
        let mut io = Io {
            live: Some(std::sync::Arc::clone(&table)),
            ..Io::default()
        };
        io.defaults
            .insert(String::from(NET_INNER), String::from("true"));
        assert!(!io.net_inner(), "宣言の既定値ではローカルへ接続しない");
        for key in ["net", "no_net", NET_INNER] {
            assert!(!io.set(String::from(key), String::from("true")), "{key}");
        }
        assert!(!io.net_inner(), "guest は書けない");
        table
            .write()
            .unwrap()
            .insert(String::from(NET_INNER), String::from("true"));
        assert!(io.net_inner(), "ユーザーが入れれば");
    }

    #[test]
    fn a_guest_write_goes_to_the_shared_table_only() {
        let table = shared(&[]);
        let stored = std::sync::Arc::new(std::sync::Mutex::new(None));
        let into = std::sync::Arc::clone(&stored);
        let mut io = Io {
            cfg: [(String::from("trace"), String::from("1"))].into(),
            live: Some(std::sync::Arc::clone(&table)),
            store: Some(std::sync::Arc::new(
                move |cfg: &BTreeMap<String, String>| {
                    *into.lock().unwrap() = Some(cfg.clone());
                },
            )),
            ..Io::default()
        };
        assert!(io.set(String::from("seen"), String::from("yes")));
        let written = stored.lock().unwrap().clone().expect("書いた");
        assert_eq!(written.get("seen").map(String::as_str), Some("yes"));
        assert!(!written.contains_key("trace"), "上書きは残さない");
        assert_eq!(
            table.read().unwrap().get("seen").map(String::as_str),
            Some("yes")
        );
    }

    #[test]
    fn a_state_is_shared_but_not_stored() {
        let table = shared(&[]);
        let stored = std::sync::Arc::new(std::sync::Mutex::new(0));
        let written = std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        let (into, into_written) = (
            std::sync::Arc::clone(&stored),
            std::sync::Arc::clone(&written),
        );
        let mut io = Io {
            live: Some(std::sync::Arc::clone(&table)),
            store: Some(std::sync::Arc::new(
                move |cfg: &BTreeMap<String, String>| {
                    *into.lock().unwrap() += 1;
                    *into_written.lock().unwrap() = cfg.clone();
                },
            )),
            states: [String::from("now")].into(),
            ..Io::default()
        };
        assert!(io.set(String::from("now"), String::from("running")));
        assert_eq!(*stored.lock().unwrap(), 0);
        assert_eq!(io.get("now").as_deref(), Some("running"));
        assert!(io.set(String::from("other"), String::from("1")));
        assert_eq!(*stored.lock().unwrap(), 1);
        assert!(
            !written.lock().unwrap().contains_key("now"),
            "ほかのキーを書くときも残さない"
        );
    }

    #[test]
    fn a_switched_off_variable_is_not_passed() {
        let io = Io {
            live: Some(shared(&[("env", r##"{"#B":"2","A":"1"}"##)])),
            envs: [String::from("env")].into(),
            ..Io::default()
        };
        assert_eq!(io.env("env"), [(String::from("A"), String::from("1"))]);
    }

    #[test]
    fn a_font_is_lent_by_its_key_or_from_the_font_folders_only() {
        let folder = std::env::temp_dir().join(format!("zw-font-{}", std::process::id()));
        let fonts = folder.join("fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("a.ttc"), b"face").unwrap();
        std::fs::write(folder.join("b.ttf"), b"other").unwrap();
        let table = shared(&[]);
        let mut io = Io {
            live: Some(std::sync::Arc::clone(&table)),
            font_keys: [String::from("font_round")].into(),
            fonts: [(String::from("sans"), std::sync::Arc::new(b"lent".to_vec()))].into(),
            ..Io::default()
        };
        assert_eq!(
            io.font("sans").as_deref(),
            Some(&b"lent"[..]),
            "本体が提供するものが先"
        );
        assert_eq!(io.font("font_round"), None, "空は無いのと同じ");
        let picked = folder.join("b.ttf").to_string_lossy().into_owned();
        assert!(
            !io.set(String::from("font_round"), picked.clone()),
            "guest は書けない"
        );
        table
            .write()
            .unwrap()
            .insert(String::from("font_round"), format!("\"{picked}\""));
        assert_eq!(
            io.font("font_round").as_deref(),
            Some(&b"other"[..]),
            "ユーザーが選んだもの"
        );
        assert_eq!(
            io.font(&picked),
            None,
            "フォントフォルダーの外はパスでは読めない"
        );
        let folders = [fonts.clone()];
        use crate::host::fonts::{is_font_file, under_any};
        assert!(under_any(&fonts.join("a.ttc"), &folders));
        assert!(
            !under_any(&fonts.join("../b.ttf"), &folders),
            "`..` で外へ出られない"
        );
        assert!(is_font_file(std::path::Path::new(
            "ヒラギノ丸ゴ ProN W4.TTC"
        )));
        assert!(!is_font_file(std::path::Path::new("/etc/passwd")));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn only_the_user_chooses_what_to_run() {
        let table = shared(&[]);
        let mut io = Io {
            live: Some(std::sync::Arc::clone(&table)),
            programs: [String::from("ts")].into(),
            ..Io::default()
        };
        io.defaults
            .insert(String::from("ts"), String::from("/bin/sh"));
        assert_eq!(io.program("ts"), None, "既定からは起動しない");
        assert!(!io.set(String::from("ts"), String::from("/bin/sh")));
        assert_eq!(io.program("ts"), None, "guest は書けない");
        table
            .write()
            .unwrap()
            .insert(String::from("ts"), String::from("  /opt/tsdecoder  "));
        assert_eq!(io.program("ts").as_deref(), Some("/opt/tsdecoder"));
        table.write().unwrap().insert(
            String::from("ts"),
            String::from(r#""C:\Program Files\tsdecoder.exe""#),
        );
        assert_eq!(
            io.program("ts").as_deref(),
            Some(r"C:\Program Files\tsdecoder.exe"),
            "「パスのコピー」の引用符は外す"
        );
        table
            .write()
            .unwrap()
            .insert(String::from("other"), String::from("/bin/sh"));
        assert_eq!(io.program("other"), None, "宣言していないキーは起動しない");
        table
            .write()
            .unwrap()
            .insert(String::from("ts"), String::from("   "));
        assert_eq!(io.program("ts"), None, "空は無いのと同じ");
    }

    #[test]
    fn only_the_user_chooses_the_environment() {
        let table = shared(&[]);
        let mut io = Io {
            live: Some(std::sync::Arc::clone(&table)),
            envs: [String::from("ts_env")].into(),
            ..Io::default()
        };
        let preload = String::from(r#"{"LD_PRELOAD":"/x.so"}"#);
        io.defaults.insert(String::from("ts_env"), preload.clone());
        assert_eq!(io.env("ts_env"), [], "既定は渡さない");
        assert!(!io.set(String::from("ts_env"), preload));
        assert_eq!(io.env("ts_env"), [], "guest は書けない");
        table
            .write()
            .unwrap()
            .insert(String::from("ts_env"), String::from(r#"{"A":"1"}"#));
        assert_eq!(io.env("ts_env"), [(String::from("A"), String::from("1"))]);
        table
            .write()
            .unwrap()
            .insert(String::from("other"), String::from(r#"{"A":"1"}"#));
        assert_eq!(io.env("other"), [], "宣言していないキーは渡さない");
    }
}
