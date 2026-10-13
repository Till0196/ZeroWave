// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-2 for a stateless V4L2 decoder. Unlike VA-API, references are timestamps and the
//! picture's slices go in with their start codes instead of slice parameters.

use mediadecoder::es::{Format, Picture, StartCodes};
use mediadecoder::video::mpeg2::headers::{self, Chroma, Matrices, Order, PictureCoding, Sequence};

use crate::decode::Error;
use crate::decode::frames;
use crate::decode::v4l2::ctrl::{self, Ctl, Kind};
use crate::decode::v4l2::pool::Pool;
use crate::decode::v4l2::{Held, Mode, Submission, Unit};

/// `intra_dc_precision` and `picture_structure` are own bytes, so the bits start at zero.
fn flags(coded: &PictureCoding) -> u32 {
    let bit = |value: bool, flag: u32| if value { flag } else { 0 };
    bit(
        coded.top_field_first,
        ctrl::V4L2_MPEG2_PIC_FLAG_TOP_FIELD_FIRST,
    ) | bit(
        coded.frame_pred_frame_dct,
        ctrl::V4L2_MPEG2_PIC_FLAG_FRAME_PRED_DCT,
    ) | bit(
        coded.concealment_motion_vectors,
        ctrl::V4L2_MPEG2_PIC_FLAG_CONCEALMENT_MV,
    ) | bit(coded.q_scale_type, ctrl::V4L2_MPEG2_PIC_FLAG_Q_SCALE_TYPE)
        | bit(coded.intra_vlc_format, ctrl::V4L2_MPEG2_PIC_FLAG_INTRA_VLC)
        | bit(coded.alternate_scan, ctrl::V4L2_MPEG2_PIC_FLAG_ALT_SCAN)
        | bit(
            coded.repeat_first_field,
            ctrl::V4L2_MPEG2_PIC_FLAG_REPEAT_FIRST,
        )
        | bit(
            coded.progressive_frame,
            ctrl::V4L2_MPEG2_PIC_FLAG_PROGRESSIVE,
        )
}

/// A field's own `top_field_first` is always zero (13818-2 §6.3.10); the flag says whether
/// the frame began with the top field, which is how hantro and cedrus tell the second field.
fn flags_for_field(coded: &PictureCoding, first_top: bool) -> u32 {
    let flags = flags(coded) & !ctrl::V4L2_MPEG2_PIC_FLAG_TOP_FIELD_FIRST;
    if first_top {
        flags | ctrl::V4L2_MPEG2_PIC_FLAG_TOP_FIELD_FIRST
    } else {
        flags
    }
}

fn f_code(coded: &PictureCoding) -> [[u8; 2]; 2] {
    coded.f_code.map(|axes| axes.map(|code| code as u8))
}

#[derive(Debug)]
pub struct Mpeg2 {
    matrices: Matrices,
    sequence: Sequence,
    order: frames::Mpeg2Order<u64>,
    /// The sequence control carries a field's own height; cedrus marks every field bad otherwise.
    field_height: bool,
    first_top: bool,
    /// The reorder's `pending` keeps the first field's parity after the second comes.
    awaiting_second: bool,
}

impl Default for Mpeg2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mpeg2 {
    pub fn new() -> Self {
        Self {
            matrices: Matrices::new(Order::AsSent, Chroma::AsSent),
            sequence: Sequence::default(),
            order: frames::Mpeg2Order::new(),
            field_height: false,
            first_top: true,
            awaiting_second: false,
        }
    }

    pub fn take_field_height(&mut self, wanted: bool) {
        self.field_height = wanted;
    }

    pub fn configure(&mut self, _mode: Mode, _prefix: u32) {}

    pub fn take(&mut self) -> Option<Held> {
        self.order.take()
    }

    pub fn flush(&mut self, pool: &mut Pool) {
        self.order.flush(pool);
    }

    pub fn reset(&mut self, pool: &mut Pool) {
        self.order.reset(pool);
        self.awaiting_second = false;
        self.matrices = Matrices::new(Order::AsSent, Chroma::AsSent);
    }

    pub fn decode(
        &mut self,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<Option<Submission>, Error> {
        let mut coded = PictureCoding::default();
        let mut slices: Option<std::ops::Range<usize>> = None;
        let mut zeroes = 0;

        for (kind, body) in StartCodes::new(data) {
            let bytes = &data[body.clone()];
            match kind {
                0xb3 => self.matrices.read_sequence(bytes),
                0xb5 => match headers::extension_id(bytes) {
                    headers::SEQUENCE_EXTENSION => self.sequence = Sequence::read(bytes),
                    headers::QUANT_MATRIX_EXTENSION => self.matrices.read_extension(bytes),
                    headers::PICTURE_CODING_EXTENSION => coded.read_extension(bytes),
                    _ => {}
                },
                0x00 => {
                    coded = PictureCoding {
                        kind: headers::read_picture(bytes),
                        ..PictureCoding::default()
                    };
                }
                0x01..=0xaf if coded.kind != 0 => {
                    // Start code included; the end stops before the next `00 00 01`.
                    let start = body.start.saturating_sub(4);
                    let (end, missing) = headers::slice_end(data, body.end);
                    zeroes = missing;
                    match &mut slices {
                        Some(range) => range.end = end,
                        None => slices = Some(start..end),
                    }
                }
                _ => {}
            }
        }

        let Some(slices) = slices else {
            return Ok(None);
        };
        if coded.kind == 0 {
            return Err(Error::Damaged("a slice with no picture header"));
        }
        // No picture coding extension (0) is a frame too.
        let field = !matches!(coded.picture_structure, 0 | 3);

        // A field whose mate is being decoded writes into the same capture buffer.
        self.order.begin(pool, tag, picture)?;
        let held = self
            .order
            .current()
            .ok_or(Error::Damaged("a slice with no picture"))?;

        let (forward, backward) = self.order.references(picture.coding);
        // A first field leaves the capture buffer open for the second only.
        let more = self.order.pending() == Some(picture.structure);
        if field && more {
            self.first_top = coded.picture_structure == u32::from(ctrl::V4L2_MPEG2_PIC_TOP_FIELD);
        }
        self.awaiting_second = more;
        let sequence = ctrl::Mpeg2Sequence {
            horizontal_size: format.width as u16,
            vertical_size: if field && self.field_height {
                format.height.div_ceil(2) as u16
            } else {
                format.height as u16
            },
            vbv_buffer_size: 0,
            profile_and_level_indication: self.sequence.profile_and_level,
            chroma_format: self.sequence.chroma_format.max(1),
            flags: if self.sequence.progressive {
                ctrl::V4L2_MPEG2_SEQ_FLAG_PROGRESSIVE
            } else {
                0
            },
        };
        let control = ctrl::Mpeg2Picture {
            backward_ref_ts: backward.map_or(0, |held| held.slot),
            forward_ref_ts: forward.map_or(0, |held| held.slot),
            flags: if field {
                flags_for_field(&coded, self.first_top)
            } else {
                flags(&coded)
            },
            f_code: f_code(&coded),
            picture_coding_type: coded.kind as u8,
            picture_structure: match coded.picture_structure as u8 {
                0 => ctrl::V4L2_MPEG2_PIC_FRAME,
                other => other,
            },
            intra_dc_precision: coded.intra_dc_precision as u8,
            reserved: [0; 5],
        };
        let quantisation = ctrl::Mpeg2Quantisation {
            intra_quantiser_matrix: self.matrices.intra,
            non_intra_quantiser_matrix: self.matrices.non_intra,
            chroma_intra_quantiser_matrix: self.matrices.chroma_intra,
            chroma_non_intra_quantiser_matrix: self.matrices.chroma_non_intra,
        };

        Ok(Some(Submission {
            ts: held.slot,
            units: vec![Unit {
                controls: vec![
                    Ctl::of(Kind::Mpeg2Sequence, &sequence),
                    Ctl::of(Kind::Mpeg2Picture, &control),
                    Ctl::of(Kind::Mpeg2Quantisation, &quantisation),
                ],
                pieces: vec![slices],
                zeroes,
            }],
            more,
        }))
    }

    pub fn finished(&mut self, pool: &mut Pool) {
        if self.awaiting_second {
            return;
        }
        self.close(pool);
    }

    /// Drops open GOP leading B pictures too.
    fn close(&mut self, pool: &mut Pool) {
        self.order.close(pool);
    }
}

#[cfg(test)]
mod tests {
    use mediadecoder::es::Structure;

    use super::*;

    #[test]
    fn a_field_says_which_field_began_its_frame() {
        let coded = PictureCoding {
            picture_structure: 2,
            ..PictureCoding::default()
        };
        assert_ne!(
            flags_for_field(&coded, true) & ctrl::V4L2_MPEG2_PIC_FLAG_TOP_FIELD_FIRST,
            0
        );
        let stray = PictureCoding {
            top_field_first: true,
            ..coded
        };
        assert_eq!(
            flags_for_field(&stray, false) & ctrl::V4L2_MPEG2_PIC_FLAG_TOP_FIELD_FIRST,
            0
        );
    }

    #[test]
    fn the_picture_flags_are_one_bit_each_and_start_at_nought() {
        let coded = PictureCoding {
            intra_dc_precision: 2,
            picture_structure: 3,
            top_field_first: true,
            alternate_scan: true,
            progressive_frame: true,
            ..PictureCoding::default()
        };
        let flags = flags(&coded);
        assert_eq!(flags & ctrl::V4L2_MPEG2_PIC_FLAG_TOP_FIELD_FIRST, 0x0001);
        assert_eq!(flags & ctrl::V4L2_MPEG2_PIC_FLAG_ALT_SCAN, 0x0020);
        assert_eq!(flags & ctrl::V4L2_MPEG2_PIC_FLAG_PROGRESSIVE, 0x0080);
        assert_eq!(flags & 0x0002, 0, "frame_pred_frame_dct was not set");
        assert_eq!(flags & 0x0004, 0, "concealment_motion_vectors was not set");
    }

    #[test]
    fn the_f_codes_are_four_bytes_in_the_order_they_are_read() {
        let coded = PictureCoding {
            f_code: [[4, 5], [6, 7]],
            ..PictureCoding::default()
        };
        let control = ctrl::Mpeg2Picture {
            f_code: f_code(&coded),
            ..ctrl::Mpeg2Picture::default()
        };
        assert_eq!(control.f_code[0][0], 4);
        assert_eq!(control.f_code[1][1], 7);
    }

    #[test]
    fn an_absent_reference_is_nought() {
        let control = ctrl::Mpeg2Picture::default();
        assert_eq!(control.forward_ref_ts, 0);
        assert_eq!(control.backward_ref_ts, 0);
    }

    #[test]
    fn a_reference_comes_out_when_the_next_one_is_decoded() {
        let mut pool = Pool::new(8);
        let mut front = Mpeg2::new();
        let push = |front: &mut Mpeg2, pool: &mut Pool, bidirectional: bool| {
            let ts = pool.take().expect("a free buffer");
            let held = Held {
                slot: ts,
                bidirectional,
                ..Held::default()
            };
            let unshowable = bidirectional && front.order.reorder.older().is_none();
            front
                .order
                .reorder
                .begin(held, Structure::Frame, bidirectional, unshowable);
            front.close(pool);
            ts
        };
        let first = push(&mut front, &mut pool, false);
        assert!(
            front.take().is_none(),
            "the first reference came out at once"
        );
        let second = push(&mut front, &mut pool, false);
        assert_eq!(front.take().map(|held| held.slot), Some(first));
        let between = push(&mut front, &mut pool, true);
        assert_eq!(front.take().map(|held| held.slot), Some(between));
        let _ = second;
    }

    #[test]
    fn the_leading_pictures_of_an_open_group_are_dropped() {
        let mut pool = Pool::new(8);
        let mut front = Mpeg2::new();
        let ts = pool.take().expect("a free buffer");
        let held = Held {
            slot: ts,
            bidirectional: true,
            ..Held::default()
        };
        front
            .order
            .reorder
            .begin(held, Structure::Frame, true, true);
        front.close(&mut pool);
        assert!(
            front.take().is_none(),
            "a B picture with nothing to predict forward from was shown"
        );
    }
}
