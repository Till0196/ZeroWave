// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一枚が一フィールドの HEVC（`field_seq_flag`）を、デコード後に二フィールドずつ一枚のフレームに組む。
//! 組まずに縦に伸ばすと半ライン分ずれて揺れる。同じパリティーが続いたら先のフィールドは捨てる。

use mediadecoder::es::Structure;
use mediadecoder::video::{Frame, Plane};

use super::Readback;

#[derive(Debug, Default)]
pub struct Weaver {
    pending: Option<(Frame, bool, Option<i64>)>,
}

impl Weaver {
    /// 対がそろえば組んだフレームと、先のフィールドの時刻を返す。
    pub fn push(
        &mut self,
        frame: Frame,
        top: bool,
        at: Option<i64>,
    ) -> Option<(Frame, Option<i64>)> {
        match self.pending.take() {
            Some((first, first_top, first_at)) if first_top != top => {
                let woven = if first_top {
                    weave(&first, &frame, true)
                } else {
                    weave(&frame, &first, false)
                };
                match woven {
                    Some(woven) => Some((woven, first_at)),
                    // 途中でサイズが変わった対。後のフィールドから組み直す。
                    None => {
                        self.pending = Some((frame, top, at));
                        None
                    }
                }
            }
            _ => {
                self.pending = Some((frame, top, at));
                None
            }
        }
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }
}

/// トップとボトムをラインごとに交互に並べたフレームにする。
pub fn weave(top: &Frame, bottom: &Frame, top_first: bool) -> Option<Frame> {
    if top.format.width != bottom.format.width
        || top.format.height != bottom.format.height
        || top.format.depth != bottom.format.depth
    {
        return None;
    }
    let size = if top.format.depth > 8 { 2 } else { 1 };
    let planes = [0, 1, 2].map(|at| interleave(&top.planes[at], &bottom.planes[at], size));
    let [Some(luma), Some(blue), Some(red)] = planes else {
        return None;
    };
    let first = if top_first { top } else { bottom };
    let (num, den) = first.format.rate;
    let mut format = first.format;
    format.height = format.height.saturating_mul(2);
    format.field_pictures = false;
    format.progressive = false;
    format.woven = true;
    format.rate = (num, den.saturating_mul(2));
    Some(Frame {
        tag: first.tag,
        format,
        planes: [luma, blue, red],
        structure: Structure::Frame,
        top_field_first: top_first,
        repeat_first_field: false,
        progressive: false,
    })
}

fn interleave(top: &Plane, bottom: &Plane, size: usize) -> Option<Plane> {
    if top.width != bottom.width || top.height != bottom.height || top.width == 0 {
        return None;
    }
    let size = top.width * size;
    let mut data = Vec::with_capacity(size * top.height * 2);
    for y in 0..top.height {
        data.extend_from_slice(top.data.get(y * top.stride..y * top.stride + size)?);
        data.extend_from_slice(
            bottom
                .data
                .get(y * bottom.stride..y * bottom.stride + size)?,
        );
    }
    Some(Plane {
        data,
        stride: size,
        width: top.width,
        height: top.height * 2,
    })
}

/// 読み戻したフレームをメインメモリのフレームにする。10 ビット（`P010`）は下位ビットに詰める。
/// 組めなければ理由を返す（輝度しか無い、プレーンがフレームの大きさに足りない）。
pub fn frame_of_readback(read: &Readback) -> Result<Frame, &'static str> {
    match read.planes.len() {
        2 | 3 => copied(read).ok_or("プレーンがフレームの大きさに足りない"),
        1 => Err("色差のプレーンが無い（輝度だけ）"),
        _ => Err("プレーンが無い"),
    }
}

fn copied(read: &Readback) -> Option<Frame> {
    let about = read.about;
    let width = about.format.width as usize;
    let height = about.format.height as usize;
    let wide = about.format.depth > 8;
    let shift = if wide {
        16 - u32::from(about.format.depth)
    } else {
        0
    };
    let size = if wide { 2 } else { 1 };
    let (half_w, half_h) = (width.div_ceil(2), height.div_ceil(2));
    // ラインごとにまとめてコピーする（一画素ずつだと読み戻しが間に合わない）。
    let copy = |bytes: &[u8], pitch: usize, w: usize, h: usize| -> Option<Vec<u8>> {
        let row_bytes = w * size;
        let mut out = Vec::with_capacity(row_bytes * h);
        for row in 0..h {
            let line = bytes.get(row * pitch..row * pitch + row_bytes)?;
            if wide {
                out.extend(line.as_chunks::<2>().0.iter().flat_map(|pair| {
                    (u16::from_le_bytes([pair[0], pair[1]]) >> shift).to_le_bytes()
                }));
            } else {
                out.extend_from_slice(line);
            }
        }
        Some(out)
    };
    let (luma, luma_pitch, _) = read.planes.first()?;
    let y = copy(luma, *luma_pitch, width, height)?;
    let (blue, red) = match read.planes.len() {
        2 => {
            let (chroma, pitch, _) = &read.planes[1];
            let mut blue = Vec::with_capacity(half_w * half_h * size);
            let mut red = Vec::with_capacity(half_w * half_h * size);
            for row in 0..half_h {
                let line = chroma.get(row * pitch..row * pitch + half_w * 2 * size)?;
                if wide {
                    for quad in line.as_chunks::<4>().0 {
                        blue.extend_from_slice(
                            &(u16::from_le_bytes([quad[0], quad[1]]) >> shift).to_le_bytes(),
                        );
                        red.extend_from_slice(
                            &(u16::from_le_bytes([quad[2], quad[3]]) >> shift).to_le_bytes(),
                        );
                    }
                } else {
                    for pair in line.as_chunks::<2>().0 {
                        blue.push(pair[0]);
                        red.push(pair[1]);
                    }
                }
            }
            (blue, red)
        }
        3 => {
            let (b, bp, _) = &read.planes[1];
            let (r, rp, _) = &read.planes[2];
            (copy(b, *bp, half_w, half_h)?, copy(r, *rp, half_w, half_h)?)
        }
        _ => return None,
    };
    let plane = |data, w: usize, h: usize| Plane {
        data,
        stride: w * size,
        width: w,
        height: h,
    };
    Some(Frame {
        tag: about.tag,
        format: about.format,
        planes: [
            plane(y, width, height),
            plane(blue, half_w, half_h),
            plane(red, half_w, half_h),
        ],
        structure: about.structure,
        top_field_first: about.top_field_first,
        repeat_first_field: about.repeat_first_field,
        progressive: about.progressive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::es::Format;

    fn field(tag: u64, value: u8) -> Frame {
        let format = Format {
            width: 4,
            height: 2,
            depth: 8,
            rate: (60000, 1001),
            field_pictures: true,
            ..Format::default()
        };
        let plane = |w: usize, h: usize, v: u8| Plane {
            data: (0..w * h).map(|i| v + i as u8).collect(),
            stride: w,
            width: w,
            height: h,
        };
        Frame {
            tag,
            format,
            planes: [plane(4, 2, value), plane(2, 1, value), plane(2, 1, value)],
            structure: Structure::Frame,
            top_field_first: false,
            repeat_first_field: false,
            progressive: false,
        }
    }

    #[test]
    fn a_top_and_a_bottom_become_one_frame_of_alternate_rows() {
        let woven = weave(&field(1, 10), &field(2, 100), true).expect("woven");
        assert_eq!(woven.format.height, 4);
        assert!(woven.format.woven && !woven.format.field_pictures);
        assert_eq!(woven.format.rate, (60000, 2002));
        let luma = &woven.planes[0];
        assert_eq!((luma.width, luma.height), (4, 4));
        assert_eq!(&luma.data[0..4], &[10, 11, 12, 13]);
        assert_eq!(&luma.data[4..8], &[100, 101, 102, 103]);
        assert_eq!(&luma.data[8..12], &[14, 15, 16, 17]);
        assert_eq!(woven.planes[1].height, 2);
        assert!(woven.top_field_first);
        assert_eq!(woven.tag, 1);
    }

    #[test]
    fn a_bottom_first_pair_says_so() {
        let mut weaver = Weaver::default();
        assert!(weaver.push(field(5, 100), false, Some(50)).is_none());
        let (woven, at) = weaver.push(field(6, 10), true, Some(51)).expect("a pair");
        assert!(!woven.top_field_first);
        assert_eq!((woven.tag, at), (5, Some(50)));
        assert_eq!(&woven.planes[0].data[0..4], &[10, 11, 12, 13]);
    }

    #[test]
    fn a_field_without_its_partner_is_dropped() {
        let mut weaver = Weaver::default();
        assert!(weaver.push(field(1, 10), true, Some(1)).is_none());
        assert!(weaver.push(field(2, 20), true, Some(2)).is_none());
        let (woven, at) = weaver.push(field(3, 100), false, Some(3)).expect("a pair");
        assert_eq!((woven.tag, at), (2, Some(2)));
    }

    #[test]
    fn a_ten_bit_read_back_is_shifted_down() {
        let format = Format {
            width: 2,
            height: 2,
            depth: 10,
            ..Format::default()
        };
        let p010 = |v: u16| (v << 6).to_le_bytes();
        let mut luma = Vec::new();
        for v in [1u16, 2, 3, 4] {
            luma.extend_from_slice(&p010(v));
        }
        let mut chroma = Vec::new();
        chroma.extend_from_slice(&p010(7));
        chroma.extend_from_slice(&p010(9));
        let read = Readback {
            about: super::super::About {
                tag: 0,
                format,
                structure: Structure::Frame,
                top_field_first: false,
                repeat_first_field: false,
                progressive: true,
            },
            planes: vec![(luma, 4, 2), (chroma, 4, 1)],
        };
        let frame = frame_of_readback(&read).expect("readable");
        assert_eq!(frame.planes[0].data, vec![1, 0, 2, 0, 3, 0, 4, 0]);
        assert_eq!(frame.planes[1].data, vec![7, 0]);
        assert_eq!(frame.planes[2].data, vec![9, 0]);
    }

    #[test]
    fn a_readback_of_luma_alone_is_refused_with_the_reason() {
        let format = Format {
            width: 2,
            height: 2,
            depth: 8,
            ..Format::default()
        };
        let read = Readback {
            about: super::super::About {
                tag: 0,
                format,
                structure: Structure::Frame,
                top_field_first: false,
                repeat_first_field: false,
                progressive: true,
            },
            planes: vec![(vec![1, 2, 3, 4], 2, 2)],
        };
        assert_eq!(
            frame_of_readback(&read).err(),
            Some("色差のプレーンが無い（輝度だけ）")
        );
    }
}
