// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! キャプチャの PNG を並列に作る。ストリップごとに deflate して同期フラッシュでつなげる。

use std::ops::Range;

const MIN_ROWS: usize = 64;

pub fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(16)
}

pub fn rows_in_parallel<T: Send>(
    out: &mut [T],
    row_len: usize,
    height: usize,
    work: impl Fn(Range<usize>, &mut [T]) + Sync,
) {
    let bands = threads().min(height.div_ceil(MIN_ROWS)).max(1);
    let per = height.div_ceil(bands);
    std::thread::scope(|scope| {
        for (nth, chunk) in out.chunks_mut(per * row_len).enumerate() {
            let rows = nth * per..(nth * per + chunk.len() / row_len.max(1)).min(height);
            let work = &work;
            scope.spawn(move || work(rows, chunk));
        }
    });
}

/// RGB の画素（8 ビットか 16 ビットのビッグエンディアン）を PNG に。`level` は 0〜9。
pub fn png(
    pixels: &[u8],
    (width, height): (u32, u32),
    sixteen: bool,
    level: u8,
    cicp: Option<[u8; 4]>,
) -> Vec<u8> {
    let bpp = if sixteen { 6 } else { 3 };
    let row = width as usize * bpp;
    let height = height as usize;
    let rows = pixels.len() / row.max(1);
    let height = height.min(rows);
    let bands = threads().min(height.div_ceil(MIN_ROWS)).max(1);
    let per = height.div_ceil(bands);
    let pieces: Vec<(Vec<u8>, u32, usize)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..bands)
            .map(|nth| {
                let rows = nth * per..((nth + 1) * per).min(height);
                let last = rows.end == height;
                scope.spawn(move || band(pixels, row, bpp, rows, level, last))
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .expect("PNG のストリップのスレッドがパニックした")
            })
            .collect()
    });
    let mut adler = 1u32;
    let mut stream =
        Vec::with_capacity(pieces.iter().map(|(data, ..)| data.len()).sum::<usize>() + 6);
    stream.extend_from_slice(&zlib_header(level));
    for (data, sum, length) in &pieces {
        stream.extend_from_slice(data);
        adler = adler32_combine(adler, *sum, *length);
    }
    stream.extend_from_slice(&adler.to_be_bytes());

    let mut out = Vec::with_capacity(stream.len() + 1024);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[if sixteen { 16 } else { 8 }, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    if let Some(cicp) = cicp {
        chunk(&mut out, b"cICP", &cicp);
    }
    for part in stream.chunks(8 << 20) {
        chunk(&mut out, b"IDAT", part);
    }
    chunk(&mut out, b"IEND", &[]);
    out
}

fn band(
    pixels: &[u8],
    row: usize,
    bpp: usize,
    rows: Range<usize>,
    level: u8,
    last: bool,
) -> (Vec<u8>, u32, usize) {
    let mut filtered = Vec::with_capacity(rows.len() * (row + 1));
    let mut candidate = vec![0u8; row];
    let mut best = vec![0u8; row];
    for y in rows {
        let line = &pixels[y * row..(y + 1) * row];
        let above = (y > 0).then(|| &pixels[(y - 1) * row..y * row]);
        if level == 0 {
            filtered.push(0);
            filtered.extend_from_slice(line);
            continue;
        }
        let mut best_kind = 0u8;
        let mut best_cost = u64::MAX;
        for kind in 0..5u8 {
            apply(kind, line, above, bpp, &mut candidate);
            let cost: u64 = candidate
                .iter()
                .map(|&byte| u64::from((byte as i8).unsigned_abs()))
                .sum();
            if cost < best_cost {
                best_cost = cost;
                best_kind = kind;
                std::mem::swap(&mut best, &mut candidate);
            }
        }
        filtered.push(best_kind);
        filtered.extend_from_slice(&best);
    }
    let sum = {
        let mut adler = adler2::Adler32::new();
        adler.write_slice(&filtered);
        adler.checksum()
    };
    let mut compress =
        flate2::Compress::new(flate2::Compression::new(u32::from(level.min(9))), false);
    let mut out = Vec::with_capacity(filtered.len() / 2 + 64);
    let flush = if last {
        flate2::FlushCompress::Finish
    } else {
        flate2::FlushCompress::Sync
    };
    loop {
        let read = compress.total_in() as usize;
        out.reserve(64 << 10);
        let status = compress
            .compress_vec(&filtered[read..], &mut out, flush)
            .expect("deflate が失敗した");
        let done = compress.total_in() as usize == filtered.len();
        match status {
            flate2::Status::StreamEnd => break,
            _ if done && !last && out.len() < out.capacity() => break,
            _ => {}
        }
    }
    (out, sum, filtered.len())
}

fn apply(kind: u8, line: &[u8], above: Option<&[u8]>, bpp: usize, out: &mut [u8]) {
    for at in 0..line.len() {
        let x = line[at];
        let a = if at >= bpp { line[at - bpp] } else { 0 };
        let b = above.map_or(0, |above| above[at]);
        let c = if at >= bpp {
            above.map_or(0, |above| above[at - bpp])
        } else {
            0
        };
        out[at] = match kind {
            0 => x,
            1 => x.wrapping_sub(a),
            2 => x.wrapping_sub(b),
            3 => x.wrapping_sub(((u16::from(a) + u16::from(b)) / 2) as u8),
            _ => x.wrapping_sub(paeth(a, b, c)),
        };
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let pa = (p - i16::from(a)).abs();
    let pb = (p - i16::from(b)).abs();
    let pc = (p - i16::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn zlib_header(level: u8) -> [u8; 2] {
    let cmf = 0x78u8;
    let flevel: u8 = match level {
        0 | 1 => 0,
        2..=5 => 1,
        6 => 2,
        _ => 3,
    };
    let flg = flevel << 6;
    let check = 31 - ((u16::from(cmf) * 256 + u16::from(flg)) % 31) as u8;
    [cmf, flg | (check % 31)]
}

/// 二つの続いたバイト列の adler32 をつなぐ（zlib の `adler32_combine`）。
fn adler32_combine(first: u32, second: u32, second_len: usize) -> u32 {
    const BASE: u64 = 65_521;
    let rem = (second_len as u64) % BASE;
    let a1 = u64::from(first & 0xffff);
    let b1 = u64::from(first >> 16);
    let a2 = u64::from(second & 0xffff);
    let b2 = u64::from(second >> 16);
    let mut sum1 = a1 + a2 + BASE - 1;
    let mut sum2 = (rem * a1) % BASE + b1 + b2 + BASE - rem;
    sum1 %= BASE;
    sum1 %= BASE;
    sum2 %= BASE;
    sum2 %= BASE;
    ((sum2 as u32) << 16) | sum1 as u32
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("ヘッダーが読める");
        let mut out = vec![0; reader.output_buffer_size().expect("大きさ")];
        let info = reader.next_frame(&mut out).expect("画像が読める");
        out.truncate(info.buffer_size());
        (info, out)
    }

    #[test]
    fn parallel_pngs_read_back_exactly() {
        let (width, height) = (97u32, 300u32);
        let eight: Vec<u8> = (0..width * height * 3)
            .map(|at| ((at * 7 + at / 291 * 13) % 251) as u8)
            .collect();
        for level in [0, 1, 6, 9] {
            let encoded = png(&eight, (width, height), false, level, None);
            let (info, pixels) = decode(&encoded);
            assert_eq!((info.width, info.height), (width, height));
            assert_eq!(info.bit_depth, png::BitDepth::Eight);
            assert_eq!(pixels, eight, "レベル {level}");
        }
        let sixteen: Vec<u8> = (0..width * height * 6)
            .map(|at| ((at * 31 + at / 582) % 256) as u8)
            .collect();
        let encoded = png(&sixteen, (width, height), true, 6, Some([9, 16, 0, 1]));
        let (info, pixels) = decode(&encoded);
        assert_eq!(info.bit_depth, png::BitDepth::Sixteen);
        assert_eq!(pixels, sixteen);
        assert!(encoded.windows(4).any(|kind| kind == b"cICP"));
    }

    #[test]
    fn adler_sums_join() {
        let whole = b"Wikipedia is a free online encyclopedia";
        let sum = |bytes: &[u8]| {
            let mut adler = adler2::Adler32::new();
            adler.write_slice(bytes);
            adler.checksum()
        };
        let (left, right) = whole.split_at(13);
        assert_eq!(
            adler32_combine(sum(left), sum(right), right.len()),
            sum(whole)
        );
    }
}
