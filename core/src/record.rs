// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 録画。そのまま、または必要なストリームだけに絞って書く（絞るのは TS は [`ts`]、MMT/TLV は [`mmt`]）。
//! 書き込みは別のスレッドで行い、読み取りは待たない。拡張子は最初のブロックで決める（`.ts` か `.mmts`）。

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use mpegts::{PACKET, SYNC};

mod mmt;
mod ts;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub service: Option<u16>,
    pub caption: bool,
    pub carousel: bool,
    pub eit: bool,
}

#[derive(Debug, Clone)]
pub struct State {
    pub path: PathBuf,
    pub started: Instant,
    pub written: u64,
}

#[derive(Debug, Clone)]
pub struct Done {
    pub path: PathBuf,
    pub written: u64,
    pub dropped: u64,
    pub failed: Option<String>,
}

/// 録画は一つのプロセスに一つ。読み取りのスレッドはこのフラグだけを見る（ロックを取らない）。
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SLOT: Mutex<Option<Running>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffers {
    pub write_unit: usize,
    pub max_pending: usize,
}

impl Default for Buffers {
    fn default() -> Self {
        Self {
            write_unit: 1 << 20,
            max_pending: 256 << 20,
        }
    }
}

const IDLE_WAIT: Duration = Duration::from_millis(10);

struct Queue {
    pending: Mutex<Pending>,
    ready: Condvar,
    max: usize,
    dropped: AtomicU64,
}

#[derive(Default)]
struct Pending {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    closed: bool,
}

enum Took {
    Chunk(Vec<u8>),
    Idle,
    Closed,
}

impl Queue {
    fn new(max: usize) -> Self {
        Self {
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
            max,
            dropped: AtomicU64::new(0),
        }
    }

    fn push(&self, bytes: &[u8]) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.closed {
            return;
        }
        pending.bytes += bytes.len();
        pending.chunks.push_back(bytes.to_vec());
        while pending.bytes > self.max && pending.chunks.len() > 1 {
            let Some(old) = pending.chunks.pop_front() else {
                break;
            };
            pending.bytes -= old.len();
            self.dropped.fetch_add(old.len() as u64, Ordering::Relaxed);
        }
        drop(pending);
        self.ready.notify_one();
    }

    fn take(&self, wait: Duration) -> Took {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.chunks.is_empty() && !pending.closed {
            pending = self
                .ready
                .wait_timeout(pending, wait)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        match pending.chunks.pop_front() {
            Some(chunk) => {
                pending.bytes -= chunk.len();
                Took::Chunk(chunk)
            }
            None if pending.closed => Took::Closed,
            None => Took::Idle,
        }
    }

    fn close(&self) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        self.ready.notify_all();
    }

    fn is_closed(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed
    }
}

struct Running {
    queue: Arc<Queue>,
    path: Arc<Mutex<PathBuf>>,
    started: Instant,
    written: Arc<AtomicU64>,
    failed: Arc<Mutex<Option<String>>>,
    told: AtomicBool,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub fn start(stem: &Path, options: Options, buffers: Buffers) -> Result<(), String> {
    let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return Err(String::from("もう録画しています"));
    }
    if let Some(folder) = stem.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("{} を作成できません: {error}", folder.display()))?;
    }
    let path = Arc::new(Mutex::new(stem.to_path_buf()));
    let queue = Arc::new(Queue::new(buffers.max_pending.max(1)));
    let written = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(Mutex::new(None));
    let thread = {
        let queue = Arc::clone(&queue);
        let written = Arc::clone(&written);
        let failed = Arc::clone(&failed);
        let path = Arc::clone(&path);
        let unit = buffers.write_unit.max(1024);
        std::thread::Builder::new()
            .name(String::from("record"))
            .spawn(move || write(&queue, &path, options, unit, &written, &failed))
            .map_err(|error| format!("録画のスレッドを起動できません: {error}"))?
    };
    *slot = Some(Running {
        queue,
        path,
        started: Instant::now(),
        written,
        failed,
        told: AtomicBool::new(false),
        thread: Some(thread),
    });
    ACTIVE.store(true, Ordering::Release);
    Ok(())
}

pub fn stop() -> Option<Done> {
    let mut running = SLOT.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    ACTIVE.store(false, Ordering::Release);
    running.queue.close();
    if let Some(thread) = running.thread.take() {
        let _ = thread.join();
    }
    let failed = running
        .failed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    Some(Done {
        path: running
            .path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        written: running.written.load(Ordering::Relaxed),
        dropped: running.queue.dropped.load(Ordering::Relaxed),
        failed,
    })
}

pub fn state() -> Option<State> {
    let slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref().map(|running| State {
        path: running
            .path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        started: running.started,
        written: running.written.load(Ordering::Relaxed),
    })
}

pub fn failure() -> Option<String> {
    let slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let running = slot.as_ref()?;
    let why = running.failed.lock().ok()?.clone()?;
    (!running.told.swap(true, Ordering::Relaxed)).then_some(why)
}

pub fn ended() -> bool {
    let slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref()
        .and_then(|running| running.thread.as_ref())
        .is_some_and(std::thread::JoinHandle::is_finished)
}

pub fn feed(bytes: &[u8]) {
    if bytes.is_empty() || !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(running) = slot.as_ref() {
        running.queue.push(bytes);
    }
}

fn write(
    queue: &Queue,
    path: &Mutex<PathBuf>,
    options: Options,
    unit: usize,
    written: &AtomicU64,
    failed: &Mutex<Option<String>>,
) {
    let fail = |why: String| {
        let mut failed = failed.lock().unwrap_or_else(|e| e.into_inner());
        if failed.is_none() {
            *failed = Some(why);
        }
    };
    let mut out: Option<std::fs::File> = None;
    let mut cache: Vec<u8> = Vec::with_capacity(unit);
    let mut filter = ts::Filter::new(options);
    let mut tlv = mmt::Filter::new(options);
    let mut kind: Option<bool> = None;
    let mut pending = Vec::new();
    loop {
        if cache.len() >= unit
            && let Some(file) = out.as_mut()
            && let Err(error) = write_out(file, &mut cache, written)
        {
            fail(error.to_string());
            if queue.is_closed() {
                return;
            }
            std::thread::sleep(IDLE_WAIT);
            continue;
        }
        let chunk = match queue.take(IDLE_WAIT) {
            Took::Chunk(chunk) => chunk,
            Took::Idle => continue,
            Took::Closed => break,
        };
        let is_ts = *kind.get_or_insert_with(|| looks_like_ts(&chunk));
        if out.is_none() {
            let named = {
                let path = path.lock().unwrap_or_else(|e| e.into_inner());
                let mut named = path.clone().into_os_string();
                named.push(if is_ts { ".ts" } else { ".mmts" });
                PathBuf::from(named)
            };
            match create_unused(&named) {
                Ok((used, file)) => {
                    *path.lock().unwrap_or_else(|e| e.into_inner()) = used;
                    out = Some(file);
                }
                Err(error) => {
                    fail(format!("{} に書き込めません: {error}", named.display()));
                    return;
                }
            }
        }
        pending.clear();
        if is_ts {
            filter.push(&chunk, &mut pending);
        } else {
            tlv.push(&chunk, &mut pending);
        }
        cache.extend_from_slice(&pending);
    }
    if let Some(file) = out.as_mut()
        && let Err(error) = write_out(file, &mut cache, written)
    {
        fail(error.to_string());
    }
}

fn write_out(
    file: &mut std::fs::File,
    cache: &mut Vec<u8>,
    written: &AtomicU64,
) -> std::io::Result<()> {
    while !cache.is_empty() {
        match file.write(cache) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(wrote) => {
                cache.drain(..wrote);
                written.fetch_add(wrote as u64, Ordering::Relaxed);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn looks_like_ts(bytes: &[u8]) -> bool {
    let Some(first) = bytes.iter().position(|byte| *byte == SYNC) else {
        return false;
    };
    (0..3).all(|nth| bytes.get(first + nth * PACKET) == Some(&SYNC))
}

/// 既にある物は上書きせず、`名前 (1).拡張子`…と空いている名前で作る。
pub fn create_unused(path: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    let stem = path.file_stem().unwrap_or_default().to_os_string();
    let extension = path.extension().map(|extension| extension.to_os_string());
    for number in 0u32.. {
        let candidate = if number == 0 {
            path.to_path_buf()
        } else {
            let mut name = stem.clone();
            name.push(format!(" ({number})"));
            if let Some(extension) = &extension {
                name.push(".");
                name.push(extension);
            }
            path.with_file_name(name)
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

pub fn without_extension(name: &str) -> &str {
    for extension in [".ts", ".mmts", ".m2ts", ".tlv"] {
        if let Some(stem) = name.strip_suffix(extension) {
            return stem;
        }
    }
    name
}

pub fn file_name(pattern: &str, channel: &str, event: &str, offset_hours: i32) -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
        + i64::from(offset_hours) * 3600;
    let (year, month, day, hour, minute, second) = crate::guide::civil(now);
    let clean = |text: &str| -> String {
        text.chars()
            .map(|c| match c {
                '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                c if c.is_control() => '_',
                c => c,
            })
            .collect()
    };
    let named = pattern
        .replace("{date}", &format!("{year:04}{month:02}{day:02}"))
        .replace("{time}", &format!("{hour:02}{minute:02}{second:02}"))
        .replace("{channel}", &clean(channel))
        .replace("{event}", &clean(event));
    tidy(&named)
}

/// 多くのファイルシステムの 255 に、`名前 (2)` の分を残す。
const NAME_MOST: usize = 240;

/// どの OS でも作れる形に整える。`.`・`..` は `_` に（上のフォルダーへ出ない）、末尾のピリオドと
/// 空白は削り、Windows の予約名は前に `_` を足し、長さは文字の境目で切る。
fn tidy(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut part = String::new();
    for c in name.chars().chain(std::iter::once('\0')) {
        if matches!(c, '/' | '\\' | '\0') {
            if !part.is_empty() {
                out.push_str(&tidy_part(&part));
                part.clear();
            }
            if c != '\0' {
                out.push(c);
            }
        } else {
            part.push(c);
        }
    }
    out
}

fn tidy_part(part: &str) -> String {
    let trimmed = part.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return String::from("_");
    }
    let (stem, extension) = match trimmed.rfind('.') {
        Some(at) if at > 0 && trimmed.len() - at <= 8 => trimmed.split_at(at),
        _ => (trimmed, ""),
    };
    let room = NAME_MOST.saturating_sub(extension.len());
    let mut cut = stem.len().min(room);
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    let stem = stem[..cut].trim_end_matches(['.', ' ']);
    let reserved = |word: &str| {
        let word = word.to_ascii_uppercase();
        matches!(word.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (word.len() == 4
                && (word.starts_with("COM") || word.starts_with("LPT"))
                && word.as_bytes()[3].is_ascii_digit())
    };
    // 予約名は拡張子が付いていても予約名（`NUL.ts`）。
    let base = stem.split('.').next().unwrap_or(stem);
    let prefix = if stem.is_empty() || reserved(base) {
        "_"
    } else {
        ""
    };
    format!("{prefix}{stem}{extension}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Caption,
    Carousel,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_oldest_pending_data_is_dropped_first() {
        let queue = Queue::new(10);
        for byte in 1..=4u8 {
            queue.push(&[byte; 4]);
        }
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 8);
        let Took::Chunk(first) = queue.take(Duration::ZERO) else {
            panic!("残っているはず");
        };
        assert_eq!(first, [3; 4]);
        queue.close();
        assert!(matches!(queue.take(Duration::ZERO), Took::Chunk(_)));
        assert!(matches!(queue.take(Duration::ZERO), Took::Closed));
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let folder = std::env::temp_dir().join(format!("zw-unused-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("番組.mmts");
        std::fs::write(&path, b"old").unwrap();
        let (first, _) = create_unused(&path).unwrap();
        let (second, _) = create_unused(&path).unwrap();
        assert_eq!(first, folder.join("番組 (1).mmts"));
        assert_eq!(second, folder.join("番組 (2).mmts"));
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn file_names_fill_in_and_clean_up() {
        let named = file_name("{channel}_{event}.ts", "BS/局", "番組:前編", 9);
        assert_eq!(named, "BS_局_番組_前編.ts");
        assert!(file_name("{date}", "", "", 9).len() == 8);
    }

    #[test]
    fn file_names_are_ones_every_system_can_make() {
        assert_eq!(
            file_name("{date}/{event}/a.ts", "", "..", 9)[8..],
            *"/_/a.ts"
        );
        assert_eq!(file_name("{event}.ts", "", "nul", 9), "_nul.ts");
        assert_eq!(file_name("{event}", "", "COM3", 9), "_COM3");
        assert_eq!(file_name("{event}", "", "番組. ", 9), "番組");
        assert_eq!(file_name("{event}.ts", "", "CONTEST", 9), "CONTEST.ts");
        let long = "あ".repeat(200);
        let named = file_name("{event}.ts", "", &long, 9);
        assert!(named.len() <= 240, "{}", named.len());
        assert!(named.ends_with("あ.ts"), "{named}");
    }

    #[test]
    fn written_extensions_are_dropped() {
        assert_eq!(without_extension("a_b.ts"), "a_b");
        assert_eq!(without_extension("a_b.mmts"), "a_b");
        assert_eq!(without_extension("番組.前編"), "番組.前編");
    }

    #[test]
    fn the_extension_follows_what_arrives() {
        let folder = std::env::temp_dir().join(format!("zw-record-{}", std::process::id()));
        let options = Options {
            service: None,
            caption: true,
            carousel: true,
            eit: true,
        };
        start(&folder.join("tlv"), options, Buffers::default()).expect("始まる");
        feed(&[0x7f; 600]);
        let done = stop().expect("止まる");
        assert_eq!(done.path, folder.join("tlv.mmts"));
        assert_eq!(
            std::fs::metadata(&done.path).map(|m| m.len()).ok(),
            Some(600)
        );
        let mut ts = vec![0u8; PACKET * 3];
        for nth in 0..3 {
            ts[nth * PACKET] = SYNC;
        }
        start(&folder.join("ts"), options, Buffers::default()).expect("始まる");
        feed(&ts);
        let done = stop().expect("止まる");
        assert_eq!(done.path, folder.join("ts.ts"));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn transport_streams_are_told_from_other_containers() {
        let mut ts = vec![0u8; PACKET * 3];
        for nth in 0..3 {
            ts[nth * PACKET] = SYNC;
        }
        assert!(looks_like_ts(&ts));
        assert!(!looks_like_ts(&[0x7f; 600]));
    }
}
