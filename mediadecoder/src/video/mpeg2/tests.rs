// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 手で組み立てたストリームで各層を試す。実際の処理と同じく [`crate::es::mpeg2`] を通す。

use super::super::{Decoder, Frame};
use super::tables::{self, BACKWARD, FORWARD, INTRA, PATTERN};
use super::{Mpeg2, idct};
use crate::es::{Access, Codec, Framing, Splitter, Structure};

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

    fn align(&mut self) {
        self.at = self.bytes.len() * 8;
    }

    fn start(&mut self, kind: u8) {
        self.align();
        self.bytes.extend_from_slice(&[0, 0, 1, kind]);
        self.at = self.bytes.len() * 8;
    }

    fn vlc(&mut self, table: &[(u16, u8, u16)], value: u16) {
        let &(code, length, _) = table
            .iter()
            .find(|&&(_, _, held)| held == value)
            .unwrap_or_else(|| panic!("{value} is in the table"));
        self.bits(usize::from(length), u32::from(code));
    }

    fn coefficient(&mut self, run: u8, level: i32) {
        let &(code, length, _, _) = tables::COEFFICIENTS_ZERO
            .iter()
            .find(|&&(_, _, held_run, held_level)| {
                held_run == run && i32::from(held_level) == level.abs()
            })
            .expect("a run and level Table B.14 holds");
        self.bits(usize::from(length), u32::from(code));
        self.bit(u32::from(level < 0));
    }

    fn end_of_block(&mut self) {
        let (code, length) = tables::END_OF_BLOCK_ZERO;
        self.bits(usize::from(length), u32::from(code));
    }

    fn dc(&mut self, table: &[(u16, u8, u16)], difference: i32) {
        let size = (32 - difference.unsigned_abs().leading_zeros()) as u16;
        self.vlc(table, size);
        if size > 0 {
            let written = if difference > 0 {
                difference
            } else {
                difference + (1 << size) - 1
            };
            self.bits(usize::from(size), written as u32);
        }
    }

    fn flat_block(&mut self, luma: bool, difference: i32) {
        let table = if luma {
            tables::DC_SIZE_LUMA
        } else {
            tables::DC_SIZE_CHROMA
        };
        self.dc(table, difference);
        self.end_of_block();
    }

    fn zero_vector(&mut self) {
        self.vlc(tables::MOTION_CODE, 0);
        self.vlc(tables::MOTION_CODE, 0);
    }
}

/// Defaults are what a Japanese broadcast carries.
#[derive(Clone, Copy)]
struct Setup {
    width: u32,
    height: u32,
    aspect: u32,
    progressive_sequence: bool,
    frame_pred_frame_dct: bool,
    intra_matrix: Option<[u8; 64]>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            width: 16,
            height: 16,
            aspect: 1,
            progressive_sequence: true,
            frame_pred_frame_dct: true,
            intra_matrix: None,
        }
    }
}

impl Setup {
    fn sequence(&self, bits: &mut Bits) {
        bits.start(0xb3);
        bits.bits(12, self.width);
        bits.bits(12, self.height);
        bits.bits(4, self.aspect); // aspect_ratio_information
        bits.bits(4, 4); // frame_rate_code: 30000/1001
        bits.bits(18, 0x12345);
        bits.bit(1); // marker_bit
        bits.bits(10, 112);
        bits.bit(0); // constrained_parameters_flag
        match self.intra_matrix {
            Some(matrix) => {
                bits.bit(1);
                // Matrices are sent in zigzag order.
                for &at in &tables::ZIGZAG {
                    bits.bits(8, u32::from(matrix[usize::from(at)]));
                }
            }
            None => bits.bit(0),
        }
        bits.bit(0); // load_non_intra_quantiser_matrix

        bits.start(0xb5);
        bits.bits(4, 1); // sequence extension
        bits.bits(8, 0x48);
        bits.bit(u32::from(self.progressive_sequence));
        bits.bits(2, 1); // chroma_format: 4:2:0
        bits.bits(2, 0);
        bits.bits(2, 0);
        bits.bits(12, 0);
        bits.bit(1); // marker_bit
        bits.bits(8, 0);
        bits.bit(0); // low_delay
        bits.bits(2, 0);
        bits.bits(5, 0);
    }

    fn picture(&self, bits: &mut Bits, coding: u32, structure: u32) {
        bits.start(0x00);
        bits.bits(10, 0); // temporal_reference
        bits.bits(3, coding);
        bits.bits(16, 0xffff); // vbv_delay
        if coding >= 2 {
            bits.bit(0); // full_pel_forward_vector
            bits.bits(3, 1); // forward_f_code
        }
        if coding == 3 {
            bits.bit(0); // full_pel_backward_vector
            bits.bits(3, 1); // backward_f_code
        }
        bits.bit(0); // extra_bit_picture

        bits.start(0xb5);
        bits.bits(4, 8); // picture coding extension
        for _ in 0..4 {
            bits.bits(4, 1); // every f_code, which is the smallest range
        }
        bits.bits(2, 0); // intra_dc_precision: eight bits
        bits.bits(2, structure);
        bits.bit(1); // top_field_first
        bits.bit(u32::from(self.frame_pred_frame_dct));
        bits.bit(0); // concealment_motion_vectors
        bits.bit(0); // q_scale_type
        bits.bit(0); // intra_vlc_format
        bits.bit(0); // alternate_scan
        bits.bit(0); // repeat_first_field
        bits.bit(u32::from(self.progressive_sequence)); // chroma_420_type
        bits.bit(u32::from(self.progressive_sequence)); // progressive_frame
        bits.bit(0); // composite_display_flag
    }

    fn slice(&self, bits: &mut Bits, row: u8) {
        bits.start(row + 1);
        bits.bits(5, 1); // quantiser_scale_code
        bits.bit(0); // extra_bit_slice
    }
}

fn run(stream: &[u8]) -> (Vec<Frame>, Vec<crate::video::Error>) {
    let mut splitter = Splitter::new(Codec::Mpeg2, Framing::StartCodes);
    let mut decoder = Mpeg2::new();
    let mut accesses: Vec<Access> = Vec::new();
    splitter.push(stream, &mut accesses);
    let mut frames = Vec::new();
    let mut errors = Vec::new();
    for access in accesses {
        let format = access
            .format
            .expect("a sequence header stands over the picture");
        if let Err(error) = decoder.decode(0, &access.picture, format, &stream[access.range]) {
            errors.push(error);
        }
        while let Some(frame) = decoder.take() {
            frames.push(frame);
        }
    }
    (frames, errors)
}

fn row(frame: &Frame, plane: usize, y: usize) -> Vec<u8> {
    let plane = &frame.planes[plane];
    plane.row(y)[..plane.width].to_vec()
}

#[test]
fn an_intra_macroblock_of_one_coefficient_comes_out_flat() {
    // DC predictor 128 + difference 72 = 200.
    let setup = Setup::default();
    let mut bits = Bits::default();
    setup.sequence(&mut bits);
    setup.picture(&mut bits, 1, 3);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
    bits.flat_block(true, 72);
    for _ in 0..3 {
        bits.flat_block(true, 0);
    }
    for _ in 0..2 {
        bits.flat_block(false, 0);
    }
    setup.picture(&mut bits, 1, 3);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
    for _ in 0..4 {
        bits.flat_block(true, 0);
    }
    for _ in 0..2 {
        bits.flat_block(false, 0);
    }

    let (frames, errors) = run(&bits.bytes);
    assert_eq!(errors, []);
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    assert_eq!(frame.structure, Structure::Frame);
    assert_eq!((frame.planes[0].width, frame.planes[0].height), (16, 16));
    for y in 0..16 {
        assert_eq!(row(frame, 0, y), [200; 16], "luminance row {y}");
    }
    for y in 0..8 {
        assert_eq!(row(frame, 1, y), [128; 8], "blue difference row {y}");
        assert_eq!(row(frame, 2, y), [128; 8], "red difference row {y}");
    }
}

fn with_matrix(matrix: Option<[u8; 64]>) -> Frame {
    let setup = Setup {
        intra_matrix: matrix,
        ..Setup::default()
    };
    let mut bits = Bits::default();
    setup.sequence(&mut bits);
    for _ in 0..2 {
        setup.picture(&mut bits, 1, 3);
        setup.slice(&mut bits, 0);
        bits.vlc(tables::ADDRESS_INCREMENT, 1);
        bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
        bits.dc(tables::DC_SIZE_LUMA, 0);
        bits.coefficient(0, 1);
        bits.end_of_block();
        for _ in 0..3 {
            bits.flat_block(true, 0);
        }
        for _ in 0..2 {
            bits.flat_block(false, 0);
        }
    }
    let (frames, errors) = run(&bits.bytes);
    assert_eq!(errors, []);
    frames.into_iter().next().expect("a picture")
}

#[test]
fn a_downloaded_quantiser_matrix_weights_the_coefficients() {
    // 7.4.2.3: (level * scale * weight) / 16 = 2 by default, 4 at weight 32.
    let default = with_matrix(None);
    let coarse = with_matrix(Some([32; 64]));

    let expect = |coefficient: i16| {
        let mut block = [0i16; 64];
        block[0] = 128 * 8;
        block[1] = coefficient;
        // 7.4.4 mismatch control.
        if (block[0] + block[1]) % 2 == 0 {
            block[63] ^= 1;
        }
        idct::transform(&mut block);
        block
    };
    for (frame, coefficient) in [(&default, 2i16), (&coarse, 4)] {
        let block = expect(coefficient);
        for y in 0..8 {
            let wanted: Vec<u8> = (0..8).map(|x| block[y * 8 + x] as u8).collect();
            assert_eq!(
                row(frame, 0, y)[..8],
                wanted[..],
                "row {y} at {coefficient}"
            );
        }
    }
    assert_ne!(row(&default, 0, 0), row(&coarse, 0, 0));
}

#[test]
fn a_skipped_macroblock_is_the_reference_unchanged() {
    // Three macroblocks across: a skipped one never begins or ends a slice.
    let setup = Setup {
        width: 48,
        ..Setup::default()
    };
    let mut bits = Bits::default();
    setup.sequence(&mut bits);
    setup.picture(&mut bits, 1, 3);
    setup.slice(&mut bits, 0);
    for (at, difference) in [(1u16, 72), (1, 20), (1, -40)] {
        bits.vlc(tables::ADDRESS_INCREMENT, at);
        bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
        bits.flat_block(true, difference);
        for _ in 0..3 {
            bits.flat_block(true, 0);
        }
        for _ in 0..2 {
            bits.flat_block(false, 0);
        }
    }
    // P picture: middle macroblock skipped, others zero vector, no residual.
    setup.picture(&mut bits, 2, 3);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_P, u16::from(FORWARD));
    bits.zero_vector();
    bits.vlc(tables::ADDRESS_INCREMENT, 2);
    bits.vlc(tables::MACROBLOCK_TYPE_P, u16::from(FORWARD));
    bits.zero_vector();
    setup.picture(&mut bits, 1, 3);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
    for _ in 0..4 {
        bits.flat_block(true, 0);
    }
    for _ in 0..2 {
        bits.flat_block(false, 0);
    }

    let (frames, errors) = run(&bits.bytes);
    assert_eq!(errors, []);
    assert_eq!(frames.len(), 2);
    for frame in &frames {
        let mut wanted = [0u8; 48];
        wanted[..16].fill(200);
        wanted[16..32].fill(220);
        wanted[32..].fill(180);
        assert_eq!(row(frame, 0, 0), wanted);
    }
}

#[test]
fn a_field_predicted_macroblock_in_a_frame_picture_takes_the_field_it_names() {
    // Broadcasts always use field DCT and field prediction in frame pictures.
    let setup = Setup {
        height: 32,
        progressive_sequence: false,
        frame_pred_frame_dct: false,
        ..Setup::default()
    };
    let mut bits = Bits::default();
    setup.sequence(&mut bits);

    // Field-coded intra: upper blocks even lines, lower blocks odd lines.
    setup.picture(&mut bits, 1, 3);
    for row in 0..2 {
        setup.slice(&mut bits, row);
        bits.vlc(tables::ADDRESS_INCREMENT, 1);
        bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
        bits.bit(1); // dct_type: field coded
        // Top field 200, bottom 100. The DC predictor resets per slice.
        for difference in [72, 0, -100, 0] {
            bits.flat_block(true, difference);
        }
        for _ in 0..2 {
            bits.flat_block(false, 0);
        }
    }

    // Both fields predicted from the reference's bottom field, zero vector.
    setup.picture(&mut bits, 2, 3);
    for row in 0..2 {
        setup.slice(&mut bits, row);
        bits.vlc(tables::ADDRESS_INCREMENT, 1);
        bits.vlc(tables::MACROBLOCK_TYPE_P, u16::from(FORWARD));
        bits.bits(2, 1); // frame_motion_type: field based
        for _ in 0..2 {
            bits.bit(1); // motion_vertical_field_select: the bottom field
            bits.zero_vector();
        }
    }
    setup.picture(&mut bits, 1, 3);
    for row in 0..2 {
        setup.slice(&mut bits, row);
        bits.vlc(tables::ADDRESS_INCREMENT, 1);
        bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
        bits.bit(0); // dct_type: frame coded
        for _ in 0..4 {
            bits.flat_block(true, 0);
        }
        for _ in 0..2 {
            bits.flat_block(false, 0);
        }
    }

    let (frames, errors) = run(&bits.bytes);
    assert_eq!(errors, []);
    assert_eq!(frames.len(), 2);

    for y in 0..32 {
        let wanted = if y % 2 == 0 { 200 } else { 100 };
        assert_eq!(row(&frames[0], 0, y), [wanted; 16], "intra row {y}");
    }
    for y in 0..32 {
        assert_eq!(row(&frames[1], 0, y), [100; 16], "predicted row {y}");
    }
}

#[test]
fn pictures_come_out_in_the_order_they_are_shown() {
    // Coded I P B B, shown I B B P.
    let setup = Setup::default();
    let mut bits = Bits::default();
    setup.sequence(&mut bits);
    let intra = |bits: &mut Bits, difference: i32| {
        setup.picture(bits, 1, 3);
        setup.slice(bits, 0);
        bits.vlc(tables::ADDRESS_INCREMENT, 1);
        bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
        bits.flat_block(true, difference);
        for _ in 0..3 {
            bits.flat_block(true, 0);
        }
        for _ in 0..2 {
            bits.flat_block(false, 0);
        }
    };
    intra(&mut bits, 72); // I, showing 200
    intra(&mut bits, -28); // the second reference, showing 100
    for _ in 0..2 {
        // B: (200 + 100 + 1) / 2 = 150.
        setup.picture(&mut bits, 3, 3);
        setup.slice(&mut bits, 0);
        bits.vlc(tables::ADDRESS_INCREMENT, 1);
        bits.vlc(tables::MACROBLOCK_TYPE_B, u16::from(FORWARD | BACKWARD));
        bits.zero_vector();
        bits.zero_vector();
    }
    intra(&mut bits, 0); // a third reference, which pushes the second out

    let (frames, errors) = run(&bits.bytes);
    assert_eq!(errors, []);
    let shown: Vec<u8> = frames.iter().map(|frame| row(frame, 0, 0)[0]).collect();
    assert_eq!(shown, [200, 150, 150, 100]);
}

#[test]
fn a_change_of_aspect_does_not_give_up_the_reference_standing() {
    // 臨時サービス changes `aspect_ratio_information` at the same size: not a reconfiguration.
    let square = Setup::default();
    let wide = Setup {
        aspect: 3, // 16:9
        ..Setup::default()
    };
    let mut bits = Bits::default();
    let intra = |bits: &mut Bits, setup: &Setup, difference: i32| {
        setup.picture(bits, 1, 3);
        setup.slice(bits, 0);
        bits.vlc(tables::ADDRESS_INCREMENT, 1);
        bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
        bits.flat_block(true, difference);
        for _ in 0..3 {
            bits.flat_block(true, 0);
        }
        for _ in 0..2 {
            bits.flat_block(false, 0);
        }
    };
    square.sequence(&mut bits);
    intra(&mut bits, &square, 72); // showing 200
    intra(&mut bits, &square, -28); // showing 100, held as the reference
    wide.sequence(&mut bits);
    intra(&mut bits, &wide, 22); // showing 150, which pushes 100 out

    let (frames, errors) = run(&bits.bytes);
    assert_eq!(errors, []);
    let shown: Vec<u8> = frames.iter().map(|frame| row(frame, 0, 0)[0]).collect();
    assert_eq!(shown, [200, 100]);
    assert_eq!(frames[0].format.aspect, (1, 1));
}

#[test]
fn a_macroblock_with_a_pattern_but_no_motion_is_predicted_from_where_it_stands() {
    // Table B.3 code `01`: not intra, no vector, with a residual.
    let setup = Setup::default();
    let mut bits = Bits::default();
    setup.sequence(&mut bits);
    setup.picture(&mut bits, 1, 3);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
    bits.flat_block(true, 72);
    for _ in 0..3 {
        bits.flat_block(true, 0);
    }
    for _ in 0..2 {
        bits.flat_block(false, 0);
    }

    setup.picture(&mut bits, 2, 3);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_P, u16::from(PATTERN));
    bits.vlc(tables::CODED_BLOCK_PATTERN, 0b100000);
    // 7.4.2.3 non-intra: (2 * 1 + 1) * 2 * 16 / 32 = 2, which only the mismatch
    // control makes visible.
    bits.coefficient(0, 4);
    bits.end_of_block();

    setup.picture(&mut bits, 1, 3);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));
    for _ in 0..4 {
        bits.flat_block(true, 0);
    }
    for _ in 0..2 {
        bits.flat_block(false, 0);
    }

    let (frames, errors) = run(&bits.bytes);
    assert_eq!(errors, []);
    assert_eq!(frames.len(), 2);
    let mut block = [0i16; 64];
    block[0] = (2 * 4 + 1) * 2 * 16 / 32;
    if block[0] % 2 == 0 {
        block[63] ^= 1;
    }
    idct::transform(&mut block);
    let wanted: Vec<u8> = (0..8).map(|x| (200 + block[x]) as u8).collect();
    assert_eq!(row(&frames[1], 0, 0)[..8], wanted[..]);
    assert_eq!(row(&frames[1], 0, 0)[8..], [200; 8]);
}

/// Interlaced MPEG-2 made by ffmpeg (`testdata/make.sh`): frame pictures with field DCT and
/// field prediction, top field first, an open GOP after the first.
#[test]
fn an_encoded_stream_decodes_to_the_pictures_it_says_it_holds() {
    let (width, height) = (352, 240);
    let bytes = crate::testdata("mpeg2-interlaced.ts");
    let mut splitter = Splitter::new(Codec::Mpeg2, Framing::StartCodes);
    let mut decoder = Mpeg2::new();
    let mut frames = 0;
    let mut coded = 0;
    let mut errors = Vec::new();
    let mut structures = Vec::new();
    for unit in packetised(&bytes) {
        let mut accesses: Vec<Access> = Vec::new();
        splitter.push(&unit, &mut accesses);
        for access in accesses {
            let Some(format) = access.format else {
                continue;
            };
            coded += 1;
            structures.push(access.picture.structure);
            if let Err(error) = decoder.decode(0, &access.picture, format, &unit[access.range]) {
                errors.push(error);
            }
            while let Some(frame) = decoder.take() {
                assert_eq!((frame.format.width, frame.format.height), (width, height));
                assert_eq!(
                    (frame.planes[0].width, frame.planes[0].height),
                    (width as usize, height as usize)
                );
                assert_eq!(
                    (frame.planes[1].width, frame.planes[1].height),
                    (width as usize / 2, height as usize / 2)
                );
                assert!(!frame.progressive);
                assert!(frame.top_field_first);
                frames += 1;
            }
        }
    }
    decoder.flush();
    while decoder.take().is_some() {
        frames += 1;
    }

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(coded, 36, "coded pictures");
    assert_eq!(frames, coded, "frames");
    assert!(structures.iter().all(|structure| !structure.field()));
}

/// No slice reads past the length a platform decoder is given.
#[test]
fn no_slice_reads_past_the_length_the_hardware_is_given() {
    for path in ["mpeg2-interlaced.ts", "mpeg2-telecine.ts"] {
        let bytes = crate::testdata(path);
        super::slice::OVERRUNS.with(|count| count.set(0));
        let mut splitter = Splitter::new(Codec::Mpeg2, Framing::StartCodes);
        let mut decoder = Mpeg2::new();
        let mut coded = 0;
        for unit in packetised(&bytes) {
            let mut accesses: Vec<Access> = Vec::new();
            splitter.push(&unit, &mut accesses);
            for access in accesses {
                let Some(format) = access.format else {
                    continue;
                };
                coded += 1;
                let _ = decoder.decode(0, &access.picture, format, &unit[access.range]);
                while decoder.take().is_some() {}
            }
        }
        assert!(coded > 20, "{path}: {coded} coded pictures");
        let overruns = super::slice::OVERRUNS.with(std::cell::Cell::get);
        assert_eq!(
            overruns, 0,
            "{path}: {overruns} slices read past their length"
        );
    }
}

/// Pulls one programme's MPEG-2 video out of a TS, one unit per PES packet.
fn packetised(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut maps: Vec<u16> = Vec::new();
    let mut video = None;
    let mut sections: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut payloads = Vec::new();
    let mut current: Vec<u8> = Vec::new();

    for packet in bytes.as_chunks::<188>().0 {
        if packet[0] != 0x47 {
            continue;
        }
        let pid = u16::from(packet[1] & 0x1f) << 8 | u16::from(packet[2]);
        let starts = packet[1] & 0x40 != 0;
        let control = packet[3] >> 4 & 3;
        let mut at = 4;
        if control & 2 != 0 {
            at += 1 + usize::from(packet[4]);
        }
        if control & 1 == 0 || at >= 188 {
            continue;
        }
        let payload = &packet[at..];

        if Some(pid) == video {
            if starts {
                if !current.is_empty() {
                    payloads.push(std::mem::take(&mut current));
                }
                if payload.len() >= 9 {
                    let header = 9 + usize::from(payload[8]);
                    if payload.len() > header {
                        current.extend_from_slice(&payload[header..]);
                    }
                }
            } else if !current.is_empty() {
                current.extend_from_slice(payload);
            }
            continue;
        }
        if pid != 0 && !maps.contains(&pid) {
            continue;
        }
        let payload = if starts {
            &payload[1 + usize::from(payload[0])..]
        } else {
            payload
        };
        let section = match sections.iter_mut().find(|(held, _)| *held == pid) {
            Some((_, section)) => section,
            None => {
                sections.push((pid, Vec::new()));
                &mut sections.last_mut().expect("just pushed").1
            }
        };
        if starts {
            section.clear();
        }
        section.extend_from_slice(payload);
        if section.len() < 3 {
            continue;
        }
        let length = 3 + (usize::from(section[1] & 0x0f) << 8 | usize::from(section[2]));
        if section.len() < length {
            continue;
        }
        let section = section.clone();
        if pid == 0 {
            let mut at = 8;
            while at + 4 <= length - 4 {
                let map = u16::from(section[at + 2] & 0x1f) << 8 | u16::from(section[at + 3]);
                if u16::from_be_bytes([section[at], section[at + 1]]) != 0 && !maps.contains(&map) {
                    maps.push(map);
                }
                at += 4;
            }
        } else {
            let mut at = 12 + (usize::from(section[10] & 0x0f) << 8 | usize::from(section[11]));
            while at + 5 <= length - 4 {
                let stream = u16::from(section[at + 1] & 0x1f) << 8 | u16::from(section[at + 2]);
                if Codec::of_stream_type(section[at]) == Some(Codec::Mpeg2) && video.is_none() {
                    video = Some(stream);
                }
                at += 5 + (usize::from(section[at + 3] & 0x0f) << 8 | usize::from(section[at + 4]));
            }
        }
    }
    if !current.is_empty() {
        payloads.push(current);
    }
    payloads
}

#[test]
fn a_stream_that_is_not_mpeg_2_is_refused_rather_than_guessed_at() {
    // No coding extension means MPEG-1.
    let setup = Setup::default();
    let mut bits = Bits::default();
    setup.sequence(&mut bits);
    bits.start(0x00);
    bits.bits(10, 0);
    bits.bits(3, 1);
    bits.bits(16, 0xffff);
    bits.bit(0);
    setup.slice(&mut bits, 0);
    bits.vlc(tables::ADDRESS_INCREMENT, 1);
    bits.vlc(tables::MACROBLOCK_TYPE_I, u16::from(INTRA));

    let (frames, errors) = run(&bits.bytes);
    assert!(frames.is_empty());
    assert_eq!(
        errors,
        [crate::video::Error::Unsupported(
            "a picture with no coding extension"
        )]
    );
}

#[test]
fn giving_up_everything_held_leaves_a_decoder_that_still_works() {
    let mut decoder = Mpeg2::new();
    decoder.reset();
    assert!(decoder.take().is_none());
}
