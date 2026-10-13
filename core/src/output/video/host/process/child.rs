// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 起動したプロセスと通信チャネル（[`Link`]）。Windows はパイプ二本、Unix は unix ソケット一本。
//! 本体が落ちても子が残らないよう、Windows はジョブ、Linux は `PR_SET_PDEATHSIG` を使う。

use std::io::{BufRead, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zwsdk::video as zv;
use zwsdk::video::process as wire;

const LINGER: Duration = Duration::from_secs(2);

const REPLY_MOST: usize = 1 << 20;

pub(super) type Status = (Option<f32>, String);
pub(super) type Said = Arc<Mutex<Option<Status>>>;

type Reply = Result<Vec<u8>, String>;

#[cfg(windows)]
type Out = std::process::ChildStdin;
#[cfg(unix)]
type Out = std::os::unix::net::UnixStream;

#[derive(Clone)]
pub(super) struct Writer {
    name: String,
    out: Arc<Mutex<Option<Out>>>,
}

impl Writer {
    /// ヘッダーと本体を続けて送る（ほかのスレッドのメッセージと混ぜない）。
    pub fn send(&self, op: u32, body: &[u8]) -> Result<(), String> {
        let message = wire::request(op, body);
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        match out.as_mut().map(|w| w.write_all(&message)) {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(format!("{} へ送れない：{e}", self.name)),
            None => Err(format!("{} への接続が閉じている", self.name)),
        }
    }
}

pub(super) struct Link {
    pub name: String,
    child: Child,
    writer: Writer,
    replies: Receiver<Reply>,
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    pub port: Option<u32>,
}

impl Link {
    pub fn spawn(spec: &zv::Launch, said: &Said, env: &[(&str, &str)]) -> Result<Self, String> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args).stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        if !spec.dir.is_empty() {
            command.current_dir(&spec.dir);
        }
        if !spec.path.is_empty() {
            let mut dirs: Vec<std::path::PathBuf> = spec.path.iter().map(Into::into).collect();
            if let Some(path) = std::env::var_os("PATH") {
                dirs.extend(std::env::split_paths(&path));
            }
            command.env("PATH", std::env::join_paths(dirs).unwrap_or_default());
        }
        let name = std::path::Path::new(&spec.program).file_stem().map_or_else(
            || spec.program.clone(),
            |s| s.to_string_lossy().into_owned(),
        );
        let failed = |e: std::io::Error| format!("{} を起動できない：{e}", spec.program);

        #[cfg(windows)]
        let (child, reader, out) = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW);
            let mut child = command.spawn().map_err(failed)?;
            bind_to_us(&child);
            let stdout = child.stdout.take().ok_or("標準出力が無い")?;
            let stdin = child.stdin.take();
            (child, Box::new(stdout) as Box<dyn Read + Send>, stdin)
        };
        #[cfg(unix)]
        let (child, reader, out) = {
            use std::os::fd::OwnedFd;
            use std::os::unix::net::UnixStream;
            let (ours, theirs) =
                UnixStream::pair().map_err(|e| format!("ソケットを作れない：{e}"))?;
            let other = theirs
                .try_clone()
                .map_err(|e| format!("ソケットを複製できない：{e}"))?;
            command
                .stdin(Stdio::from(OwnedFd::from(theirs)))
                .stdout(Stdio::from(OwnedFd::from(other)));
            let child = spawn_bound(command).map_err(failed)?;
            let reader = ours
                .try_clone()
                .map_err(|e| format!("ソケットを複製できない：{e}"))?;
            (child, Box::new(reader) as Box<dyn Read + Send>, Some(ours))
        };
        let mut child = child;
        if let Some(stderr) = child.stderr.take() {
            let (who, said) = (name.clone(), said.clone());
            let _ = std::thread::Builder::new()
                .name(format!("{name} stderr"))
                .spawn(move || {
                    for line in std::io::BufReader::new(stderr)
                        .lines()
                        .map_while(Result::ok)
                    {
                        match wire::status_line(&line) {
                            Some(status) => {
                                if let Ok(mut said) = said.lock() {
                                    *said = Some(status);
                                }
                            }
                            None => tracing::info!("{who}: {line}"),
                        }
                    }
                });
        }
        let (sender, replies) = mpsc::channel();
        let who = name.clone();
        std::thread::Builder::new()
            .name(format!("{name} replies"))
            .spawn(move || read_replies(reader, &who, &sender))
            .map_err(|e| format!("スレッドを作れない：{e}"))?;
        Ok(Self {
            writer: Writer {
                name: name.clone(),
                out: Arc::new(Mutex::new(out)),
            },
            name,
            child,
            replies,
            #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
            port: None,
        })
    }

    pub fn send(&mut self, op: u32, body: &[u8]) -> Result<(), String> {
        self.writer.send(op, body)
    }

    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    pub fn wait(&mut self, limit: Option<Duration>) -> Reply {
        let got = match limit {
            Some(limit) => self.replies.recv_timeout(limit).map_err(|e| match e {
                RecvTimeoutError::Timeout => {
                    format!("{} が {} 秒たっても応答しない", self.name, limit.as_secs())
                }
                RecvTimeoutError::Disconnected => format!("{} の応答が無い", self.name),
            }),
            None => self
                .replies
                .recv()
                .map_err(|_| format!("{} の応答が無い", self.name)),
        };
        got?
    }

    /// `cancel` が立つか `limit` を過ぎたらやめる（OPEN は準備に何分もかかる）。
    pub fn wait_cancellable(
        &mut self,
        limit: Duration,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Reply {
        let deadline = Instant::now() + limit;
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(format!("{} の準備を取りやめた", self.name));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "{} が {} 分たっても準備できない",
                    self.name,
                    limit.as_secs() / 60
                ));
            }
            match self
                .replies
                .recv_timeout((deadline - now).min(Duration::from_millis(200)))
            {
                Ok(reply) => return reply,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("{} の応答が無い", self.name));
                }
            }
        }
    }

    #[cfg(any(windows, all(feature = "videotoolbox", target_vendor = "apple")))]
    pub fn call(&mut self, op: u32, body: &[u8], limit: Option<Duration>) -> Reply {
        self.send(op, body)?;
        self.wait(limit).map_err(|why| match self.dead() {
            Some(dead) => format!("{why}（{dead}）"),
            None => why,
        })
    }

    pub fn try_reply(&mut self) -> Option<Reply> {
        match self.replies.try_recv() {
            Ok(reply) => Some(reply),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(format!("{} の応答が無い", self.name))),
        }
    }

    pub fn dead(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(format!("{} が終わった（{status}）", self.name)),
            Ok(None) => None,
            Err(e) => Some(format!("{} の状態が分からない：{e}", self.name)),
        }
    }

    #[cfg(any(windows, all(feature = "videotoolbox", target_vendor = "apple")))]
    pub fn share(&mut self, share: wire::Share) -> Result<(), String> {
        self.call(wire::SHARE, &share.encode(), Some(LINGER * 5))
            .map(drop)
    }

    /// ハンドルを相手のプロセスへ複製して SHARE で渡す（こちらのハンドルは呼び出し側が閉じる）。
    #[cfg(windows)]
    pub fn share_handle(
        &mut self,
        what: u32,
        index: u32,
        stride: u32,
        size: u64,
        handle: windows::Win32::Foundation::HANDLE,
    ) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut theirs = HANDLE::default();
        // SAFETY: 生きている二つのプロセスと、こちらのハンドル。
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                handle,
                HANDLE(self.child.as_raw_handle()),
                &mut theirs,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .map_err(|e| format!("DuplicateHandle：{e}"))?;
        self.share(wire::Share {
            what,
            index,
            stride,
            size,
            handle: theirs.0 as u64,
        })
    }

    /// fd を付けて SHARE で渡す（こちらの fd は呼び出し側が閉じる）。
    #[cfg(unix)]
    pub fn share_fd(
        &mut self,
        what: u32,
        index: u32,
        stride: u32,
        size: u64,
        fd: std::os::fd::BorrowedFd<'_>,
    ) -> Result<(), String> {
        let message = wire::request(
            wire::SHARE,
            &wire::Share {
                what,
                index,
                stride,
                size,
                handle: 0,
            }
            .encode(),
        );
        {
            let out = self.writer.out.lock().unwrap_or_else(|e| e.into_inner());
            let socket = out
                .as_ref()
                .ok_or_else(|| format!("{} への接続が閉じている", self.name))?;
            send_with_fd(socket, &message, fd)
                .map_err(|e| format!("{} へ fd を送れない：{e}", self.name))?;
        }
        self.wait(Some(LINGER * 5)).map(drop)
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        // 接続を閉じると相手はインポートしたリソースを解放して終了する。それを待ってからバッファを破棄する。
        let out = self
            .writer
            .out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        #[cfg(unix)]
        if let Some(socket) = &out {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        drop(out);
        #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
        if let Some(port) = self.port.take() {
            super::metal::release_port(port);
        }
        let deadline = Instant::now() + LINGER;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_replies(mut reader: Box<dyn Read + Send>, who: &str, sender: &mpsc::Sender<Reply>) {
    loop {
        let mut head = [0u8; 8];
        if let Err(e) = reader.read_exact(&mut head) {
            let _ = sender.send(Err(format!(
                "{who} の応答が無い（クラッシュしたかもしれない）：{e}"
            )));
            return;
        }
        let (status, len) = wire::head(head);
        if len > REPLY_MOST {
            let _ = sender.send(Err(format!("{who} の応答が長すぎる（{len} バイト）")));
            return;
        }
        let mut body = vec![0u8; len];
        if let Err(e) = reader.read_exact(&mut body) {
            let _ = sender.send(Err(format!("{who} の応答が途中で切れた：{e}")));
            return;
        }
        let reply = if status == 0 {
            Ok(body)
        } else {
            Err(format!("{who}：{}", String::from_utf8_lossy(&body)))
        };
        if sender.send(reply).is_err() {
            return;
        }
    }
}

#[cfg(unix)]
fn send_with_fd(
    socket: &std::os::unix::net::UnixStream,
    message: &[u8],
    fd: std::os::fd::BorrowedFd<'_>,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let raw = fd.as_raw_fd();
    // SAFETY: ローカルのバッファだけを指す msghdr。
    let sent = unsafe {
        let mut control = [0u64; 8];
        let space = libc::CMSG_SPACE(size_of::<libc::c_int>() as u32) as usize;
        let mut iov = libc::iovec {
            iov_base: message.as_ptr().cast_mut().cast(),
            iov_len: message.len(),
        };
        let mut header: libc::msghdr = std::mem::zeroed();
        header.msg_iov = &raw mut iov;
        header.msg_iovlen = 1;
        header.msg_control = control.as_mut_ptr().cast();
        header.msg_controllen = space as _;
        let cmsg = libc::CMSG_FIRSTHDR(&raw const header);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<libc::c_int>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::c_int>(), raw);
        libc::sendmsg(socket.as_raw_fd(), &raw const header, 0)
    };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let sent = sent as usize;
    if sent < message.len() {
        (&*socket).write_all(&message[sent..])?;
    }
    Ok(())
}

/// Linux：ずっと生きているスレッドから起動する（`PR_SET_PDEATHSIG` は起動したスレッドの終了で効く）。
#[cfg(target_os = "linux")]
fn spawn_bound(mut command: Command) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    type Job = (Command, mpsc::Sender<std::io::Result<Child>>);
    static SPAWNER: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: fork のあと exec の前に、子の側で非同期シグナル安全な呼び出しだけをする。
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    let (sender, receiver) = mpsc::channel();
    {
        let mut spawner = SPAWNER.lock().unwrap_or_else(|e| e.into_inner());
        if spawner.is_none() {
            let (jobs, incoming) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name(String::from("zw process spawner"))
                .spawn(move || {
                    for (mut command, answer) in incoming {
                        let _ = answer.send(command.spawn());
                    }
                })?;
            *spawner = Some(jobs);
        }
        if let Some(jobs) = spawner.as_ref() {
            jobs.send((command, sender))
                .map_err(|_| std::io::Error::other("起動用のスレッドが無い"))?;
        }
    }
    receiver
        .recv()
        .map_err(|_| std::io::Error::other("起動用のスレッドが応答しない"))?
}

#[cfg(all(unix, not(target_os = "linux")))]
fn spawn_bound(mut command: Command) -> std::io::Result<Child> {
    command.spawn()
}

/// 本体が終了すれば OS が終了させるジョブに入れる（準備の最中は接続が切れても相手が気づかない）。
#[cfg(windows)]
fn bind_to_us(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    static JOB: std::sync::OnceLock<Option<isize>> = std::sync::OnceLock::new();
    let job = JOB.get_or_init(|| {
        // SAFETY: 名前の無いジョブを作り、閉じたら中のプロセスを終了させる設定を付ける。
        unsafe {
            let job = CreateJobObjectW(None, None).ok()?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .ok()?;
            Some(job.0 as isize)
        }
    });
    if let Some(job) = job {
        // SAFETY: 生きているジョブと、いま起動したプロセスのハンドル。
        let assigned = unsafe {
            AssignProcessToJobObject(
                HANDLE(*job as *mut core::ffi::c_void),
                HANDLE(child.as_raw_handle()),
            )
        };
        if let Err(why) = assigned {
            tracing::warn!(
                "外部プロセスの補間: ジョブオブジェクトに登録できない（本体と一緒に終了しない）: {why}"
            );
        }
    }
}
