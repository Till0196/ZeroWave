// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 本体が提供する API：時計、設定、フォント、通信。native ではローカルのスタブ。

#[cfg(target_arch = "wasm32")]
use crate::plug::Buf;

pub use crate::net::Reply;

/// 先頭 0 なら失敗で i32 の番号、1 なら u16 status、長さ付きの Content-Type・Location・ヘッダー、残りが本文。
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, i32> {
    const BROKEN: i32 = -2;
    fn next<'a>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8], i32> {
        let (head, tail) = rest.split_at_checked(n).ok_or(BROKEN)?;
        *rest = tail;
        Ok(head)
    }
    fn u16_next(rest: &mut &[u8]) -> Result<u16, i32> {
        next(rest, 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    let (&kind, mut rest) = bytes.split_first().ok_or(BROKEN)?;
    if kind == 0 {
        return Err(rest
            .first_chunk()
            .map_or(BROKEN, |b| i32::from_be_bytes(*b)));
    }
    let status = u16_next(&mut rest)?;
    let mut texts = [None, None, None];
    for text in &mut texts {
        let n = usize::from(u16_next(&mut rest)?);
        let bytes = next(&mut rest, n)?;
        *text = (!bytes.is_empty()).then(|| String::from_utf8_lossy(bytes).into_owned());
    }
    let [content_type, location, headers] = texts;
    let headers = headers
        .iter()
        .flat_map(|all| all.split("\r\n"))
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    Ok(Reply {
        status,
        content_type,
        location,
        headers,
        body: rest.to_vec(),
    })
}

pub fn encode_reply(reply: Result<&Reply, i32>) -> Vec<u8> {
    let mut out = Vec::new();
    match reply {
        Err(code) => {
            out.push(0);
            out.extend_from_slice(&code.to_be_bytes());
        }
        Ok(reply) => {
            out.push(1);
            out.extend_from_slice(&reply.status.to_be_bytes());
            let headers = reply
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect::<Vec<_>>()
                .join("\r\n");
            for text in [
                reply.content_type.as_deref(),
                reply.location.as_deref(),
                Some(headers.as_str()),
            ] {
                let bytes = text.unwrap_or("").as_bytes();
                let n = bytes.len().min(0xffff);
                out.extend_from_slice(&(n as u16).to_be_bytes());
                out.extend_from_slice(&bytes[..n]);
            }
            out.extend_from_slice(&reply.body);
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    // 本体は渡されたアドレスと長さを確かめてコピーするだけで guest のメモリーへは書かないので、
    // どんな引数で呼んでも guest の側は壊れない。
    #[link(wasm_import_module = "zw_clock_v1")]
    unsafe extern "C" {
        pub safe fn unix_ms() -> f64;
        pub safe fn monotonic_ms() -> f64;
    }

    // guest のメモリーの `dst` へ書くので safe にしない。
    #[link(wasm_import_module = "zw_draw_v1")]
    unsafe extern "C" {
        #[link_name = "shrink"]
        pub fn draw_shrink(
            src: *const u8,
            src_w: u32,
            src_h: u32,
            dst: *mut u8,
            dst_w: u32,
            dst_h: u32,
        ) -> u32;
    }

    #[link(wasm_import_module = "zw_cfg_v1")]
    unsafe extern "C" {
        #[link_name = "get"]
        pub safe fn cfg_get(ptr: *const u8, len: u32) -> u32;
        #[link_name = "set"]
        pub safe fn cfg_set(
            name: *const u8,
            name_len: u32,
            value: *const u8,
            value_len: u32,
        ) -> u32;
        #[link_name = "list"]
        pub safe fn cfg_list() -> u32;
    }

    #[link(wasm_import_module = "zw_font_v1")]
    unsafe extern "C" {
        #[link_name = "get"]
        pub safe fn font_get(ptr: *const u8, len: u32) -> u32;
    }

    #[link(wasm_import_module = "zw_net_v1")]
    unsafe extern "C" {
        pub safe fn has_ip() -> u32;
        pub safe fn resolve(ptr: *const u8, len: u32) -> u32;
        pub safe fn reach(ptr: *const u8, len: u32, port: u32, timeout_ms: u32) -> u32;
        pub safe fn request(
            method_ptr: *const u8,
            method_len: u32,
            url_ptr: *const u8,
            url_len: u32,
            headers: u32,
            body: u32,
        ) -> u32;
        pub safe fn add_root_certificate(der: u32) -> u32;
    }

    #[link(wasm_import_module = "zw_tuner_v1")]
    unsafe extern "C" {
        #[link_name = "words"]
        pub safe fn tuner_words() -> u32;
        #[link_name = "open"]
        pub safe fn tuner_open(words: *const u8, len: u32) -> u32;
        #[link_name = "read"]
        pub safe fn tuner_read(handle: u32) -> u32;
        #[link_name = "close"]
        pub safe fn tuner_close(handle: u32);
    }

    #[link(wasm_import_module = "zw_proc_v1")]
    unsafe extern "C" {
        #[link_name = "spawn"]
        pub safe fn proc_spawn(key: *const u8, key_len: u32, args: *const u8, args_len: u32)
        -> u32;
        #[link_name = "spawn_env"]
        pub safe fn proc_spawn_env(
            key: *const u8,
            key_len: u32,
            args: *const u8,
            args_len: u32,
            env: *const u8,
            env_len: u32,
        ) -> u32;
        #[link_name = "write"]
        pub safe fn proc_write(handle: u32, ptr: *const u8, len: u32) -> u32;
        #[link_name = "read"]
        pub safe fn proc_read(handle: u32) -> u32;
        #[link_name = "close"]
        pub safe fn proc_close(handle: u32);
    }

    #[link(wasm_import_module = "zw_files_v1")]
    unsafe extern "C" {
        #[link_name = "list"]
        pub safe fn files_list(key: *const u8, key_len: u32, dir: *const u8, dir_len: u32) -> u32;
        #[link_name = "read"]
        pub safe fn files_read(key: *const u8, key_len: u32, path: *const u8, path_len: u32)
        -> u32;
    }

    #[link(wasm_import_module = "zw_sock_v1")]
    unsafe extern "C" {
        #[link_name = "connect"]
        pub safe fn sock_connect(key: *const u8, key_len: u32) -> u32;
        #[link_name = "write"]
        pub safe fn sock_write(handle: u32, ptr: *const u8, len: u32) -> u32;
        #[link_name = "read"]
        pub safe fn sock_read(handle: u32) -> u32;
        #[link_name = "close"]
        pub safe fn sock_close(handle: u32);
    }

    #[link(wasm_import_module = "zw_ui_v1")]
    unsafe extern "C" {
        #[link_name = "notice"]
        pub safe fn ui_notice(ptr: *const u8, len: u32);
    }

    #[link(wasm_import_module = "zw_video_v1")]
    unsafe extern "C" {
        #[link_name = "put"]
        pub safe fn video_put(name: *const u8, name_len: u32, ptr: *const u8, len: u32) -> u32;
    }

    #[link(wasm_import_module = "zw_control_v1")]
    unsafe extern "C" {
        #[link_name = "act"]
        pub safe fn control_act(ptr: *const u8, len: u32) -> u32;
        #[link_name = "tune"]
        pub safe fn control_tune(network: u32, service: u32) -> u32;
        #[link_name = "channels"]
        pub safe fn control_channels() -> u32;
        #[link_name = "now"]
        pub safe fn control_now() -> u32;
        #[link_name = "logo"]
        pub safe fn control_logo(network: u32, service: u32, logo_type: u32) -> u32;
        #[link_name = "host"]
        pub safe fn control_host() -> u32;
    }
}

pub mod video {
    /// [`crate::export_video`] が `blobs` を渡すので、ふつうは呼ばなくてよい。
    pub fn put(name: &str, bytes: &[u8]) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            super::wasm::video_put(
                name.as_ptr(),
                name.len() as u32,
                bytes.as_ptr(),
                bytes.len() as u32,
            ) != 0
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (name, bytes);
            true
        }
    }
}

/// hooks に `"control"` を宣言したプラグインだけ。
pub mod control {
    pub fn act(name: &str) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            super::wasm::control_act(name.as_ptr(), name.len() as u32) == 1
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = name;
            false
        }
    }

    pub fn tune(network: u16, service: u16) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            super::wasm::control_tune(u32::from(network), u32::from(service)) == 1
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (network, service);
            false
        }
    }

    pub fn channels() -> Option<crate::control::Channels> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::control_channels();
            let bytes = super::take_buf(handle)?;
            crate::ui::from_json(&bytes).ok()
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// いま見ている局と番組。
    pub fn now() -> Option<crate::control::Now> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::control_now();
            let bytes = super::take_buf(handle)?;
            crate::ui::from_json(&bytes).ok()
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// 局ロゴの PNG。`logo_type` は 0〜7 か [`crate::control::LOGO_ANY`]。
    /// 本体によってはいま見ている局の分だけを返す。
    pub fn logo(network: u16, service: u16, logo_type: u8) -> Option<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::control_logo(
                u32::from(network),
                u32::from(service),
                u32::from(logo_type),
            );
            super::take_buf(handle)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (network, service, logo_type);
            None
        }
    }

    /// 本体の名前とバージョン。
    pub fn host() -> Option<crate::control::Host> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::control_host();
            let bytes = super::take_buf(handle)?;
            crate::ui::from_json(&bytes).ok()
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
}

/// 一行目がタイトル、二行目からが詳細。変わったときだけ出すこと。
pub fn notify(text: &str) {
    notice(&crate::ui::Notice::of_text(text));
}

pub fn notice(notice: &crate::ui::Notice) {
    #[cfg(target_arch = "wasm32")]
    {
        let json = crate::ui::to_json(notice);
        wasm::ui_notice(json.as_ptr(), json.len() as u32);
    }
    #[cfg(not(target_arch = "wasm32"))]
    NOTICES.with(|notices| notices.borrow_mut().push(notice.clone()));
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static NOTICES: std::cell::RefCell<Vec<crate::ui::Notice>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(not(target_arch = "wasm32"))]
pub fn take_notices() -> Vec<crate::ui::Notice> {
    NOTICES.with(|notices| std::mem::take(&mut *notices.borrow_mut()))
}

pub fn unix_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm::unix_ms()
    }
    #[cfg(not(target_arch = "wasm32"))]
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

pub fn monotonic_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm::monotonic_ms()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        static BEGAN: OnceLock<std::time::Instant> = OnceLock::new();
        BEGAN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
            * 1000.0
    }
}

#[cfg(target_arch = "wasm32")]
fn take_buf(handle: u32) -> Option<Vec<u8>> {
    (handle != 0).then(|| Buf::owned(handle).to_vec())
}

#[cfg(target_arch = "wasm32")]
fn take_text(handle: u32) -> Option<String> {
    take_buf(handle).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// 値で判定する（設定ウィンドウで無効にすると `false` が書かれるため）。
pub fn on(name: &str) -> bool {
    cfg(name).is_some_and(|value| crate::abi::truthy(&value))
}

pub fn cfg(name: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let handle = wasm::cfg_get(name.as_ptr(), name.len() as u32);
        take_text(handle)
    }
    #[cfg(not(target_arch = "wasm32"))]
    local::get(name)
}

#[cfg(not(target_arch = "wasm32"))]
mod local {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};

    fn table() -> &'static Mutex<BTreeMap<String, String>> {
        static TABLE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
        TABLE.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    pub fn get(name: &str) -> Option<String> {
        if let Ok(table) = table().lock()
            && let Some(value) = table.get(name)
        {
            return Some(value.clone());
        }
        std::env::var(format!("ZW_{}", name.to_ascii_uppercase())).ok()
    }

    pub fn set(name: &str, value: &str) {
        if let Ok(mut table) = table().lock() {
            table.insert(name.to_owned(), value.to_owned());
        }
    }

    pub fn all() -> Vec<(String, String)> {
        table().lock().map_or_else(
            |_| Vec::new(),
            |table| {
                table
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            },
        )
    }
}

pub fn set_cfg(name: &str, value: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        wasm::cfg_set(
            name.as_ptr(),
            name.len() as u32,
            value.as_ptr(),
            value.len() as u32,
        ) == 1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        local::set(name, value);
        true
    }
}

pub fn cfgs() -> Vec<(String, String)> {
    #[cfg(target_arch = "wasm32")]
    {
        let handle = wasm::cfg_list();
        take_text(handle)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    local::all()
}

/// [`crate::image::shrink`] を本体で行う。
pub fn shrink(rgba: &[u8], from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    #[cfg(target_arch = "wasm32")]
    {
        let mut out = vec![0u8; to.0 as usize * to.1 as usize * 4];
        if rgba.len() >= from.0 as usize * from.1 as usize * 4 {
            // SAFETY: `rgba` は `from` の大きさ以上あり、`out` はちょうど `to` の大きさで、重ならない。
            let done = unsafe {
                wasm::draw_shrink(rgba.as_ptr(), from.0, from.1, out.as_mut_ptr(), to.0, to.1)
            };
            if done == 1 {
                return out;
            }
        }
        crate::image::shrink(rgba, from, to)
    }
    #[cfg(not(target_arch = "wasm32"))]
    crate::image::shrink(rgba, from, to)
}

/// `name` は `sans`・`mono`、`kind = "font"` のキー、OS のフォントフォルダー以下のパス、フォント名のどれか。
/// 名前で検索したものは TTC でもそのフォントだけのファイルで来る。
pub fn font(name: &str) -> Option<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    {
        let handle = wasm::font_get(name.as_ptr(), name.len() as u32);
        take_buf(handle)
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "host"))]
    {
        crate::host::fonts::lend(name)
    }
    #[cfg(all(not(target_arch = "wasm32"), not(feature = "host")))]
    {
        let _ = name;
        None
    }
}

pub mod net {
    use super::Reply;
    #[cfg(target_arch = "wasm32")]
    use super::{decode_reply, take_buf, take_text};
    #[cfg(target_arch = "wasm32")]
    use crate::plug::Buf;

    pub fn has_ip() -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            super::wasm::has_ip() != 0
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }

    pub fn resolve(host: &str) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::resolve(host.as_ptr(), host.len() as u32);
            take_text(handle)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = host;
            None
        }
    }

    pub fn reach(host: &str, port: u16, timeout_ms: u32) -> Option<(String, u32)> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::reach(
                host.as_ptr(),
                host.len() as u32,
                u32::from(port),
                timeout_ms,
            );
            let text = take_text(handle)?;
            let (address, ms) = text.split_once(' ')?;
            Some((address.to_owned(), ms.parse().ok()?))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (host, port, timeout_ms);
            None
        }
    }

    /// 失敗は `transmitTextDataOverIP` の返り値の番号。
    pub fn request(
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
    ) -> Result<Reply, i32> {
        #[cfg(target_arch = "wasm32")]
        {
            let header_text: String = headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
            let header_buf = (!header_text.is_empty()).then(|| Buf::of(header_text.as_bytes()));
            let body_buf = body.map(Buf::of);
            let handle = super::wasm::request(
                method.as_ptr(),
                method.len() as u32,
                url.as_ptr(),
                url.len() as u32,
                header_buf.as_ref().map_or(0, Buf::handle),
                body_buf.as_ref().map_or(0, Buf::handle),
            );
            let bytes = take_buf(handle).ok_or(-2)?;
            decode_reply(&bytes)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (method, url, headers, body);
            Err(-300)
        }
    }

    pub fn add_root_certificate(der: &[u8]) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let buf = Buf::of(der);
            super::wasm::add_root_certificate(buf.handle()) != 0
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = der;
            false
        }
    }
}

/// 起動できるのは `kind = "program"` のキーにユーザーが入れたパスだけ。
pub mod proc {
    #[cfg(target_arch = "wasm32")]
    use super::take_buf;

    #[derive(Debug)]
    pub struct Process {
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        handle: u32,
    }

    impl Process {
        pub fn spawn<S: AsRef<str>>(key: &str, args: &[S]) -> Option<Self> {
            let args = crate::config::list_text(args);
            #[cfg(target_arch = "wasm32")]
            {
                let handle = super::wasm::proc_spawn(
                    key.as_ptr(),
                    key.len() as u32,
                    args.as_ptr(),
                    args.len() as u32,
                );
                (handle != 0).then_some(Self { handle })
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (key, args);
                None
            }
        }

        /// 子プロセスに渡す環境変数を `kind = "map"` のキーで。値が無ければ追加せずに起動する。
        pub fn spawn_with<S: AsRef<str>>(key: &str, args: &[S], env: &str) -> Option<Self> {
            let args = crate::config::list_text(args);
            #[cfg(target_arch = "wasm32")]
            {
                let handle = super::wasm::proc_spawn_env(
                    key.as_ptr(),
                    key.len() as u32,
                    args.as_ptr(),
                    args.len() as u32,
                    env.as_ptr(),
                    env.len() as u32,
                );
                (handle != 0).then_some(Self { handle })
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (key, args, env);
                None
            }
        }

        pub fn write(&mut self, bytes: &[u8]) -> bool {
            #[cfg(target_arch = "wasm32")]
            {
                super::wasm::proc_write(self.handle, bytes.as_ptr(), bytes.len() as u32) != 0
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = bytes;
                false
            }
        }

        /// 待たない。子が終了して出力を読み切っていれば `None`。
        pub fn read(&mut self) -> Option<Vec<u8>> {
            #[cfg(target_arch = "wasm32")]
            {
                let handle = super::wasm::proc_read(self.handle);
                take_buf(handle)
            }
            #[cfg(not(target_arch = "wasm32"))]
            None
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            #[cfg(target_arch = "wasm32")]
            {
                super::wasm::proc_close(self.handle)
            }
        }
    }
}

/// 接続先は `kind = "socket"` のキーの値。パスならドメインソケット、`host:port` なら TCP、空なら coaxd。
/// Windows の `\\.\pipe\…` は名前付きパイプ。値の `$名前`・`${名前}` は本体の環境変数で展開する。
pub mod sock {
    #[cfg(target_arch = "wasm32")]
    use super::take_buf;

    #[derive(Debug)]
    pub struct Socket {
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        handle: u32,
    }

    impl Socket {
        pub fn connect(key: &str) -> Option<Self> {
            #[cfg(target_arch = "wasm32")]
            {
                let handle = super::wasm::sock_connect(key.as_ptr(), key.len() as u32);
                (handle != 0).then_some(Self { handle })
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = key;
                None
            }
        }

        pub fn write(&mut self, bytes: &[u8]) -> bool {
            #[cfg(target_arch = "wasm32")]
            {
                super::wasm::sock_write(self.handle, bytes.as_ptr(), bytes.len() as u32) != 0
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = bytes;
                false
            }
        }

        /// 待たない。相手が閉じて読み切っていれば `None`。
        pub fn read(&mut self) -> Option<Vec<u8>> {
            #[cfg(target_arch = "wasm32")]
            {
                let handle = super::wasm::sock_read(self.handle);
                take_buf(handle)
            }
            #[cfg(not(target_arch = "wasm32"))]
            None
        }
    }

    impl Drop for Socket {
        fn drop(&mut self) {
            #[cfg(target_arch = "wasm32")]
            {
                super::wasm::sock_close(self.handle)
            }
        }
    }
}

/// `kind = "folder"` のキーが指すフォルダーの下だけで、書き込み API は無い。
pub mod files {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Entry {
        pub name: String,
        pub dir: bool,
        pub size: u64,
    }

    fn parse(text: &str) -> Vec<Entry> {
        text.lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let kind = parts.next()?;
                let size = parts.next()?.parse().ok()?;
                let name = parts.next()?.to_owned();
                Some(Entry {
                    name,
                    dir: kind == "d",
                    size,
                })
            })
            .collect()
    }

    pub fn list(key: &str, dir: &str) -> Option<Vec<Entry>> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::files_list(
                key.as_ptr(),
                key.len() as u32,
                dir.as_ptr(),
                dir.len() as u32,
            );
            super::take_text(handle).map(|text| parse(&text))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let root = native::root(key)?.join(dir);
            let mut entries: Vec<Entry> = std::fs::read_dir(root)
                .ok()?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let meta = entry.metadata().ok()?;
                    Some(Entry {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        dir: meta.is_dir(),
                        size: if meta.is_dir() { 0 } else { meta.len() },
                    })
                })
                .collect();
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            let _ = parse;
            Some(entries)
        }
    }

    pub fn read(key: &str, path: &str) -> Option<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::files_read(
                key.as_ptr(),
                key.len() as u32,
                path.as_ptr(),
                path.len() as u32,
            );
            super::take_buf(handle)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::fs::read(native::root(key)?.join(path)).ok()
        }
    }

    pub fn read_text(key: &str, path: &str) -> Option<String> {
        read(key, path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub use native::set_root;

    #[cfg(not(target_arch = "wasm32"))]
    mod native {
        use std::collections::BTreeMap;
        use std::path::PathBuf;
        use std::sync::Mutex;

        static ROOTS: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());

        pub fn set_root(key: &str, root: impl Into<PathBuf>) {
            ROOTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key.to_owned(), root.into());
        }

        pub(super) fn root(key: &str) -> Option<PathBuf> {
            ROOTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(key)
                .cloned()
        }
    }
}

/// hooks で `"tuners"` を宣言したプラグインだけ。
pub mod tuner {
    #[cfg(target_arch = "wasm32")]
    use super::{take_buf, take_text};

    pub fn words() -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            let handle = super::wasm::tuner_words();
            take_text(handle)
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    #[derive(Debug)]
    pub struct Tuner {
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        handle: u32,
    }

    impl Tuner {
        pub fn open(words: &str) -> Option<Self> {
            #[cfg(target_arch = "wasm32")]
            {
                let handle = super::wasm::tuner_open(words.as_ptr(), words.len() as u32);
                (handle != 0).then_some(Self { handle })
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = words;
                None
            }
        }

        /// 待たない。終了して出力を読み切っていれば `None`。
        pub fn read(&mut self) -> Option<Vec<u8>> {
            #[cfg(target_arch = "wasm32")]
            {
                let handle = super::wasm::tuner_read(self.handle);
                take_buf(handle)
            }
            #[cfg(not(target_arch = "wasm32"))]
            None
        }
    }

    impl Drop for Tuner {
        fn drop(&mut self) {
            #[cfg(target_arch = "wasm32")]
            {
                super::wasm::tuner_close(self.handle)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_survives_the_round_trip() {
        let reply = Reply {
            status: 200,
            content_type: Some("text/html".into()),
            location: None,
            headers: vec![
                ("Content-Type".into(), "text/html".into()),
                ("X-API-Ticket".into(), "ticket-1".into()),
            ],
            body: b"<p>".to_vec(),
        };
        assert_eq!(decode_reply(&encode_reply(Ok(&reply))), Ok(reply));
        assert_eq!(decode_reply(&encode_reply(Err(-500))), Err(-500));
    }
}
