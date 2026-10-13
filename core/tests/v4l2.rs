// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! V4L2: what a board would say. The stateful half has met hardware (Raspberry Pi 2);
//! the stateless half is written from the kernel documentation. The spec checks live in the
//! modules' own tests; the board tests are `#[ignore]`:
//! `cargo test -p zerowave-core --features v4l2 --test v4l2 -- --ignored --nocapture`

#![cfg(all(feature = "v4l2", target_os = "linux"))]

use zwcore::decode::v4l2::{
    Flavour, Mode, ctrl, modifier, probe,
    uapi::{self, Timeval},
};
use zwcore::filters::cut::Cutout;

mod common;

/// The one that needs a board. Never fails; prints what is there.
#[test]
#[ignore = "needs a Raspberry Pi or a Rockchip board; prints what it found"]
fn say_what_the_board_has() {
    println!("{}", probe::report());
}

/// The stateless format is looked for first (Raspberry Pi 4: HEVC stateless, H.264 not).
#[test]
fn the_stateless_contract_is_preferred() {
    use mediadecoder::es::Codec;
    for codec in [Codec::Mpeg2, Codec::H264, Codec::Hevc] {
        let [(_, first), (_, second)] = probe::wanted(codec);
        assert_eq!(first, Flavour::Stateless);
        assert_eq!(second, Flavour::Stateful);
    }
}

/// Every control has a size, and the two HEVC spellings are different identifiers in
/// different classes (the wrong class reports "no stateless decoder" on a board with one).
#[test]
fn the_two_spellings_of_the_hevc_controls_are_different_controls() {
    let stateless = ctrl::Kind::HevcSps.id_for(ctrl::Naming::Stateless);
    let older = ctrl::Kind::HevcSps.id_for(ctrl::Naming::MpegVideo);
    assert_ne!(stateless, older);
    assert_eq!(stateless & 0xffff_0000, 0x00a4_0000, "the stateless class");
    assert_eq!(older & 0xffff_0000, 0x0099_0000, "the codec class");
    assert_eq!(ctrl::Kind::H264Sps.legacy(), None);
    assert_eq!(ctrl::Kind::Mpeg2Picture.legacy(), None);
}

/// A picture is named by its timestamp, carried through a `struct timeval` (microseconds).
#[test]
fn a_picture_is_named_by_a_number_the_kernel_can_carry() {
    for ns in [1_000_000u64, 2_000_000, 64_000_000, 3_600_000_000_000] {
        assert_eq!(Timeval::from_ns(ns).to_ns(), ns);
    }
}

/// The Broadcom SAND modifier carries the column height (`bytesperline`), not a row stride.
#[test]
fn the_sand_modifier_is_built_from_the_column_height() {
    let column = 1088 * 3 / 2;
    let value = uapi::sand128_col_height(column);
    assert_eq!(value >> 56, 0x07, "the Broadcom vendor");
    assert_eq!(value & 0xff, 4, "SAND128 rather than SAND32 or SAND64");
    assert_eq!((value >> 8) & 0x00ff_ffff, u64::from(column));
}

/// No entry for P030 on purpose: it is not a Vulkan format, so ten bit SAND is refused.
#[test]
fn ten_bit_sand_has_no_plane_wise_form() {
    assert_eq!(modifier::planes_of(false)[0].0, uapi::DRM_FORMAT_R8);
    assert_eq!(modifier::planes_of(true)[0].0, uapi::DRM_FORMAT_R16);
    assert_ne!(uapi::DRM_FORMAT_P030, uapi::DRM_FORMAT_P010);
}

/// A linear picture is never asked about (would make a build without Vulkan refuse VA-API).
#[test]
fn only_a_tiled_picture_is_asked_about() {
    assert!(modifier::require(uapi::DRM_FORMAT_MOD_LINEAR, false).is_ok());
    assert!(modifier::require(uapi::DRM_FORMAT_MOD_LINEAR, true).is_ok());
}

#[test]
fn both_ways_of_feeding_a_decoder_exist() {
    assert_ne!(Mode::Slice, Mode::Frame);
}

/// The deinterlacer on a device of its own, end to end. Needs the board.
#[test]
#[ignore = "needs a board with a deinterlacer"]
fn the_hardware_deinterlacer_doubles_and_says_so() {
    use mediadecoder::es::{Codec, Framing};
    use zwcore::decode::{DeinterlaceBy, Prefer};

    let name = "mpeg2-interlaced.ts";
    let bytes = common::testdata(name);
    let Some(units) = cut(&bytes) else {
        println!("{name} に映像がありません");
        return;
    };
    for by in [DeinterlaceBy::Software, DeinterlaceBy::Hardware] {
        let mut decoder =
            zwcore::decode::open_with(Codec::Mpeg2, Framing::StartCodes, Prefer::Platform, by);
        let began = std::time::Instant::now();
        let mut out = 0usize;
        let mut progressive = 0usize;
        for Cutout {
            tag,
            picture,
            format,
            data,
        } in &units
        {
            let _ = decoder.decode(*tag, picture, *format, data);
            while let Some(got) = decoder.take() {
                if got.about().progressive {
                    progressive += 1;
                }
                out += 1;
            }
        }
        // Timed in two halves, to tell a slow decoder from a drain at the end.
        let fed = began.elapsed();
        let during = out;
        let drain = std::time::Instant::now();
        decoder.flush();
        let flushed = drain.elapsed();
        while let Some(got) = decoder.take() {
            if got.about().progressive {
                progressive += 1;
            }
            out += 1;
        }
        let spent = began.elapsed().as_secs_f64();
        println!(
            "{by:?}: 入れた {} / 出た {out}（1枚あたり {:.2}）               progressive {progressive}  {:.1} fps               deinterlaces={} pictures_per_frame={}",
            units.len(),
            out as f64 / units.len() as f64,
            units.len() as f64 / spent,
            decoder.deinterlaces(),
            decoder.pictures_per_frame(),
        );
        println!(
            "   内訳: 投入 {:.2} 秒（その間に {during} 枚、{:.1} fps）  ドレイン {:.2} 秒  \
             ドレインで出た {} 枚",
            fed.as_secs_f64(),
            units.len() as f64 / fed.as_secs_f64(),
            flushed.as_secs_f64(),
            out - during,
        );
    }
}

/// デコーダーが渡したフレームを保持していても中身が変わらないか（参照を持っている間は使い回されない）。
/// 受け取ったときに輝度の指紋を取り、`hold` 枚あとに読み直して比べる。保持する数が
/// [`zwcore::decode::MOST_HELD`] を越えると、デコーダーは一番古いフレームを再利用するので変わってよい。
#[test]
#[ignore = "needs a board with a stateless decoder"]
fn a_held_picture_keeps_its_content() {
    use mediadecoder::es::{Codec, Framing};
    use zwcore::decode::{Decoded, Held, Prefer};

    let bytes = common::testdata("mpeg2-telecine.ts");
    let Some(units) = cut(&bytes) else {
        println!("映像がありません");
        return;
    };
    let units = &units[..units.len().min(400)];
    // 輝度プレーンのハッシュ（一行おきに 64 バイトおきに足す）。
    let print = |picture: &Decoded| -> Option<u64> {
        let Decoded::Surface(surface) = picture else {
            return None;
        };
        let Held::DmaBuf(dma_buf) = &surface.held else {
            return None;
        };
        let plane = dma_buf.planes.first()?;
        let length = plane.offset as usize + plane.stride as usize * dma_buf.height as usize;
        use std::os::fd::AsRawFd;
        // SAFETY: 読み取り専用のマップ。長さはプレーンの終わりまで。
        let at = unsafe {
            mmap(
                std::ptr::null_mut(),
                length,
                1, // PROT_READ
                1, // MAP_SHARED
                plane.fd.as_raw_fd(),
                0,
            )
        };
        if at as usize == usize::MAX {
            return None;
        }
        // SAFETY: マップしたばかりの `length` バイト。
        let data = unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) };
        let mut sum = 0u64;
        for y in (0..dma_buf.height as usize).step_by(2) {
            let row = plane.offset as usize + y * plane.stride as usize;
            for x in (0..dma_buf.width as usize).step_by(64) {
                sum = sum.wrapping_mul(31).wrapping_add(u64::from(data[row + x]));
            }
        }
        // SAFETY: 上でマップした物を解除する。
        unsafe { munmap(at, length) };
        Some(sum)
    };
    for hold in [8usize, 22, 30] {
        let mut decoder = zwcore::decode::open(Codec::Mpeg2, Framing::StartCodes, Prefer::Platform);
        let mut held: std::collections::VecDeque<(Decoded, u64)> = Default::default();
        let (mut checked, mut changed, mut unreadable) = (0usize, 0usize, 0usize);
        let mut first_changed = None;
        let mut take =
            |decoder: &mut Box<dyn zwcore::decode::Decode>,
             held: &mut std::collections::VecDeque<(Decoded, u64)>| {
                while let Some(got) = decoder.take() {
                    let Some(sum) = print(&got) else {
                        unreadable += 1;
                        continue;
                    };
                    held.push_back((got, sum));
                    while held.len() > hold {
                        let (old, sum) = held.pop_front().expect("just checked");
                        checked += 1;
                        if print(&old) != Some(sum) {
                            changed += 1;
                            first_changed.get_or_insert(checked);
                        }
                    }
                }
            };
        for Cutout {
            tag,
            picture,
            format,
            data,
        } in units
        {
            let _ = decoder.decode(*tag, picture, *format, data);
            take(&mut decoder, &mut held);
        }
        decoder.flush();
        take(&mut decoder, &mut held);
        println!(
            "保持 {hold:2} 枚：比べた {checked}、変わった {changed}（最初は {first_changed:?} 枚目）、読めない {unreadable}"
        );
    }
}

// 最後の `off_t` はポインターと同じ幅（`src/decode/mmap.rs` と同じ宣言）。
unsafe extern "C" {
    fn mmap(
        addr: *mut std::ffi::c_void,
        length: usize,
        prot: std::ffi::c_int,
        flags: std::ffi::c_int,
        fd: std::ffi::c_int,
        offset: isize,
    ) -> *mut std::ffi::c_void;
    fn munmap(addr: *mut std::ffi::c_void, length: usize) -> std::ffi::c_int;
}

fn cut(bytes: &[u8]) -> Option<Vec<Cutout>> {
    Some(
        zwcore::filters::cut::Cutting::of(bytes, zwcore::filters::Container::Ts, usize::MAX)?.units,
    )
}
