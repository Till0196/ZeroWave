// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `video` hook の一つのインスタンス。レンダリングスレッドから呼んでよい。クラッシュしたら以後は触らない。

use wasmtime::{Caller, Linker, TypedFunc};

use crate::abi;
use crate::host::{HostState, Level, Made, files, guest_bytes, guest_text};
use crate::video::{Blob, Env, Graph, Launch};

/// 一つのビルドで渡せる blob の数（重み・係数・シェーダーのテクスチャ）。
const BLOBS_COUNT_MOST: usize = 256;

pub struct VideoInstance {
    name: String,
    made: Made,
    handle: u32,
    poisoned: bool,
    build: TypedFunc<(u32, u32), u32>,
    key: Option<TypedFunc<(u32, u32), u32>>,
    event: Option<TypedFunc<(u32, u32), ()>>,
    drop: TypedFunc<u32, ()>,
}

impl std::fmt::Debug for VideoInstance {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("VideoInstance")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// 実行ファイルは `kind = "program"`、作業フォルダーと `PATH` は `kind = "folder"` のキーの値だけ。
fn launch(io: &crate::host::Io, process: &crate::video::Process) -> Result<Launch, String> {
    let program = io.program(&process.program).ok_or_else(|| {
        format!(
            "起動する実行ファイル（設定の {}）が選ばれていません。",
            process.program
        )
    })?;
    let folder = |key: &str| {
        io.folder(key)
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| format!("設定の {key} がフォルダーの項目ではないか、空です。"))
    };
    let dir = if process.dir.is_empty() {
        String::new()
    } else {
        folder(&process.dir)?
    };
    let path = process
        .path
        .iter()
        .map(|key| folder(key))
        .collect::<Result<_, _>>()?;
    Ok(Launch {
        program,
        args: process.args.clone(),
        dir,
        path,
        ways: process.ways.clone(),
    })
}

impl VideoInstance {
    pub(super) fn new(name: String, mut made: Made) -> Result<Self, String> {
        let new: TypedFunc<(), u32> = made.func(abi::export::VIDEO_NEW)?;
        let build = made.func(abi::export::VIDEO_BUILD)?;
        let drop = made.func(abi::export::VIDEO_DROP)?;
        let key = made.func(abi::export::VIDEO_KEY).ok();
        let event = made.func(abi::export::VIDEO_EVENT).ok();
        let handle = new
            .call(made.armed(), ())
            .map_err(|error| format!("{}: {error}", abi::export::VIDEO_NEW))?;
        if handle == 0 {
            return Err(format!("{} が 0 を返した", abi::export::VIDEO_NEW));
        }
        Ok(Self {
            name,
            made,
            handle,
            poisoned: false,
            build,
            key,
            event,
            drop,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn build(&mut self, env: &Env) -> Result<Graph, String> {
        if self.poisoned {
            return Err(format!(
                "{} はエラーで停止しているため使えません。",
                self.name
            ));
        }
        let mut env = env.clone();
        env.settings = self.made.store.data().io.all();
        self.made.store.data_mut().blobs.clear();
        let input = self.made.give(crate::video::to_json(&env)).map_err(|why| {
            self.poisoned = true;
            format!("{}：{why}", self.name)
        })?;
        let result = self.build.call(self.made.armed(), (self.handle, input));
        let state = self.made.store.data_mut();
        state.bufs.take(input);
        let answer = match result {
            Ok(answer) => answer,
            Err(error) => {
                self.poisoned = true;
                return Err(format!("{} でエラーが発生しました。\n{error}", self.name));
            }
        };
        let bytes = state.bufs.take(answer).unwrap_or_default();
        let blobs = std::mem::take(&mut state.blobs);
        let reply: Result<Graph, String> = crate::video::from_json(&bytes)
            .map_err(|why| format!("{} から受け取った内容を読み取れません。\n{why}", self.name))?;
        let mut graph = reply?;
        graph.blobs = blobs;
        let io = &self.made.store.data().io;
        for pass in &mut graph.passes {
            pass.launch = match &pass.process {
                Some(process) => Some(
                    launch(io, process)
                        .map_err(|why| format!("{}（{}）：{why}", self.name, pass.name))?,
                ),
                None => None,
            };
        }
        Ok(graph)
    }

    /// 返さないなら `None`（問い合わせるたびにビルドし直す）。
    pub fn key(&mut self, env: &Env) -> Option<String> {
        if self.poisoned {
            return None;
        }
        let key = self.key.clone()?;
        let mut env = env.clone();
        env.settings = self.made.store.data().io.all();
        let input = self.made.give(crate::video::to_json(&env)).ok()?;
        let result = key.call(self.made.armed(), (self.handle, input));
        let state = self.made.store.data_mut();
        state.bufs.take(input);
        match result {
            Ok(0) => None,
            Ok(answer) => state
                .bufs
                .take(answer)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) => {
                self.poisoned = true;
                tracing::warn!("{}: {}: {error}", self.name, abi::export::VIDEO_KEY);
                None
            }
        }
    }

    pub fn event(&mut self, event: &crate::video::Event) {
        let Some(func) = self.event.clone().filter(|_| !self.poisoned) else {
            return;
        };
        let Ok(input) = self.made.give(crate::video::to_json(event)) else {
            return;
        };
        let result = func.call(self.made.armed(), (self.handle, input));
        self.made.store.data_mut().bufs.take(input);
        if let Err(error) = result {
            self.poisoned = true;
            tracing::warn!("{}: {}: {error}", self.name, abi::export::VIDEO_EVENT);
        }
    }

    /// 変わったら [`VideoInstance::build`] を呼び直す。
    pub fn stamp(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let io = &self.made.store.data().io;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        io.all().hash(&mut hasher);
        files::fingerprint(io).hash(&mut hasher);
        hasher.finish()
    }
}

impl Drop for VideoInstance {
    fn drop(&mut self) {
        if !self.poisoned {
            let _ = self.drop.call(self.made.armed(), self.handle);
        }
    }
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::VIDEO,
        "put",
        |mut caller: Caller<'_, HostState>, name: u32, name_len: u32, ptr: u32, len: u32| -> u32 {
            let name = guest_text(&mut caller, name, name_len);
            let Some(bytes) = guest_bytes(&mut caller, ptr, len) else {
                return 0;
            };
            let state = caller.data_mut();
            let others = state.blobs.iter().filter(|(other, _)| **other != name);
            let (count, held) = others.fold((0, 0), |(count, held), (other, blob)| {
                (count + 1, held + other.len() + blob.0.len())
            });
            let most = state.limits.blobs;
            if held + name.len() + bytes.len() > most || count >= BLOBS_COUNT_MOST {
                state.log(
                    Level::Warning,
                    &format!(
                        "zw_video_v1: blob が上限を超える（{name}、{} 個目・合計 {} バイト、上限 {BLOBS_COUNT_MOST} 個・{} MiB）",
                        count + 1,
                        held + name.len() + bytes.len(),
                        most >> 20
                    ),
                );
                return 0;
            }
            state.blobs.insert(name, Blob::new(bytes));
            1
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Io;
    use crate::video::Process;

    #[test]
    fn an_outside_process_starts_only_what_the_user_chose() {
        let mut io = Io::default();
        io.programs.insert(String::from("p"));
        io.folders.insert(String::from("f"));
        io.defaults
            .insert(String::from("p"), String::from("/bin/sh"));
        let process = Process {
            program: String::from("p"),
            args: vec![String::from("serve")],
            dir: String::from("f"),
            path: vec![String::from("f")],
            ..Process::default()
        };
        assert!(launch(&io, &process).is_err());
        io.cfg
            .insert(String::from("p"), String::from("/opt/rife/zwtrt"));
        let folder = std::env::temp_dir().to_string_lossy().into_owned();
        io.cfg.insert(String::from("f"), folder.clone());
        let launched = launch(&io, &process).unwrap();
        assert_eq!(launched.program, "/opt/rife/zwtrt");
        assert_eq!(launched.args, ["serve"]);
        assert_eq!(launched.dir, folder);
        assert_eq!(launched.path, [folder]);
        io.cfg.insert(String::from("q"), String::from("/bin/sh"));
        for program in ["q", "f"] {
            let process = Process {
                program: String::from(program),
                ..Process::default()
            };
            assert!(launch(&io, &process).is_err(), "{program}");
        }
        let process = Process {
            program: String::from("p"),
            dir: String::from("q"),
            ..Process::default()
        };
        assert!(launch(&io, &process).is_err());
    }
}
