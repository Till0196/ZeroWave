// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-2 のソフトウェアデコーダー（Main profile / Main level、4:2:0、8 ビット）。
//! ピクチャーヘッダーから上は [`crate::es::mpeg2`]、量子化マトリクスと並べ直しは
//! [`headers`] と [`reorder`] が持つ。

pub mod headers;
mod idct;
mod predict;
pub mod reorder;
mod slice;
mod tables;
#[cfg(test)]
mod tests;
mod vlc;

use std::collections::VecDeque;

use crate::es::{Coding, Format, Picture, StartCodes, Structure};
use crate::video::{Decoder, Error, Frame, Plane};

use headers::{Chroma, Matrices, Order, PictureCoding};
use predict::View;
use reorder::Reorder;

/// Decoded picture's coding, kept so a frame from two fields can still report it.
#[derive(Debug, Clone, Copy, Default)]
struct Held {
    tag: u64,
    structure: Structure,
    top_field_first: bool,
    repeat_first_field: bool,
    progressive: bool,
}

/// One frame's samples. A field pair shares one buffer, since the second
/// field may be predicted from the first.
#[derive(Debug, Default, Clone)]
struct Buffer {
    planes: [Vec<u8>; 3],
    stride: [usize; 3],
    height: [usize; 3],
    held: Held,
}

impl Buffer {
    fn new(stride: [usize; 3], height: [usize; 3]) -> Self {
        Self {
            planes: std::array::from_fn(|plane| vec![128; stride[plane] * height[plane]]),
            stride,
            height,
            held: Held::default(),
        }
    }

    fn origin(&self, plane: usize, field: Option<usize>) -> (usize, usize) {
        match field {
            None => (0, self.stride[plane]),
            Some(parity) => (parity * self.stride[plane], 2 * self.stride[plane]),
        }
    }

    fn view(&self, plane: usize, field: Option<usize>) -> View<'_> {
        let (at, stride) = self.origin(plane, field);
        View {
            data: &self.planes[plane],
            at,
            stride,
            width: self.stride[plane],
            height: match field {
                None => self.height[plane],
                Some(_) => self.height[plane] / 2,
            },
        }
    }
}

struct State {
    coding: PictureCoding,
    kind: Coding,
    structure: Structure,
    top_field_first: bool,
    parity: usize,
    matrices: Matrices,
    scan: &'static [u8; 64],
    /// Reference field source by direction and parity. The second field of an
    /// I/P frame takes the other parity from its own frame (I field + P field entry).
    field_source: [[Option<usize>; 2]; 2],
    frame_source: [Option<usize>; 2],
    mb_width: usize,
    mb_height: usize,
}

impl State {
    fn field(&self) -> Option<usize> {
        match self.structure {
            Structure::Frame => None,
            _ => Some(self.parity),
        }
    }
}

/// The MPEG-2 decoder. Three buffers: two references and the current picture.
#[derive(Debug)]
pub struct Mpeg2 {
    format: Format,
    matrices: Matrices,
    frames: Vec<Buffer>,
    /// Shared with the platform decoders so they agree on output order.
    order: Reorder<usize>,
    cur: usize,
    ready: VecDeque<Frame>,
}

impl Default for Mpeg2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mpeg2 {
    pub fn new() -> Self {
        Self {
            format: Format::default(),
            // Weights are looked up in raster order; 4:2:0 chroma uses the luma matrix.
            matrices: Matrices::new(Order::Raster, Chroma::LikeLuma),
            frames: Vec::new(),
            order: Reorder::new(),
            cur: 0,
            ready: VecDeque::new(),
        }
    }

    /// Takes up a new sample geometry (a change of channel).
    fn reconfigure(&mut self, format: Format) {
        // Put out the standing reference first; it is cropped to the format it was decoded in.
        self.flush();
        self.format = format;
        let mb_width = (format.width as usize).div_ceil(16);
        // Interlaced: round up per field, so 1080 becomes 1088.
        let mb_height = if format.progressive {
            (format.height as usize).div_ceil(16)
        } else {
            2 * (format.height as usize).div_ceil(32)
        };
        let stride = [mb_width * 16, mb_width * 8, mb_width * 8];
        let height = [mb_height * 16, mb_height * 8, mb_height * 8];
        self.frames = (0..3).map(|_| Buffer::new(stride, height)).collect();
        self.matrices = Matrices::new(Order::Raster, Chroma::LikeLuma);
        self.cur = 0;
    }

    fn free(&self) -> usize {
        (0..self.frames.len())
            .find(|&at| Some(at) != self.order.older() && Some(at) != self.order.newer())
            .unwrap_or(0)
    }

    fn start(&mut self, tag: u64, picture: &Picture) {
        let mut sink = Emit {
            frames: &self.frames,
            format: self.format,
            ready: &mut self.ready,
        };
        self.order.open(&mut sink);
        self.cur = self.free();
        let buffer = &mut self.frames[self.cur];
        buffer.held = Held {
            tag,
            structure: picture.structure,
            top_field_first: match picture.structure {
                // Zero in field pictures by definition, so pairs are taken as top first.
                Structure::Frame => picture.top_field_first,
                Structure::Top => true,
                Structure::Bottom => false,
            },
            repeat_first_field: picture.repeat_first_field,
            progressive: picture.progressive,
        };
    }

    fn finish(&mut self) {
        let mut sink = Emit {
            frames: &self.frames,
            format: self.format,
            ready: &mut self.ready,
        };
        self.order.close(&mut sink);
    }

    fn begin(&mut self, tag: u64, picture: &Picture, coding: PictureCoding) -> State {
        let second = self.order.second(picture.structure);
        if !second {
            self.start(tag, picture);
        }
        let parity = usize::from(picture.structure == Structure::Bottom);
        let frame = picture.structure == Structure::Frame;

        let (forward, backward) = match picture.coding {
            Coding::B => (self.order.older(), self.order.newer()),
            _ => (self.order.newer(), None),
        };
        if !second {
            // A B picture with no forward reference: an open GOP's leading picture, not shown.
            let bidirectional = picture.coding == Coding::B;
            self.order.begin(
                self.cur,
                picture.structure,
                bidirectional,
                bidirectional && forward.is_none(),
            );
        }
        let mut field_source = [[forward; 2], [backward; 2]];
        if !frame && second && picture.coding != Coding::B {
            // Second field of an I/P frame: the other parity is this frame's first
            // field; `motion_vertical_field_select` picks which.
            field_source[0][1 - parity] = Some(self.cur);
            if field_source[0][parity].is_none() {
                // I field + P field entry point: the I field is all there is.
                field_source[0][parity] = Some(self.cur);
            }
        }

        let mb_width = self.frames[self.cur].stride[0] / 16;
        let rows = self.frames[self.cur].height[0] / 16;
        State {
            coding,
            kind: picture.coding,
            structure: picture.structure,
            top_field_first: picture.top_field_first,
            parity,
            matrices: self.matrices,
            scan: if coding.alternate_scan {
                &tables::ALTERNATE
            } else {
                &tables::ZIGZAG
            },
            field_source,
            frame_source: [forward, backward],
            mb_width,
            mb_height: if frame { rows } else { rows / 2 },
        }
    }
}

struct Emit<'a> {
    frames: &'a [Buffer],
    format: Format,
    ready: &'a mut VecDeque<Frame>,
}

impl reorder::Frames for Emit<'_> {
    type Picture = usize;

    fn show(&mut self, at: usize) {
        let buffer = &self.frames[at];
        let planes = std::array::from_fn(|plane| {
            let divisor = if plane == 0 { 1 } else { 2 };
            Plane {
                data: buffer.planes[plane].clone(),
                stride: buffer.stride[plane],
                width: (self.format.width as usize).div_ceil(divisor),
                height: (self.format.height as usize).div_ceil(divisor),
            }
        });
        self.ready.push_back(Frame {
            tag: buffer.held.tag,
            format: self.format,
            planes,
            structure: buffer.held.structure,
            top_field_first: buffer.held.top_field_first,
            repeat_first_field: buffer.held.repeat_first_field,
            progressive: buffer.held.progressive,
        });
    }

    fn show_and_keep(&mut self, at: usize) {
        self.show(at);
    }

    fn discard(&mut self, _at: usize) {}
}

impl Decoder for Mpeg2 {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format above 4:2:0"));
        }
        if format.depth != 8 {
            return Err(Error::Unsupported("a sample deeper than eight bits"));
        }
        if format.width == 0 || format.height == 0 {
            return Err(Error::Damaged("a sequence header with no size"));
        }
        // A change of aspect or frame rate (6.2.2.1) is not a change of decoder:
        // rebuilding would lose the standing reference. Only the sample geometry decides.
        let geometry = |format: &Format| (format.width, format.height, format.depth, format.chroma);
        if geometry(&format) != geometry(&self.format) {
            self.reconfigure(format);
        } else {
            self.format = format;
        }

        let mut coding = None;
        let mut state = None;
        let mut damage = None;
        for (kind, body) in StartCodes::new(data) {
            let bytes = &data[body.clone()];
            match kind {
                0xb3 => self.matrices.read_sequence(bytes),
                0xb5 => match headers::extension_id(bytes) {
                    headers::QUANT_MATRIX_EXTENSION => self.matrices.read_extension(bytes),
                    headers::PICTURE_CODING_EXTENSION => {
                        coding = Some(PictureCoding::read(bytes));
                    }
                    _ => {}
                },
                0x01..=0xaf => {
                    let Some(coding) = coding else {
                        // MPEG-1 has no picture coding extension; do not guess the fields.
                        return Err(Error::Unsupported("a picture with no coding extension"));
                    };
                    let state = state.get_or_insert_with(|| self.begin(tag, picture, coding));
                    // Slice start codes count rows from one.
                    let row = usize::from(kind) - 1;
                    // Runs to the next `00 00 01`, zeroes included, as platform decoders get it.
                    let (end, padding) = headers::slice_end(data, body.end);
                    let bytes = &data[body.start..end];
                    if let Err(error) = self.slice(state, row, bytes, padding) {
                        // A lost slice loses a strip; the next intra picture puts it right.
                        damage.get_or_insert(error);
                    }
                }
                _ => {}
            }
        }

        let Some(state) = state else {
            return Err(Error::Damaged("a picture with no slices"));
        };
        // A first field leaves the frame open for the second of the other parity.
        if self.order.pending() != Some(state.structure) {
            self.finish();
        }
        match damage {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn take(&mut self) -> Option<Frame> {
        self.ready.pop_front()
    }

    fn flush(&mut self) {
        let mut sink = Emit {
            frames: &self.frames,
            format: self.format,
            ready: &mut self.ready,
        };
        self.order.flush(&mut sink);
    }

    fn reset(&mut self) {
        let mut sink = Emit {
            frames: &self.frames,
            format: self.format,
            ready: &mut self.ready,
        };
        self.order.reset(&mut sink);
        self.cur = 0;
        self.matrices = Matrices::new(Order::Raster, Chroma::LikeLuma);
        self.ready.clear();
    }
}
