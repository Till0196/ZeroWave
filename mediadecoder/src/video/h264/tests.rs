// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Hand-written CAVLC streams fed through [`crate::es::h264`].

use super::H264;
use crate::es::{Access, Codec, Coding, Framing, Splitter, Structure};
use crate::video::{Decoder, Error, Frame};

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    at: usize,
}

impl Bits {
    fn bit(&mut self, value: u32) {
        if self.at.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if value & 1 == 1 {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 1 << (7 - self.at % 8);
        }
        self.at += 1;
    }

    fn bits(&mut self, count: usize, value: u32) {
        for shift in (0..count).rev() {
            self.bit(value >> shift);
        }
    }

    fn ue(&mut self, value: u32) {
        let raised = value + 1;
        let width = 32 - raised.leading_zeros() as usize;
        self.bits(width - 1, 0);
        self.bits(width, raised);
    }

    fn se(&mut self, value: i32) {
        self.ue(if value <= 0 {
            (-2 * value) as u32
        } else {
            (2 * value - 1) as u32
        });
    }

    fn stop(&mut self) {
        self.bit(1);
        while !self.at.is_multiple_of(8) {
            self.bit(0);
        }
    }
}

fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut zeroes = 0;
    for &byte in bytes {
        if zeroes == 2 && byte <= 3 {
            out.push(3);
            zeroes = 0;
        }
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
        out.push(byte);
    }
    out
}

fn nal(kind: u8, reference: u8, write: impl FnOnce(&mut Bits)) -> Vec<u8> {
    let mut bits = Bits::default();
    write(&mut bits);
    let mut out = vec![0, 0, 0, 1, reference << 5 | kind];
    out.extend(escape(&bits.bytes));
    out
}

fn sequence() -> Vec<u8> {
    nal(7, 3, |bits| {
        bits.bits(8, 66); // profile_idc, Baseline
        bits.bits(8, 0); // the constraint flags
        bits.bits(8, 10); // level_idc
        bits.ue(0); // seq_parameter_set_id
        bits.ue(0); // log2_max_frame_num_minus4
        bits.ue(2); // pic_order_cnt_type
        bits.ue(1); // max_num_ref_frames
        bits.bit(0); // gaps_in_frame_num_value_allowed_flag
        bits.ue(0); // pic_width_in_mbs_minus1
        bits.ue(0); // pic_height_in_map_units_minus1
        bits.bit(1); // frame_mbs_only_flag
        bits.bit(1); // direct_8x8_inference_flag
        bits.bit(0); // frame_cropping_flag
        bits.bit(0); // vui_parameters_present_flag
        bits.stop();
    })
}

fn picture_set() -> Vec<u8> {
    nal(8, 3, |bits| {
        bits.ue(0); // pic_parameter_set_id
        bits.ue(0); // seq_parameter_set_id
        bits.bit(0); // entropy_coding_mode_flag
        bits.bit(0); // bottom_field_pic_order_in_frame_present_flag
        bits.ue(0); // num_slice_groups_minus1
        bits.ue(0); // num_ref_idx_l0_default_active_minus1
        bits.ue(0); // num_ref_idx_l1_default_active_minus1
        bits.bit(0); // weighted_pred_flag
        bits.bits(2, 0); // weighted_bipred_idc
        bits.se(0); // pic_init_qp_minus26
        bits.se(0); // pic_init_qs_minus26
        bits.se(0); // chroma_qp_index_offset
        bits.bit(0); // deblocking_filter_control_present_flag
        bits.bit(0); // constrained_intra_pred_flag
        bits.bit(0); // redundant_pic_cnt_present_flag
        bits.stop();
    })
}

/// One IDR picture of one Intra16x16 macroblock with `level` as its one DC coefficient.
fn refresh(level: Option<i32>) -> Vec<u8> {
    nal(5, 3, |bits| {
        bits.ue(0); // first_mb_in_slice
        bits.ue(7); // slice_type
        bits.ue(0); // pic_parameter_set_id
        bits.bits(4, 0); // frame_num
        bits.ue(0); // idr_pic_id
        bits.bit(0); // no_output_of_prior_pics_flag
        bits.bit(0); // long_term_reference_flag
        bits.se(0); // slice_qp_delta
        // I_16x16_2_0_0: DC prediction, DC coefficients only.
        bits.ue(3);
        bits.ue(0); // intra_chroma_pred_mode
        bits.se(0); // mb_qp_delta
        match level {
            None => bits.bits(1, 0b1),
            Some(level) if level.abs() == 1 => {
                bits.bits(2, 0b01);
                bits.bit(u32::from(level < 0)); // its sign
                bits.bits(1, 0b1); // total_zeros: none before it
            }
            Some(level) => {
                bits.bits(6, 0b000101);
                let code = if level > 0 {
                    2 * level - 2
                } else {
                    -2 * level - 1
                };
                bits.bits(code as usize - 2, 0);
                bits.bit(1);
                bits.bits(1, 0b1); // total_zeros
            }
        }
        bits.stop();
    })
}

fn skipped(frame_num: u32) -> Vec<u8> {
    nal(1, 1, |bits| {
        bits.ue(0); // first_mb_in_slice
        bits.ue(5); // slice_type
        bits.ue(0); // pic_parameter_set_id
        bits.bits(4, frame_num);
        bits.bit(0); // num_ref_idx_active_override_flag
        bits.bit(0); // ref_pic_list_modification_flag_l0
        bits.bit(0); // adaptive_ref_pic_marking_mode_flag
        bits.se(0); // slice_qp_delta
        bits.ue(1); // mb_skip_run
        bits.stop();
    })
}

fn run(stream: &[u8]) -> (Vec<Frame>, Vec<Error>) {
    let mut splitter = Splitter::new(Codec::H264, Framing::StartCodes);
    let mut decoder = H264::new();
    let mut frames = Vec::new();
    let mut errors = Vec::new();
    let mut accesses: Vec<Access> = Vec::new();
    splitter.push(stream, &mut accesses);
    for (at, access) in accesses.iter().enumerate() {
        let Some(format) = access.format else {
            continue;
        };
        if let Err(error) = decoder.decode(
            at as u64,
            &access.picture,
            format,
            &stream[access.range.clone()],
        ) {
            errors.push(error);
        }
        while let Some(frame) = decoder.take() {
            frames.push(frame);
        }
    }
    decoder.flush();
    while let Some(frame) = decoder.take() {
        frames.push(frame);
    }
    (frames, errors)
}

fn stream(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.iter().flatten().copied().collect()
}

fn sized_sequence(width_mbs_minus1: u32, height_units_minus1: u32) -> Vec<u8> {
    let mut bits = Bits::default();
    bits.bits(8, 66);
    bits.bits(8, 0);
    bits.bits(8, 10);
    bits.ue(0);
    bits.ue(0);
    bits.ue(2);
    bits.ue(1);
    bits.bit(0);
    bits.ue(width_mbs_minus1);
    bits.ue(height_units_minus1);
    bits.bit(1);
    bits.bit(1);
    bits.bit(0);
    bits.bit(0);
    bits.stop();
    bits.bytes
}

#[test]
fn a_sequence_larger_than_any_level_is_refused() {
    use super::params::sequence;
    assert!(sequence(&sized_sequence(119, 67)).is_some());
    assert!(sequence(&sized_sequence(511, 271)).is_some());
    assert!(sequence(&sized_sequence(512, 0)).is_none());
    assert!(sequence(&sized_sequence(0, 272)).is_none());
    assert!(sequence(&sized_sequence(u32::MAX - 1, u32::MAX - 1)).is_none());
}

#[test]
fn a_macroblock_predicted_from_nothing_is_the_middle_of_the_range() {
    let (frames, errors) = run(&stream(&[sequence(), picture_set(), refresh(None)]));
    assert_eq!(errors, []);
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    assert_eq!((frame.format.width, frame.format.height), (16, 16));
    assert_eq!(frame.structure, Structure::Frame);
    assert!(frame.progressive);
    assert!(frame.planes[0].data.iter().all(|&value| value == 128));
    assert!(frame.planes[1].data.iter().all(|&value| value == 128));
    assert!(frame.planes[2].data.iter().all(|&value| value == 128));
}

#[test]
fn one_direct_current_coefficient_lifts_the_whole_macroblock() {
    for (level, wanted) in [(1, 129), (-1, 127), (4, 131), (-4, 125)] {
        let (frames, errors) = run(&stream(&[sequence(), picture_set(), refresh(Some(level))]));
        assert_eq!(errors, [], "level {level}");
        assert_eq!(frames.len(), 1, "level {level}");
        assert!(
            frames[0].planes[0]
                .data
                .iter()
                .all(|&value| value == wanted),
            "level {level}: {:?}",
            &frames[0].planes[0].data[..8]
        );
        assert!(frames[0].planes[1].data.iter().all(|&value| value == 128));
    }
}

#[test]
fn a_picture_the_coder_said_nothing_about_is_the_one_before_it() {
    let (frames, errors) = run(&stream(&[
        sequence(),
        picture_set(),
        refresh(Some(4)),
        skipped(1),
        skipped(2),
    ]));
    assert_eq!(errors, []);
    assert_eq!(frames.len(), 3);
    for (at, frame) in frames.iter().enumerate() {
        assert!(
            frame.planes[0].data.iter().all(|&value| value == 131),
            "picture {at}"
        );
    }
    assert_eq!(
        frames.iter().map(|frame| frame.tag).collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
fn a_refresh_is_where_the_front_end_says_a_decoder_may_begin() {
    let mut splitter = Splitter::new(Codec::H264, Framing::StartCodes);
    let mut accesses: Vec<Access> = Vec::new();
    splitter.push(
        &stream(&[sequence(), picture_set(), refresh(None), skipped(1)]),
        &mut accesses,
    );
    assert_eq!(accesses.len(), 2);
    assert!(accesses[0].picture.random_access);
    assert_eq!(accesses[0].picture.coding, Coding::I);
    assert!(!accesses[1].picture.random_access);
    assert_eq!(accesses[1].picture.coding, Coding::P);
}

#[test]
fn a_stream_that_switches_between_frames_and_fields_inside_one_is_refused() {
    let mut sequence = nal(7, 3, |bits| {
        bits.bits(8, 77); // Main profile
        bits.bits(8, 0);
        bits.bits(8, 30);
        bits.ue(0);
        bits.ue(0); // log2_max_frame_num_minus4
        bits.ue(2); // pic_order_cnt_type
        bits.ue(1); // max_num_ref_frames
        bits.bit(0);
        bits.ue(0); // pic_width_in_mbs_minus1
        bits.ue(0); // pic_height_in_map_units_minus1
        bits.bit(0); // frame_mbs_only_flag
        bits.bit(1); // mb_adaptive_frame_field_flag
        bits.bit(1); // direct_8x8_inference_flag
        bits.bit(0);
        bits.bit(0);
        bits.stop();
    });
    sequence.extend(picture_set());
    sequence.extend(nal(5, 3, |bits| {
        bits.ue(0);
        bits.ue(7);
        bits.ue(0);
        bits.bits(4, 0);
        bits.bit(0); // field_pic_flag
        bits.ue(0); // idr_pic_id
        bits.bit(0);
        bits.bit(0);
        bits.se(0);
        bits.ue(3);
        bits.ue(0);
        bits.se(0);
        bits.bits(1, 0b1);
        bits.stop();
    }));
    let (frames, errors) = run(&sequence);
    assert!(frames.is_empty());
    assert_eq!(
        errors,
        [Error::Unsupported(
            "macroblock adaptive frame and field coding"
        )]
    );
}

#[test]
fn a_slice_with_no_parameter_set_to_read_it_by_decodes_nothing() {
    let (frames, errors) = run(&stream(&[sequence(), refresh(None)]));
    assert!(frames.is_empty());
    assert_eq!(
        errors,
        [Error::Damaged(
            "a slice with no parameter set to read it by"
        )]
    );
}

#[test]
fn giving_up_everything_held_leaves_a_decoder_that_still_works() {
    let mut decoder = H264::new();
    decoder.reset();
    assert!(decoder.take().is_none());
    decoder.flush();
    assert!(decoder.take().is_none());
}

/// Streams made by `testdata/make.sh`.
#[test]
fn an_encoded_stream_decodes_to_the_pictures_it_says_it_holds() {
    for (name, width, height) in [("h264-baseline.ts", 320, 180), ("h264-high.ts", 352, 288)] {
        let bytes = crate::testdata(name);
        let mut splitter = Splitter::new(Codec::H264, Framing::StartCodes);
        let mut decoder = H264::new();
        let mut frames = Vec::new();
        let mut coded = 0;
        let mut errors = Vec::new();
        let mut structures = Vec::new();
        let mut codings = Vec::new();
        for unit in crate::pes(&bytes, 0x100) {
            let mut accesses: Vec<Access> = Vec::new();
            splitter.push(&unit, &mut accesses);
            for access in accesses {
                let Some(format) = access.format else {
                    continue;
                };
                coded += 1;
                structures.push(access.picture.structure);
                codings.push(access.picture.coding);
                if let Err(error) =
                    decoder.decode(coded, &access.picture, format, &unit[access.range])
                {
                    errors.push(error);
                }
                while let Some(frame) = decoder.take() {
                    frames.push(frame);
                }
            }
        }
        decoder.flush();
        while let Some(frame) = decoder.take() {
            frames.push(frame);
        }

        assert!(errors.is_empty(), "{name}: {errors:?}");
        assert_eq!(coded, 30, "{name}: coded pictures");
        assert_eq!(frames.len(), coded as usize, "{name}: frames");
        assert!(structures.iter().all(|structure| !structure.field()));
        assert_eq!(codings[0], Coding::I, "{name}");
        for frame in &frames {
            assert_eq!((frame.format.width, frame.format.height), (width, height));
            let luma = &frame.planes[0];
            assert_eq!((luma.width, luma.height), (width as usize, height as usize));
            assert_eq!(
                (frame.planes[1].width, frame.planes[1].height),
                (width as usize / 2, height as usize / 2)
            );
            assert_eq!(
                luma.data.len(),
                luma.stride * (height as usize).next_multiple_of(16)
            );
            assert!(luma.data.iter().any(|&value| value != 128));
        }
        let shown: Vec<u64> = frames.iter().map(|frame| frame.tag).collect();
        if name == "h264-high.ts" {
            assert!(
                shown.windows(2).any(|pair| pair[0] > pair[1]),
                "{name}: nothing was reordered: {shown:?}"
            );
        }
    }
}
