// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! スライス層から下（マクロブロック、動きベクトル、係数、逆量子化、書き戻し）。
//! スライスは互いに依存しない（予測子も量子化も持ち越さない）。

use super::predict::{Target, fetch, fetch_average};
use super::tables::{BACKWARD, FORWARD, INTRA, PATTERN, QUANT, quantiser_scale};
use super::vlc::{self, END_OF_BLOCK, ESCAPE};
use super::{Mpeg2, State};
use super::{idct, tables};
use crate::es::{Coding, Structure};
use crate::video::Error;
use crate::video::bits::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Frame,
    /// One vector per field: two in a frame picture, one in a field picture.
    Field,
    /// 16x8: upper and lower halves. Field pictures only.
    Halves,
    DualPrime,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Motion {
    pub kind: Kind,
    pub uses: [bool; 2],
    /// By direction, prediction and axis. Vertical is in the units of what it
    /// predicts (field lines for a field prediction).
    pub mv: [[[i32; 2]; 2]; 2],
    pub field: [[usize; 2]; 2],
    /// Dual prime vector to the opposite parity, for top then bottom field.
    pub dual: [[i32; 2]; 2],
}

struct Slice {
    quantiser: i32,
    /// By direction, prediction and axis. Vertical is always in frame lines, so it
    /// is halved for field predictions and doubled back.
    pmv: [[[i32; 2]; 2]; 2],
    dc: [i32; 3],
    /// What a skipped macroblock in a B picture repeats.
    last: Motion,
}

pub struct Prediction {
    luma: [u8; 256],
    chroma: [[u8; 64]; 2],
}

impl Default for Prediction {
    fn default() -> Self {
        Self {
            luma: [128; 256],
            chroma: [[128; 64]; 2],
        }
    }
}

struct Rectangle {
    buffer: usize,
    field: Option<usize>,
    /// First luma row inside the macroblock, and the count.
    luma: (usize, usize),
    chroma: (usize, usize),
    /// Row step: two for one field of a frame-coded macroblock.
    step: usize,
    at: (usize, usize),
    /// Half luma samples.
    mv: [i32; 2],
    /// Average into what is predicted rather than overwrite. Only dual prime: its
    /// two rectangles cover the same samples, unlike field and 16x8.
    average: bool,
}

#[cfg(test)]
thread_local! {
    pub(super) static OVERRUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Mpeg2 {
    /// Decodes one slice. `row` is the start code's macroblock row; `padding` the
    /// zeroes cut off with the access unit ([`super::headers::slice_end`]).
    pub(super) fn slice(
        &mut self,
        state: &State,
        row: usize,
        bytes: &[u8],
        padding: usize,
    ) -> Result<(), Error> {
        let tables = vlc::tables();
        let mut reader = Reader::new(bytes);
        let mut row = row;
        // Only above 2800 lines; read anyway or the slice desynchronises.
        if self.format.height > 2800 {
            row |= (reader.bits(3) as usize) << 7;
        }
        if row >= state.mb_height {
            return Err(Error::Damaged("a slice above the top of the picture"));
        }
        let mut slice = Slice {
            quantiser: quantiser_scale(reader.bits(5), state.coding.q_scale_type),
            pmv: [[[0; 2]; 2]; 2],
            dc: [1 << (state.coding.intra_dc_precision + 7); 3],
            last: Motion::default(),
        };
        if reader.peek(1) == 1 {
            reader.skip(1 + 1 + 7);
        }
        while reader.bit() == 1 {
            reader.skip(8);
        }

        let mut prediction = Prediction::default();
        let mut address = (row * state.mb_width) as i32 - 1;
        let mut first = true;
        // 23 zeroes can only be the next start code (the reader pads with zeroes).
        while reader.peek(23) != 0 {
            let mut increment = 0i32;
            loop {
                let Some(value) = tables.address_increment.read(&mut reader) else {
                    return Err(Error::Damaged("a macroblock address that is not a code"));
                };
                match value {
                    tables::ESCAPE => increment += 33,
                    tables::STUFFING => {}
                    value => {
                        increment += i32::from(value);
                        break;
                    }
                }
            }
            if first {
                address += increment;
                first = false;
            } else {
                // スキップするマクロブロックもピクチャーの中に限る（最後の行で大きくスキップすると範囲外に書く）。
                let total = (state.mb_width * state.mb_height) as i64;
                if i64::from(address) + i64::from(increment) > total {
                    return Err(Error::Damaged("a macroblock outside the picture"));
                }
                for _ in 1..increment {
                    address += 1;
                    self.skipped(state, &mut slice, &mut prediction, address)?;
                }
                address += 1;
            }
            if address < 0 || address as usize >= state.mb_width * state.mb_height {
                return Err(Error::Damaged("a macroblock outside the picture"));
            }
            self.macroblock(state, &mut slice, &mut prediction, &mut reader, address)?;
        }
        // Past the end reads zeroes here, not on a platform decoder; tests count overruns.
        #[cfg(test)]
        if reader.at() > (bytes.len() + padding) * 8 {
            OVERRUNS.with(|count| count.set(count.get() + 1));
        }
        #[cfg(not(test))]
        let _ = padding;
        Ok(())
    }

    /// A macroblock not coded at all. P: zero vector, predictors reset.
    /// B: repeats the last macroblock's directions.
    fn skipped(
        &mut self,
        state: &State,
        slice: &mut Slice,
        prediction: &mut Prediction,
        address: i32,
    ) -> Result<(), Error> {
        let motion = if state.kind == Coding::B {
            // 7.6.6: keeps the directions, not the motion type (frame or own-parity field).
            // Repeating a field prediction puts the macroblock entirely elsewhere.
            let mut motion = Motion {
                kind: if state.structure == Structure::Frame {
                    Kind::Frame
                } else {
                    Kind::Field
                },
                uses: slice.last.uses,
                field: [[state.parity; 2]; 2],
                ..Motion::default()
            };
            for direction in 0..2 {
                motion.mv[direction][0] = slice.pmv[direction][0];
                motion.mv[direction][1] = slice.pmv[direction][0];
            }
            motion
        } else {
            slice.pmv = [[[0; 2]; 2]; 2];
            Motion {
                kind: if state.structure == Structure::Frame {
                    Kind::Frame
                } else {
                    Kind::Field
                },
                uses: [true, false],
                field: [[state.parity; 2]; 2],
                ..Motion::default()
            }
        };
        slice.dc = [1 << (state.coding.intra_dc_precision + 7); 3];
        let (mb_x, mb_y) = (
            address as usize % state.mb_width,
            address as usize / state.mb_width,
        );
        self.predict(state, &motion, mb_x, mb_y, prediction);
        self.store(state, mb_x, mb_y, Some(prediction), &[[0; 64]; 6], false);
        Ok(())
    }

    fn macroblock(
        &mut self,
        state: &State,
        slice: &mut Slice,
        prediction: &mut Prediction,
        reader: &mut Reader<'_>,
        address: i32,
    ) -> Result<(), Error> {
        let tables = vlc::tables();
        let table = match state.kind {
            Coding::I => &tables.macroblock_type_i,
            Coding::P => &tables.macroblock_type_p,
            Coding::B => &tables.macroblock_type_b,
            Coding::Unknown => return Err(Error::Damaged("a picture of no coding type")),
        };
        let Some(modes) = table.read(reader) else {
            return Err(Error::Damaged("a macroblock type that is not a code"));
        };
        let modes = modes as u8;
        let intra = modes & INTRA != 0;
        let frame = state.structure == Structure::Frame;

        // 6.2.5.1 order: motion type, DCT type, quantiser, then vectors.
        let motion_type = if intra || modes & (FORWARD | BACKWARD) == 0 {
            None
        } else if frame && state.coding.frame_pred_frame_dct {
            Some(2)
        } else {
            Some(reader.bits(2))
        };
        let field_dct = frame
            && !state.coding.frame_pred_frame_dct
            && (intra || modes & PATTERN != 0)
            && reader.bit() == 1;
        if modes & QUANT != 0 {
            slice.quantiser = quantiser_scale(reader.bits(5), state.coding.q_scale_type);
        }

        let mut motion = Motion::default();
        if intra {
            if state.coding.concealment_motion_vectors {
                // Concealment vectors are unused but still move the predictors.
                if !frame {
                    reader.skip(1);
                }
                let mut spare = Motion {
                    uses: [true, false],
                    ..Motion::default()
                };
                self.vectors(state, slice, reader, &mut spare, 0, Kind::Frame);
                reader.skip(1); // marker_bit
            } else {
                slice.pmv = [[[0; 2]; 2]; 2];
            }
        } else {
            motion.uses = [modes & FORWARD != 0, modes & BACKWARD != 0];
            motion.kind = match (motion_type, frame) {
                (None, true) => Kind::Frame,
                (None, false) => Kind::Field,
                (Some(1), _) => Kind::Field,
                (Some(2), true) => Kind::Frame,
                (Some(2), false) => Kind::Halves,
                (Some(3), _) => Kind::DualPrime,
                _ => return Err(Error::Damaged("a reserved motion type")),
            };
            if motion_type.is_none() {
                // P with pattern and no motion: zero vector, predictors reset.
                slice.pmv = [[[0; 2]; 2]; 2];
                motion.uses = [true, false];
                motion.field = [[state.parity; 2]; 2];
            } else {
                let kind = motion.kind;
                for direction in 0..2 {
                    if motion.uses[direction] {
                        self.vectors(state, slice, reader, &mut motion, direction, kind);
                    }
                }
            }
            slice.dc = [1 << (state.coding.intra_dc_precision + 7); 3];
            slice.last = motion;
        }

        let pattern = if intra {
            0x3f
        } else if modes & PATTERN != 0 {
            let Some(pattern) = tables.coded_block_pattern.read(reader) else {
                return Err(Error::Damaged("a coded block pattern that is not a code"));
            };
            pattern
        } else {
            0
        };

        let mut blocks = [[0i16; 64]; 6];
        for (index, coefficients) in blocks.iter_mut().enumerate() {
            if pattern & 1 << (5 - index) == 0 {
                continue;
            }
            block(state, slice, reader, index, intra, coefficients)?;
            idct::transform(coefficients);
        }

        let (mb_x, mb_y) = (
            address as usize % state.mb_width,
            address as usize / state.mb_width,
        );
        if intra {
            self.store(state, mb_x, mb_y, None, &blocks, field_dct);
        } else {
            self.predict(state, &motion, mb_x, mb_y, prediction);
            self.store(state, mb_x, mb_y, Some(prediction), &blocks, field_dct);
        }
        Ok(())
    }

    /// Reads one direction's vectors and field selects, updating the predictors (7.6.3.1).
    fn vectors(
        &self,
        state: &State,
        slice: &mut Slice,
        reader: &mut Reader<'_>,
        motion: &mut Motion,
        direction: usize,
        kind: Kind,
    ) {
        let frame = state.structure == Structure::Frame;
        let f_code = state.coding.f_code[direction];
        match kind {
            Kind::Frame => {
                for (axis, &code) in f_code.iter().enumerate() {
                    let value = delta(reader, code, slice.pmv[direction][0][axis]);
                    slice.pmv[direction][0][axis] = value;
                    slice.pmv[direction][1][axis] = value;
                    motion.mv[direction][0][axis] = value;
                }
            }
            Kind::Field if frame => {
                // Field vector counts field lines, the predictor frame lines: a factor of two.
                for which in 0..2 {
                    motion.field[direction][which] = reader.bit() as usize;
                    let value = delta(reader, f_code[0], slice.pmv[direction][which][0]);
                    slice.pmv[direction][which][0] = value;
                    motion.mv[direction][which][0] = value;
                    let value = delta(reader, f_code[1], slice.pmv[direction][which][1] >> 1);
                    slice.pmv[direction][which][1] = value * 2;
                    motion.mv[direction][which][1] = value;
                }
            }
            Kind::Field => {
                motion.field[direction][0] = reader.bit() as usize;
                for (axis, &code) in f_code.iter().enumerate() {
                    let value = delta(reader, code, slice.pmv[direction][0][axis]);
                    slice.pmv[direction][0][axis] = value;
                    slice.pmv[direction][1][axis] = value;
                    motion.mv[direction][0][axis] = value;
                }
            }
            Kind::Halves => {
                for which in 0..2 {
                    motion.field[direction][which] = reader.bit() as usize;
                    for (axis, &code) in f_code.iter().enumerate() {
                        let value = delta(reader, code, slice.pmv[direction][which][axis]);
                        slice.pmv[direction][which][axis] = value;
                        motion.mv[direction][which][axis] = value;
                    }
                }
            }
            Kind::DualPrime => {
                // 7.6.3.6: derive the opposite-parity vector, scaled by field distance, plus
                // the coded correction and the one-line field offset.
                let shift = u32::from(frame);
                let x = delta(reader, f_code[0], slice.pmv[direction][0][0]);
                slice.pmv[direction][0][0] = x;
                slice.pmv[direction][1][0] = x;
                let dx = dmvector(reader);
                let y = delta(reader, f_code[1], slice.pmv[direction][0][1] >> shift);
                let dy = dmvector(reader);
                slice.pmv[direction][0][1] = y << shift;
                slice.pmv[direction][1][1] = y << shift;
                motion.mv[direction][0] = [x, y];
                if frame {
                    let near = if state.top_field_first { 1 } else { 3 };
                    motion.dual[0] = [
                        ((x * near + i32::from(x > 0)) >> 1) + dx,
                        ((y * near + i32::from(y > 0)) >> 1) + dy - 1,
                    ];
                    let far = 4 - near;
                    motion.dual[1] = [
                        ((x * far + i32::from(x > 0)) >> 1) + dx,
                        ((y * far + i32::from(y > 0)) >> 1) + dy + 1,
                    ];
                } else {
                    let line = if state.parity == 0 { -1 } else { 1 };
                    motion.dual[0] = [
                        ((x + i32::from(x > 0)) >> 1) + dx,
                        ((y + i32::from(y > 0)) >> 1) + dy + line,
                    ];
                }
            }
        }
    }

    fn predict(
        &self,
        state: &State,
        motion: &Motion,
        mb_x: usize,
        mb_y: usize,
        prediction: &mut Prediction,
    ) {
        let mut placed = false;
        for direction in 0..2 {
            if !motion.uses[direction] {
                continue;
            }
            let mut rectangles = [const { None }; 4];
            self.rectangles(state, motion, direction, mb_x, mb_y, &mut rectangles);
            // A direction whose reference is missing (mid-GOP start) draws nothing and
            // must not count as placed, or the next one averages with stale samples.
            let mut drawn = false;
            for rectangle in rectangles.iter().flatten() {
                // Backward averages with forward; halves of one direction do not.
                self.apply(rectangle, prediction, placed || rectangle.average);
                drawn = true;
            }
            placed |= drawn;
        }
        if !placed {
            // No reference (mid-GOP start): grey until the next intra macroblock.
            *prediction = Prediction::default();
        }
    }

    fn rectangles(
        &self,
        state: &State,
        motion: &Motion,
        direction: usize,
        mb_x: usize,
        mb_y: usize,
        out: &mut [Option<Rectangle>; 4],
    ) {
        let frame = state.structure == Structure::Frame;
        let mut count = 0;
        let mut push = |rectangle: Option<Rectangle>| {
            if let Some(rectangle) = rectangle {
                out[count] = Some(rectangle);
                count += 1;
            }
        };
        let field = |parity: usize| state.field_source[direction][parity];
        match motion.kind {
            Kind::Frame if frame => {
                push(state.frame_source[direction].map(|buffer| Rectangle {
                    buffer,
                    field: None,
                    luma: (0, 16),
                    chroma: (0, 8),
                    step: 1,
                    at: (mb_x * 16, mb_y * 16),
                    mv: motion.mv[direction][0],
                    average: false,
                }));
            }
            // In a field picture, frame prediction means the picture's own field.
            Kind::Frame => {
                let parity = state.parity;
                push(field(parity).map(|buffer| Rectangle {
                    buffer,
                    field: Some(parity),
                    luma: (0, 16),
                    chroma: (0, 8),
                    step: 1,
                    at: (mb_x * 16, mb_y * 16),
                    mv: motion.mv[direction][0],
                    average: false,
                }));
            }
            Kind::Field if frame => {
                for which in 0..2 {
                    let parity = motion.field[direction][which];
                    push(field(parity).map(|buffer| Rectangle {
                        buffer,
                        field: Some(parity),
                        luma: (which, 8),
                        chroma: (which, 4),
                        step: 2,
                        at: (mb_x * 16, mb_y * 8),
                        mv: motion.mv[direction][which],
                        average: false,
                    }));
                }
            }
            Kind::Field => {
                let parity = motion.field[direction][0];
                push(field(parity).map(|buffer| Rectangle {
                    buffer,
                    field: Some(parity),
                    luma: (0, 16),
                    chroma: (0, 8),
                    step: 1,
                    at: (mb_x * 16, mb_y * 16),
                    mv: motion.mv[direction][0],
                    average: false,
                }));
            }
            Kind::Halves => {
                for which in 0..2 {
                    let parity = motion.field[direction][which];
                    push(field(parity).map(|buffer| Rectangle {
                        buffer,
                        field: Some(parity),
                        luma: (which * 8, 8),
                        chroma: (which * 4, 4),
                        step: 1,
                        at: (mb_x * 16, mb_y * 16 + which * 8),
                        mv: motion.mv[direction][which],
                        average: false,
                    }));
                }
            }
            Kind::DualPrime if frame => {
                for which in 0..2 {
                    let both = [
                        (which, motion.mv[direction][0], false),
                        (1 - which, motion.dual[which], true),
                    ];
                    for (parity, mv, average) in both {
                        push(field(parity).map(|buffer| Rectangle {
                            buffer,
                            field: Some(parity),
                            luma: (which, 8),
                            chroma: (which, 4),
                            step: 2,
                            at: (mb_x * 16, mb_y * 8),
                            mv,
                            average,
                        }));
                    }
                }
            }
            Kind::DualPrime => {
                let own = state.parity;
                let both = [
                    (own, motion.mv[direction][0], false),
                    (1 - own, motion.dual[0], true),
                ];
                for (parity, mv, average) in both {
                    push(field(parity).map(|buffer| Rectangle {
                        buffer,
                        field: Some(parity),
                        luma: (0, 16),
                        chroma: (0, 8),
                        step: 1,
                        at: (mb_x * 16, mb_y * 16),
                        mv,
                        average,
                    }));
                }
            }
        }
    }

    fn apply(&self, rectangle: &Rectangle, prediction: &mut Prediction, average: bool) {
        let buffer = &self.frames[rectangle.buffer];
        let view = buffer.view(0, rectangle.field);
        let mut target = Target {
            data: &mut prediction.luma,
            at: rectangle.luma.0 * 16,
            stride: 16 * rectangle.step,
            width: 16,
            height: rectangle.luma.1,
        };
        let x = rectangle.at.0 as i32 * 2 + rectangle.mv[0];
        let y = rectangle.at.1 as i32 * 2 + rectangle.mv[1];
        if average {
            fetch_average(&mut target, &view, x, y);
        } else {
            fetch(&mut target, &view, x, y);
        }
        // 7.6.3.7: chroma vector is luma / 2, truncated toward zero.
        let x = (rectangle.at.0 / 2) as i32 * 2 + rectangle.mv[0] / 2;
        let y = (rectangle.at.1 / 2) as i32 * 2 + rectangle.mv[1] / 2;
        for plane in 0..2 {
            let view = buffer.view(plane + 1, rectangle.field);
            let mut target = Target {
                data: &mut prediction.chroma[plane],
                at: rectangle.chroma.0 * 8,
                stride: 8 * rectangle.step,
                width: 8,
                height: rectangle.chroma.1,
            };
            if average {
                fetch_average(&mut target, &view, x, y);
            } else {
                fetch(&mut target, &view, x, y);
            }
        }
    }

    fn store(
        &mut self,
        state: &State,
        mb_x: usize,
        mb_y: usize,
        prediction: Option<&Prediction>,
        blocks: &[[i16; 64]; 6],
        field_dct: bool,
    ) {
        let field = state.field();
        let buffer = &mut self.frames[self.cur];
        let (start, stride) = buffer.origin(0, field);
        for (index, block) in blocks[..4].iter().enumerate() {
            let left = (index & 1) * 8;
            for row in 0..8 {
                // Field DCT: upper blocks are one field, lower the other.
                let line = if field_dct {
                    2 * row + (index >> 1)
                } else {
                    (index >> 1) * 8 + row
                };
                let at = start + (mb_y * 16 + line) * stride + mb_x * 16 + left;
                for column in 0..8 {
                    let residual = i32::from(block[row * 8 + column]);
                    let value = match prediction {
                        Some(prediction) => {
                            i32::from(prediction.luma[line * 16 + left + column]) + residual
                        }
                        None => residual,
                    };
                    buffer.planes[0][at + column] = value.clamp(0, 255) as u8;
                }
            }
        }
        for (plane, block) in blocks[4..].iter().enumerate() {
            let (start, stride) = buffer.origin(plane + 1, field);
            for row in 0..8 {
                let at = start + (mb_y * 8 + row) * stride + mb_x * 8;
                for column in 0..8 {
                    let residual = i32::from(block[row * 8 + column]);
                    let value = match prediction {
                        Some(prediction) => {
                            i32::from(prediction.chroma[plane][row * 8 + column]) + residual
                        }
                        None => residual,
                    };
                    buffer.planes[plane + 1][at + column] = value.clamp(0, 255) as u8;
                }
            }
        }
    }
}

fn block(
    state: &State,
    slice: &mut Slice,
    reader: &mut Reader<'_>,
    index: usize,
    intra: bool,
    out: &mut [i16; 64],
) -> Result<(), Error> {
    let tables = vlc::tables();
    let luma = index < 4;
    let matrix = match (intra, luma) {
        (true, true) => &state.matrices.intra,
        (true, false) => &state.matrices.chroma_intra,
        (false, true) => &state.matrices.non_intra,
        (false, false) => &state.matrices.chroma_non_intra,
    };
    let quantiser = slice.quantiser;
    let coefficients = if intra && state.coding.intra_vlc_format {
        &tables.coefficients_one
    } else {
        &tables.coefficients_zero
    };
    // 7.4.4 mismatch control: make the sum odd, so IEEE 1180-conforming decoders
    // do not drift apart over P pictures.
    let mut mismatch = 1i32;
    let mut at: i32 = -1;

    if intra {
        let (table, component) = if luma {
            (&tables.dc_size_luma, 0)
        } else {
            (&tables.dc_size_chroma, index - 3)
        };
        let Some(size) = table.read(reader) else {
            return Err(Error::Damaged("a direct current size that is not a code"));
        };
        let size = u32::from(size);
        let difference = if size == 0 {
            0
        } else {
            let value = reader.bits(size) as i32;
            if value < 1 << (size - 1) {
                value - (1 << size) + 1
            } else {
                value
            }
        };
        slice.dc[component] += difference;
        let value =
            (slice.dc[component] * (8 >> state.coding.intra_dc_precision)).clamp(-2048, 2047);
        out[0] = value as i16;
        mismatch ^= value;
        at = 0;
    } else if reader.peek(1) == 1 {
        // B.14: a non-intra block's first coefficient `1s` is level one, not EOB.
        reader.skip(1);
        let negative = reader.bit() == 1;
        let value = non_intra_level(1, negative, quantiser, matrix[0]);
        out[0] = value as i16;
        mismatch ^= value;
        at = 0;
    }

    loop {
        let Some(code) = coefficients.read(reader) else {
            return Err(Error::Damaged("a coefficient that is not a code"));
        };
        let (run, level, negative) = match code {
            END_OF_BLOCK => break,
            ESCAPE => {
                let run = reader.bits(6) as i32;
                let level = reader.signed(12);
                (run, level.abs(), level < 0)
            }
            code => {
                let run = vlc::run(code) as i32;
                let level = vlc::level(code);
                (run, level, reader.bit() == 1)
            }
        };
        at += run + 1;
        if at > 63 {
            return Err(Error::Damaged(
                "a block with more coefficients than it has places",
            ));
        }
        let place = usize::from(state.scan[at as usize]);
        let value = if intra {
            intra_level(level, negative, quantiser, matrix[place])
        } else {
            non_intra_level(level, negative, quantiser, matrix[place])
        };
        out[place] = value as i16;
        mismatch ^= value;
    }
    out[63] ^= (mismatch & 1) as i16;
    Ok(())
}

/// 7.4.2.3, intra AC coefficient.
fn intra_level(level: i32, negative: bool, quantiser: i32, weight: u8) -> i32 {
    let value = (level * quantiser * i32::from(weight)) >> 4;
    let value = if negative { -value } else { value };
    value.clamp(-2048, 2047)
}

/// 7.4.2.3, non-intra. The extra one before halving makes the dead zone.
fn non_intra_level(level: i32, negative: bool, quantiser: i32, weight: u8) -> i32 {
    if level == 0 {
        return 0;
    }
    let value = ((2 * level + 1) * quantiser * i32::from(weight)) >> 5;
    let value = if negative { -value } else { value };
    value.clamp(-2048, 2047)
}

/// One motion vector difference added to the predictor, wrapped to the
/// `f_code` range (7.6.3.1).
fn delta(reader: &mut Reader<'_>, f_code: u32, prediction: i32) -> i32 {
    let Some(code) = vlc::tables().motion_code.read(reader) else {
        return prediction;
    };
    if code == 0 {
        return prediction;
    }
    let negative = reader.bit() == 1;
    let shift = f_code.saturating_sub(1);
    let mut value = i32::from(code);
    if shift > 0 {
        value = ((value - 1) << shift) + reader.bits(shift) as i32 + 1;
    }
    if negative {
        value = -value;
    }
    let width = 5 + shift;
    let sign = 1 << (width - 1);
    (((prediction + value) & ((1 << width) - 1)) ^ sign) - sign
}

/// Table B.11: dual prime correction.
fn dmvector(reader: &mut Reader<'_>) -> i32 {
    if reader.bit() == 0 {
        0
    } else {
        1 - 2 * reader.bit() as i32
    }
}
