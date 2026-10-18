// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 引数が文字列と u32 だけのメソッドを呼ぶための、libdbus（`dbus/dbus.h`）の最小限の呼び出し。
//! libdbus は実行時に開き（[`zwcore::dl::Object`]）、無い機械では頼めないだけ。

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::OnceLock;

use zwcore::dl::Object;

const SONAMES: [&str; 2] = ["libdbus-1.so.3", "libdbus-1.so"];
const TIMEOUT_MS: c_int = 5_000;

const TYPE_INVALID: c_int = 0;
const TYPE_STRING: c_int = b's' as c_int;
const TYPE_UINT32: c_int = b'u' as c_int;
const TYPE_UNIX_FD: c_int = b'h' as c_int;

#[derive(Clone, Copy)]
pub enum Kind {
    Session = 0,
    System = 1,
}

pub enum Arg<'a> {
    Text(&'a CStr),
    U32(u32),
}

/// `DBusError` と同じ並び（ビットフィールドの 5 ビットは一つの `unsigned int` に収まる）。
#[repr(C)]
struct Error {
    name: *const c_char,
    message: *const c_char,
    dummy: c_uint,
    padding: *mut c_void,
}

type Connection = *mut c_void;
type Message = *mut c_void;

struct Symbols {
    _object: Object,
    threads_init_default: unsafe extern "C" fn() -> c_uint,
    error_init: unsafe extern "C" fn(*mut Error),
    error_free: unsafe extern "C" fn(*mut Error),
    error_is_set: unsafe extern "C" fn(*const Error) -> c_uint,
    bus_get_private: unsafe extern "C" fn(c_int, *mut Error) -> Connection,
    set_exit_on_disconnect: unsafe extern "C" fn(Connection, c_uint),
    connection_close: unsafe extern "C" fn(Connection),
    connection_unref: unsafe extern "C" fn(Connection),
    new_method_call:
        unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char) -> Message,
    append_args: unsafe extern "C" fn(Message, c_int, ...) -> c_uint,
    send_with_reply_and_block:
        unsafe extern "C" fn(Connection, Message, c_int, *mut Error) -> Message,
    get_args: unsafe extern "C" fn(Message, *mut Error, c_int, ...) -> c_uint,
    message_unref: unsafe extern "C" fn(Message),
}

macro_rules! symbol {
    ($object:expr, $name:literal) => {
        // SAFETY: the name is an entry point of libdbus, bound to its signature in `Symbols`.
        unsafe { $object.symbol($name) }.map_err(|missing| format!("libdbus: {missing}"))?
    };
}

impl Symbols {
    fn open() -> Result<Self, String> {
        let object = Object::open_any(&SONAMES).map_err(|missing| missing.to_string())?;
        let symbols = Self {
            threads_init_default: symbol!(object, b"dbus_threads_init_default\0"),
            error_init: symbol!(object, b"dbus_error_init\0"),
            error_free: symbol!(object, b"dbus_error_free\0"),
            error_is_set: symbol!(object, b"dbus_error_is_set\0"),
            bus_get_private: symbol!(object, b"dbus_bus_get_private\0"),
            set_exit_on_disconnect: symbol!(object, b"dbus_connection_set_exit_on_disconnect\0"),
            connection_close: symbol!(object, b"dbus_connection_close\0"),
            connection_unref: symbol!(object, b"dbus_connection_unref\0"),
            new_method_call: symbol!(object, b"dbus_message_new_method_call\0"),
            append_args: symbol!(object, b"dbus_message_append_args\0"),
            send_with_reply_and_block: symbol!(
                object,
                b"dbus_connection_send_with_reply_and_block\0"
            ),
            get_args: symbol!(object, b"dbus_message_get_args\0"),
            message_unref: symbol!(object, b"dbus_message_unref\0"),
            _object: object,
        };
        // SAFETY: 引数は無く、何度呼んでもよい。
        unsafe { (symbols.threads_init_default)() };
        Ok(symbols)
    }

    fn error(&self) -> Error {
        let mut error = Error {
            name: std::ptr::null(),
            message: std::ptr::null(),
            dummy: 0,
            padding: std::ptr::null_mut(),
        };
        // SAFETY: `error` は書き込める `DBusError`。
        unsafe { (self.error_init)(&mut error) };
        error
    }

    /// 立っていれば文字にして片付ける。
    fn take(&self, error: &mut Error) -> Option<String> {
        // SAFETY: `error_init` した `DBusError`。
        if unsafe { (self.error_is_set)(error) } == 0 {
            return None;
        }
        let text = [error.name, error.message]
            .into_iter()
            .filter(|text| !text.is_null())
            // SAFETY: 立っている `DBusError` の名前と文は NUL 終端。
            .map(|text| {
                unsafe { CStr::from_ptr(text) }
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>()
            .join(": ");
        // SAFETY: 同上。
        unsafe { (self.error_free)(error) };
        Some(text)
    }
}

fn symbols() -> Result<&'static Symbols, String> {
    static SYMBOLS: OnceLock<Result<Symbols, String>> = OnceLock::new();
    SYMBOLS
        .get_or_init(Symbols::open)
        .as_ref()
        .map_err(Clone::clone)
}

/// 自分だけの接続（閉じれば相手が抑止を外せる）。
pub struct Bus {
    symbols: &'static Symbols,
    connection: Connection,
}

impl Bus {
    pub fn open(kind: Kind) -> Result<Self, String> {
        let symbols = symbols()?;
        let mut error = symbols.error();
        // SAFETY: `error` は初期化した `DBusError`。
        let connection = unsafe { (symbols.bus_get_private)(kind as c_int, &mut error) };
        if let Some(why) = symbols.take(&mut error) {
            return Err(why);
        }
        if connection.is_null() {
            return Err(String::from("バスに繋げない"));
        }
        // 既定ではバスが切れるとプロセスごと終わる。
        // SAFETY: 繋いだばかりの接続。
        unsafe { (symbols.set_exit_on_disconnect)(connection, 0) };
        Ok(Self {
            symbols,
            connection,
        })
    }

    /// 応答を待って返す。呼んだ側が `message_unref` する。
    fn send(
        &self,
        destination: &CStr,
        path: &CStr,
        interface: &CStr,
        method: &CStr,
        args: &[Arg],
    ) -> Result<Message, String> {
        let symbols = self.symbols;
        let what = || {
            format!(
                "{}.{}",
                interface.to_string_lossy(),
                method.to_string_lossy()
            )
        };
        // SAFETY: どれも NUL 終端の文字列。
        let message = unsafe {
            (symbols.new_method_call)(
                destination.as_ptr(),
                path.as_ptr(),
                interface.as_ptr(),
                method.as_ptr(),
            )
        };
        if message.is_null() {
            return Err(format!("{}: メッセージを作れない", what()));
        }
        let mut appended = true;
        for arg in args {
            // SAFETY: 型の印に合う物の位置を渡し、`TYPE_INVALID` で終える。値は呼び出しの間にコピーされる。
            appended &= unsafe {
                match arg {
                    Arg::Text(text) => {
                        let pointer = text.as_ptr();
                        (symbols.append_args)(
                            message,
                            TYPE_STRING,
                            &raw const pointer,
                            TYPE_INVALID,
                        )
                    }
                    Arg::U32(value) => {
                        (symbols.append_args)(message, TYPE_UINT32, &raw const *value, TYPE_INVALID)
                    }
                }
            } != 0;
        }
        let mut error = symbols.error();
        let reply = if appended {
            // SAFETY: 生きている接続とメッセージ。
            unsafe {
                (symbols.send_with_reply_and_block)(
                    self.connection,
                    message,
                    TIMEOUT_MS,
                    &mut error,
                )
            }
        } else {
            std::ptr::null_mut()
        };
        // SAFETY: 作ったメッセージを一度だけ手放す（送るときは libdbus が自分で参照を持つ）。
        unsafe { (symbols.message_unref)(message) };
        if let Some(why) = symbols.take(&mut error) {
            return Err(format!("{}: {why}", what()));
        }
        if reply.is_null() {
            return Err(format!("{}: 応答が無い", what()));
        }
        Ok(reply)
    }

    pub fn call(
        &self,
        destination: &CStr,
        path: &CStr,
        interface: &CStr,
        method: &CStr,
        args: &[Arg],
    ) -> Result<(), String> {
        let reply = self.send(destination, path, interface, method, args)?;
        // SAFETY: `send` が返した応答。
        unsafe { (self.symbols.message_unref)(reply) };
        Ok(())
    }

    /// 応答の最初の値を `kind` の型で `into` へ読む。
    fn read(&self, reply: Message, kind: c_int, into: *mut c_void) -> Result<(), String> {
        let symbols = self.symbols;
        let mut error = symbols.error();
        // SAFETY: `into` は `kind` の型の書き込める場所。
        let read = unsafe { (symbols.get_args)(reply, &mut error, kind, into, TYPE_INVALID) };
        // SAFETY: `send` が返した応答を一度だけ手放す。
        unsafe { (symbols.message_unref)(reply) };
        match symbols.take(&mut error) {
            Some(why) => Err(why),
            None if read == 0 => Err(String::from("応答を読めない")),
            None => Ok(()),
        }
    }

    pub fn call_for_u32(
        &self,
        destination: &CStr,
        path: &CStr,
        interface: &CStr,
        method: &CStr,
        args: &[Arg],
    ) -> Result<u32, String> {
        let reply = self.send(destination, path, interface, method, args)?;
        let mut value: u32 = 0;
        self.read(reply, TYPE_UINT32, (&raw mut value).cast())?;
        Ok(value)
    }

    pub fn call_for_fd(
        &self,
        destination: &CStr,
        path: &CStr,
        interface: &CStr,
        method: &CStr,
        args: &[Arg],
    ) -> Result<OwnedFd, String> {
        let reply = self.send(destination, path, interface, method, args)?;
        let mut fd: c_int = -1;
        self.read(reply, TYPE_UNIX_FD, (&raw mut fd).cast())?;
        if fd < 0 {
            return Err(String::from("fd が来ない"));
        }
        // SAFETY: `dbus_message_get_args` が複製して渡した fd で、持ち主はこちら。
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        // SAFETY: 自分だけの接続は閉じてから手放す。
        unsafe {
            (self.symbols.connection_close)(self.connection);
            (self.symbols.connection_unref)(self.connection);
        }
    }
}
