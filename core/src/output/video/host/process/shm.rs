// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `shm` の転送方式：メインメモリの共有（どの GPU とでも動くフォールバック）。本体がリードバックして書き込む。
//! Windows は名前の無いファイルマッピング、Linux はシールした memfd、ほかは名前を消した `shm_open`。

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use zwsdk::video::process::{self as wire, Format};

use super::super::Gpu;
use super::courier::Then;
use super::{Handed, Link, STUCK};

const IDLE: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

pub(super) struct Shm {
    inputs: Arc<[Mapping]>,
    middles: Vec<Mapping>,
    packed: Vec<wgpu::Buffer>,
    taken: Vec<wgpu::Buffer>,
    readbacks: Vec<Readback>,
    device: wgpu::Device,
    bytes: u64,
    stride: u32,
    value: u64,
}

impl Shm {
    pub fn new(
        device: &wgpu::Device,
        format: Format,
        size: [u32; 2],
        inputs: usize,
        middles: usize,
    ) -> Result<Self, String> {
        let stride = wire::stride(size[0], format);
        let bytes = wire::picture_bytes(size[1], stride);
        let maps = |count: usize| {
            (0..count)
                .map(|_| Mapping::new(bytes))
                .collect::<Result<Vec<_>, _>>()
        };
        let buffers = |count: usize, label, usage| -> Vec<wgpu::Buffer> {
            (0..count)
                .map(|_| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size: bytes,
                        usage,
                        mapped_at_creation: false,
                    })
                })
                .collect()
        };
        let readbacks = buffers(
            inputs,
            "zw process readback",
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        )
        .into_iter()
        .map(|buffer| Readback {
            buffer,
            state: Arc::new(AtomicU8::new(IDLE)),
        })
        .collect();
        Ok(Self {
            inputs: maps(inputs)?.into(),
            middles: maps(middles)?,
            packed: buffers(
                inputs,
                "zw process input",
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            taken: buffers(
                middles,
                "zw process mid",
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            readbacks,
            device: device.clone(),
            bytes,
            stride,
            value: 0,
        })
    }
}

impl super::Way for Shm {
    fn stride(&self) -> u32 {
        self.stride
    }

    fn input(&self, slot: usize) -> &wgpu::Buffer {
        &self.packed[slot]
    }

    fn output(&self, slot: usize) -> &wgpu::Buffer {
        &self.taken[slot]
    }

    fn share(&mut self, link: &mut Link) -> Result<(), String> {
        for (what, maps) in [
            (wire::INPUT, &self.inputs[..]),
            (wire::MIDDLE, &self.middles[..]),
        ] {
            for (index, map) in (0..).zip(maps) {
                #[cfg(windows)]
                link.share_handle(what, index, self.stride, self.bytes, map.handle)?;
                #[cfg(unix)]
                link.share_fd(
                    what,
                    index,
                    self.stride,
                    self.bytes,
                    std::os::fd::AsFd::as_fd(&map.fd),
                )?;
            }
        }
        Ok(())
    }

    fn hand_over(&mut self, gpu: &mut Gpu<'_>, packed: &[usize]) -> Result<Handed, String> {
        for &slot in packed {
            gpu.encoder.copy_buffer_to_buffer(
                &self.packed[slot],
                0,
                &self.readbacks[slot].buffer,
                0,
                self.bytes,
            );
        }
        let after = gpu.flush();
        let mut reading = Vec::new();
        for &slot in packed {
            let readback = &self.readbacks[slot];
            readback.state.store(MAPPING, Ordering::Release);
            let state = readback.state.clone();
            readback
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    state.store(
                        if result.is_ok() { MAPPED } else { FAILED },
                        Ordering::Release,
                    );
                });
            reading.push((slot, readback.buffer.clone(), readback.state.clone()));
        }
        let (device, inputs) = (self.device.clone(), self.inputs.clone());
        let then: Then = Box::new(move || copy_in(&device, &inputs, reading));
        self.value += 1;
        Ok(Handed {
            value: self.value,
            after: Some((after, Some(then))),
        })
    }

    fn done(&self, _value: u64) -> Option<bool> {
        None
    }

    fn take_back(&mut self, gpu: &mut Gpu<'_>, _value: u64, slot: usize) -> Result<(), String> {
        gpu.flush();
        gpu.queue
            .write_buffer(&self.taken[slot], 0, self.middles[slot].bytes());
        Ok(())
    }
}

fn copy_in(
    device: &wgpu::Device,
    inputs: &[Mapping],
    reading: Vec<(usize, wgpu::Buffer, Arc<AtomicU8>)>,
) -> Result<(), String> {
    let began = Instant::now();
    for (slot, buffer, state) in reading {
        while state.load(Ordering::Acquire) == MAPPING {
            if began.elapsed() > STUCK {
                return Err(String::from("入力フレームのリードバックがタイムアウト"));
            }
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_millis(100)),
            });
        }
        if state.load(Ordering::Acquire) == FAILED {
            return Err(String::from("入力フレームをリードバックできない"));
        }
        {
            let view = buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("リードバックのバッファを読めない：{e}"))?;
            inputs[slot].write(&view);
        }
        buffer.unmap();
        state.store(IDLE, Ordering::Release);
    }
    Ok(())
}

struct Mapping {
    pointer: *mut u8,
    len: usize,
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

// SAFETY: 一つのスロットに同時にアクセスするのは一つのスレッドだけ（相手が読み書きしているスロットにはアクセスしない）。
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    fn bytes(&self) -> &[u8] {
        // SAFETY: マップしている間は len バイト読める。
        unsafe { std::slice::from_raw_parts(self.pointer, self.len) }
    }

    fn write(&self, from: &[u8]) {
        let len = from.len().min(self.len);
        // SAFETY: マップしている間は len バイト書ける。このスロットに書くのはこのスレッドだけ。
        unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), self.pointer, len) }
    }

    #[cfg(windows)]
    fn new(size: u64) -> Result<Self, String> {
        use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE,
        };
        // SAFETY: 名前の無い、ページファイルで裏付けたマッピング。
        unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                windows::core::PCWSTR::null(),
            )
            .map_err(|e| format!("CreateFileMapping：{e}"))?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size as usize);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return Err(String::from("MapViewOfFile ができない"));
            }
            Ok(Self {
                pointer: view.Value.cast(),
                len: size as usize,
                handle,
            })
        }
    }

    /// Linux：memfd を縮められないようシールする（相手に縮められると SIGBUS でクラッシュする）。
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn new(size: u64) -> Result<Self, String> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // SAFETY: 名前は NUL 終端の定数。返った fd はすぐ OwnedFd が持つ。
        let fd = unsafe {
            let fd = libc::memfd_create(
                c"zw-shm".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            );
            if fd < 0 {
                return Err(format!("memfd_create：{}", std::io::Error::last_os_error()));
            }
            OwnedFd::from_raw_fd(fd)
        };
        let fd = grown(fd, size)?;
        // SAFETY: 伸ばした fd をシールしてからマップする。
        unsafe {
            if libc::fcntl(
                fd.as_raw_fd(),
                libc::F_ADD_SEALS,
                libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL,
            ) != 0
            {
                return Err(format!("F_ADD_SEALS：{}", std::io::Error::last_os_error()));
            }
            let pointer = libc::mmap(
                std::ptr::null_mut(),
                size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if pointer == libc::MAP_FAILED {
                return Err(format!("mmap：{}", std::io::Error::last_os_error()));
            }
            Ok(Self {
                pointer: pointer.cast(),
                len: size as usize,
                fd,
            })
        }
    }

    /// macOS など：shm_open して名前を消す（一度伸ばすと大きさを変えられない）。
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
    fn new(size: u64) -> Result<Self, String> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::sync::atomic::AtomicU32;
        static SERIAL: AtomicU32 = AtomicU32::new(0);
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let name = std::ffi::CString::new(format!("/zw-{}-{serial}", std::process::id()))
            .map_err(|e| e.to_string())?;
        #[cfg(target_vendor = "apple")]
        let mode = 0o600 as libc::c_uint;
        #[cfg(not(target_vendor = "apple"))]
        let mode = 0o600 as libc::mode_t;
        // SAFETY: 名前は今作った C の文字列。開いたらすぐ名前を消す（fd だけが残る）。
        let fd = unsafe {
            let fd = libc::shm_open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                mode,
            );
            if fd < 0 {
                return Err(format!("shm_open：{}", std::io::Error::last_os_error()));
            }
            libc::shm_unlink(name.as_ptr());
            OwnedFd::from_raw_fd(fd)
        };
        let fd = grown(fd, size)?;
        // SAFETY: 伸ばした fd をマップする。
        unsafe {
            let pointer = libc::mmap(
                std::ptr::null_mut(),
                size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if pointer == libc::MAP_FAILED {
                return Err(format!("mmap：{}", std::io::Error::last_os_error()));
            }
            Ok(Self {
                pointer: pointer.cast(),
                len: size as usize,
                fd,
            })
        }
    }
}

#[cfg(unix)]
fn grown(fd: std::os::fd::OwnedFd, size: u64) -> Result<std::os::fd::OwnedFd, String> {
    let file = std::fs::File::from(fd);
    file.set_len(size).map_err(|e| format!("ftruncate：{e}"))?;
    Ok(file.into())
}

impl Drop for Mapping {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: マップしたビューとハンドルを一度だけアンマップして閉じる。
        unsafe {
            use windows::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.pointer.cast(),
            });
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
        #[cfg(unix)]
        // SAFETY: マップした領域を一度だけアンマップする（fd は OwnedFd が閉じる）。
        unsafe {
            libc::munmap(self.pointer.cast(), self.len);
        }
    }
}
