// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

/// ファイル末尾への追記を待つ時間。録画中のファイルは一秒に何度も書き込まれるので、これだけ止まれば終わりとみなす。
pub const PATIENCE: Duration = Duration::from_secs(3);

const NAP: Duration = Duration::from_millis(200);

pub const STALE: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct Growing {
    file: File,
    patient: bool,
    at_end_since: Option<Instant>,
}

impl Growing {
    pub fn new(file: File) -> Self {
        let patient = file
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_none_or(|age| age < STALE);
        Self {
            file,
            patient,
            at_end_since: None,
        }
    }
}

impl Read for Growing {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.file.read(into)?;
            if read > 0 {
                self.at_end_since = None;
                return Ok(read);
            }
            if !self.patient {
                return Ok(0);
            }
            let since = *self.at_end_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= PATIENCE {
                return Ok(0);
            }
            std::thread::sleep(NAP);
        }
    }
}

impl Seek for Growing {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.at_end_since = None;
        self.file.seek(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_what_is_appended_while_waiting() {
        let dir = std::env::temp_dir().join(format!("zw-growing-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("a.ts");
        std::fs::write(&path, b"abc").unwrap();
        let mut growing = Growing::new(File::open(&path).unwrap());
        let mut buffer = [0u8; 8];
        assert_eq!(growing.read(&mut buffer).unwrap(), 3);
        let writer = path.clone();
        let appending = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(writer)
                .unwrap();
            file.write_all(b"de").unwrap();
        });
        assert_eq!(growing.read(&mut buffer).unwrap(), 2, "追記された分を読む");
        appending.join().unwrap();
        let began = Instant::now();
        assert_eq!(growing.read(&mut buffer).unwrap(), 0);
        assert!(began.elapsed() >= PATIENCE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
