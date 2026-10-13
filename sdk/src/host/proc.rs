// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 子プロセスとの受け渡し（`zw_proc_v1`）。起動するのは `kind = "program"` のキーにユーザーが入れたパスだけ。
//! 書く側は上限付きのキューでバックプレッシャーが掛かり、読む側はたまった分を待たずに返す。

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use wasmtime::{Caller, Linker};

use crate::abi;
use crate::host::pump::Inbox;
use crate::host::{HostState, Level, guest_bytes, guest_text};
use crate::slots::Slots;

const WRITE_QUEUE: usize = 64;

const STDOUT_HELD: usize = 256;

const STDERR_HELD: usize = 256;
const STDERR_LINE_MOST: usize = 8 << 10;

/// これだけ空かなければ子は止まったと見なす（再生のスレッドを止めっぱなしにしない）。
const WRITE_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) type Procs = Slots<Proc>;

pub(crate) struct Proc {
    child: Child,
    input: Option<SyncSender<Vec<u8>>>,
    output: Inbox,
    errors: Receiver<String>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Proc {
    fn spawn(path: &str, args: &[String], envs: &[(String, String)]) -> std::io::Result<Self> {
        let mut command = Command::new(path);
        command
            .args(args)
            .envs(envs.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW：コンソールの無い本体から起動しても黒いウィンドウを出さない。
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");

        let (input, queued) = sync_channel::<Vec<u8>>(WRITE_QUEUE);
        std::thread::Builder::new()
            .name(String::from("proc-stdin"))
            .spawn(move || {
                for bytes in queued {
                    if stdin.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })?;

        // 溢れたら待つ。捨てると子プロセスの出力が壊れ、貯め続けるといくらでも増える。
        let output = Inbox::spawn_waiting("proc-stdout", stdout, STDOUT_HELD)?;

        // guest が読まない間に溢れた行は捨てる。
        let (said, errors) = sync_channel::<String>(STDERR_HELD);
        std::thread::Builder::new()
            .name(String::from("proc-stderr"))
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match (&mut reader)
                        .take(STDERR_LINE_MOST as u64)
                        .read_until(b'\n', &mut line)
                    {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim_end_matches(['\r', '\n']).to_owned();
                    if let Err(TrySendError::Disconnected(_)) = said.try_send(text) {
                        break;
                    }
                }
            })?;

        Ok(Self {
            child,
            input: Some(input),
            output,
            errors,
        })
    }

    fn write(&mut self, mut bytes: Vec<u8>) -> bool {
        let Some(input) = &self.input else {
            return false;
        };
        let deadline = std::time::Instant::now() + WRITE_PATIENCE;
        loop {
            match input.try_send(bytes) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(back)) => {
                    if std::time::Instant::now() >= deadline {
                        self.input = None;
                        return false;
                    }
                    bytes = back;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
    }

    fn read(&self) -> Option<Vec<u8>> {
        self.output.drain()
    }
}

/// 値の `$名前`・`${名前}` は `lookup` で展開する。名前に使えない文字のある物は二つ目に返す。
pub(crate) fn expand_env(
    pairs: Vec<(String, String)>,
    lookup: impl Fn(&str) -> Option<String>,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut envs = Vec::new();
    let mut bad = Vec::new();
    for (name, value) in pairs {
        if crate::config::env_name(&name) {
            let value = expand(&value, &lookup);
            envs.push((name, value));
        } else {
            bad.push(name);
        }
    }
    (envs, bad)
}

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn expand(value: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, next) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], &braced[end + 1..]),
                None => ("", after),
            }
        } else {
            let end = after.find(|c| !name_char(c)).unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if name.is_empty() {
            out.push('$');
            rest = after;
        } else {
            out.push_str(&lookup(name).unwrap_or_default());
            rest = next;
        }
    }
    out.push_str(rest);
    out
}

fn spawn_keyed(state: &mut HostState, key: &str, args: &str, env_key: Option<&str>) -> u32 {
    let args = crate::config::list_of(args);
    let Some(path) = state.io.program(key) else {
        return 0;
    };
    let pairs = env_key.map_or_else(Vec::new, |env_key| state.io.env(env_key));
    let (envs, bad) = expand_env(pairs, |name| std::env::var(name).ok());
    for name in bad {
        state.log(
            Level::Warning,
            &format!("環境変数の「{name}」は名前に使えない文字があるので渡しません"),
        );
    }
    match Proc::spawn(&path, &args, &envs) {
        Ok(proc) => state.procs.put(proc),
        Err(error) => {
            state.log(Level::Warning, &format!("{path} を起動できない: {error}"));
            0
        }
    }
}

fn relay_errors(state: &HostState, handle: u32) {
    if let Some(proc) = state.procs.get(handle) {
        for line in proc.errors.try_iter() {
            state.log(Level::Info, &line);
        }
    }
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::PROC,
        "spawn",
        |mut caller: Caller<'_, HostState>,
         key: u32,
         key_len: u32,
         args: u32,
         args_len: u32|
         -> u32 {
            let key = guest_text(&mut caller, key, key_len);
            let args = guest_text(&mut caller, args, args_len);
            spawn_keyed(caller.data_mut(), &key, &args, None)
        },
    )?;
    linker.func_wrap(
        abi::import::PROC,
        "spawn_env",
        |mut caller: Caller<'_, HostState>,
         key: u32,
         key_len: u32,
         args: u32,
         args_len: u32,
         env: u32,
         env_len: u32|
         -> u32 {
            let key = guest_text(&mut caller, key, key_len);
            let args = guest_text(&mut caller, args, args_len);
            let env = guest_text(&mut caller, env, env_len);
            spawn_keyed(caller.data_mut(), &key, &args, Some(&env))
        },
    )?;
    linker.func_wrap(
        abi::import::PROC,
        "write",
        |mut caller: Caller<'_, HostState>, handle: u32, ptr: u32, len: u32| -> u32 {
            let Some(bytes) = guest_bytes(&mut caller, ptr, len) else {
                return 0;
            };
            let state = caller.data_mut();
            relay_errors(state, handle);
            state
                .procs
                .get_mut(handle)
                .is_some_and(|proc| proc.write(bytes))
                .into()
        },
    )?;
    linker.func_wrap(
        abi::import::PROC,
        "read",
        |mut caller: Caller<'_, HostState>, handle: u32| -> u32 {
            let state = caller.data_mut();
            relay_errors(state, handle);
            match state.procs.get(handle).and_then(Proc::read) {
                Some(bytes) => state.put_buf(bytes),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::PROC,
        "close",
        |mut caller: Caller<'_, HostState>, handle: u32| {
            caller.data_mut().procs.take(handle);
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_values_have_the_hosts_variables_expanded() {
        let host = |name: &str| match name {
            "TMPDIR" => Some(String::from("/var/folders/x/T/")),
            "HOME" => Some(String::from("/Users/me")),
            _ => None,
        };
        let pairs = [
            ("PCSCLITE_CSOCK_NAME", "$TMPDIR/coaxd/pcscd.comm"),
            ("B", "a b"),
            ("C", "${HOME}x"),
            ("D", "$NOPE"),
            ("E", "1$"),
            ("F", "$"),
            ("o-ops", "x"),
            ("", "x"),
        ]
        .map(|(n, v)| (n.to_owned(), v.to_owned()));
        let (envs, bad) = expand_env(pairs.to_vec(), host);
        let pairs: Vec<(&str, &str)> = envs.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        assert_eq!(
            pairs,
            [
                ("PCSCLITE_CSOCK_NAME", "/var/folders/x/T//coaxd/pcscd.comm"),
                ("B", "a b"),
                ("C", "/Users/mex"),
                ("D", ""),
                ("E", "1$"),
                ("F", "$"),
            ]
        );
        assert_eq!(bad, ["o-ops", ""]);
    }

    #[cfg(unix)]
    #[test]
    fn the_child_sees_the_environment() {
        let proc = Proc::spawn(
            "sh",
            &[
                String::from("-c"),
                String::from("printf %s \"$ZW_A:${PATH:+path}\""),
            ],
            &[(String::from("ZW_A"), String::from("x y"))],
        )
        .expect("sh");
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match proc.read() {
                Some(bytes) => got.extend(bytes),
                None => break,
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(String::from_utf8_lossy(&got), "x y:path");
    }

    #[cfg(unix)]
    #[test]
    fn bytes_go_through_the_child_and_come_back() {
        let mut proc = Proc::spawn("cat", &[], &[]).expect("cat");
        let sent: Vec<u8> = (0..200_000u32).map(|n| n as u8).collect();
        for chunk in sent.chunks(7000) {
            assert!(proc.write(chunk.to_vec()));
        }
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while got.len() < sent.len() && std::time::Instant::now() < deadline {
            got.extend(proc.read().unwrap_or_default());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, sent);
    }

    #[cfg(unix)]
    #[test]
    fn a_finished_child_reads_as_gone() {
        let proc = Proc::spawn("true", &[], &[]).expect("true");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match proc.read() {
                None => break,
                Some(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Some(_) => panic!("終わったのに読める"),
            }
        }
    }
}
