// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-2, handed to the hardware: headers copied into VA-API's structs, references and reordering
//! ([`mediadecoder::video::mpeg2::reorder`], shared with the software decoder).

use mediadecoder::bits::Reader;
use mediadecoder::es::{Format, Picture, StartCodes};
use mediadecoder::video::mpeg2::headers::{self, Chroma, Matrices, Order, PictureCoding};

use crate::decode::Error;
use crate::decode::frames;
use crate::decode::vaapi::Held;
use crate::decode::vaapi::pool::Pool;
use crate::decode::vaapi::va::{self, VABufferID, VAContextID, VASurfaceID};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct PictureParameters {
    horizontal_size: u16,
    vertical_size: u16,
    forward_reference_picture: VASurfaceID,
    backward_reference_picture: VASurfaceID,
    picture_coding_type: i32,
    f_code: i32,
    picture_coding_extension: u32,
    va_reserved: [u32; 4],
}

/// `VAIQMatrixBufferMPEG2`. Zigzag order, as the bitstream sends them.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct IqMatrix {
    load_intra: i32,
    load_non_intra: i32,
    load_chroma_intra: i32,
    load_chroma_non_intra: i32,
    intra: [u8; 64],
    non_intra: [u8; 64],
    chroma_intra: [u8; 64],
    chroma_non_intra: [u8; 64],
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct SliceParameters {
    slice_data_size: u32,
    slice_data_offset: u32,
    slice_data_flag: u32,
    macroblock_offset: u32,
    slice_horizontal_position: u32,
    slice_vertical_position: u32,
    quantiser_scale_code: i32,
    intra_slice_flag: i32,
    va_reserved: [u32; 4],
}

fn extension(coded: &PictureCoding, is_first_field: bool) -> u32 {
    let bit = |value: bool, at: u32| u32::from(value) << at;
    (coded.intra_dc_precision & 3)
        | ((coded.picture_structure & 3) << 2)
        | bit(coded.top_field_first, 4)
        | bit(coded.frame_pred_frame_dct, 5)
        | bit(coded.concealment_motion_vectors, 6)
        | bit(coded.q_scale_type, 7)
        | bit(coded.intra_vlc_format, 8)
        | bit(coded.alternate_scan, 9)
        | bit(coded.repeat_first_field, 10)
        | bit(coded.progressive_frame, 11)
        | bit(is_first_field, 12)
}

#[derive(Debug)]
pub struct Mpeg2 {
    matrices: Matrices,
    order: frames::Mpeg2Order<usize>,
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
            order: frames::Mpeg2Order::new(),
        }
    }

    pub fn take(&mut self) -> Option<Held> {
        self.order.take()
    }

    pub fn flush(&mut self, pool: &mut Pool) {
        self.order.flush(pool);
    }

    pub fn reset(&mut self, pool: &mut Pool) {
        self.order.reset(pool);
        self.matrices = Matrices::new(Order::AsSent, Chroma::AsSent);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        display: &va::Display,
        context: VAContextID,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        let mut coded = PictureCoding::default();
        let mut started = false;
        let mut buffers = va::Buffers::new(display);
        let mut surface = va::VA_INVALID_SURFACE;

        for (kind, body) in StartCodes::new(data) {
            let bytes = &data[body.clone()];
            match kind {
                0xb3 => self.matrices.read_sequence(bytes),
                0xb5 => match headers::extension_id(bytes) {
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
                    if !started {
                        surface = self.begin(
                            display,
                            context,
                            pool,
                            tag,
                            picture,
                            format,
                            &coded,
                            &mut buffers,
                        )?;
                        started = true;
                    }
                    // スライスはスタートコードから渡し、`macroblock_offset` もスタートコードの先頭から数える。
                    // スタートコードでスライスを探すドライバーがある。
                    let from = body.start.saturating_sub(4);
                    // 末尾の 0 を削ると、長さどおりに読むドライバーは最後のマクロブロックを
                    // 読み切れない。
                    let (end, missing) = headers::slice_end(data, body.end);
                    let padded;
                    let slice = if missing == 0 {
                        &data[from..end]
                    } else {
                        padded = [&data[from..end], &[0; 2][..missing]].concat();
                        &padded[..]
                    };
                    let mut reader = Reader::new(slice);
                    reader.skip(8 * (body.start - from));
                    let quantiser = reader.bits(5) as i32;
                    let intra_slice_flag = reader.bit();
                    if intra_slice_flag == 1 {
                        reader.skip(8);
                        while reader.bit() == 1 {
                            reader.skip(8);
                        }
                    }
                    let parameters = SliceParameters {
                        slice_data_size: slice.len() as u32,
                        slice_data_offset: 0,
                        slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL,
                        macroblock_offset: reader.at() as u32,
                        slice_horizontal_position: 0,
                        slice_vertical_position: u32::from(kind) - 1,
                        quantiser_scale_code: quantiser,
                        intra_slice_flag: intra_slice_flag as i32,
                        va_reserved: [0; 4],
                    };
                    buffers.push(display.parameters(
                        context,
                        va::VA_SLICE_PARAMETER_BUFFER_TYPE,
                        &parameters,
                    )?);
                    buffers.push(display.coded(context, slice)?);
                }
                _ => {}
            }
        }

        if !started {
            return Ok(());
        }
        display.decode(context, surface, &mut buffers)?;
        if self.order.pending() != Some(picture.structure) {
            self.finish(pool);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn begin(
        &mut self,
        display: &va::Display,
        context: VAContextID,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        coded: &PictureCoding,
        buffers: &mut Vec<VABufferID>,
    ) -> Result<VASurfaceID, Error> {
        // The second field of a pair decodes into the same surface and may predict from the first.
        let second = self.order.begin(pool, tag, picture)?;
        let held = self
            .order
            .current()
            .ok_or(Error::Damaged("a slice with no picture"))?;
        let surface = pool.id(held.slot);

        let (forward, backward) = self.order.references(picture.coding);
        let parameters = PictureParameters {
            horizontal_size: format.width as u16,
            vertical_size: format.height as u16,
            forward_reference_picture: forward
                .map_or(va::VA_INVALID_SURFACE, |held| pool.id(held.slot)),
            backward_reference_picture: backward
                .map_or(va::VA_INVALID_SURFACE, |held| pool.id(held.slot)),
            picture_coding_type: coded.kind as i32,
            f_code: ((coded.f_code[0][0] << 12)
                | (coded.f_code[0][1] << 8)
                | (coded.f_code[1][0] << 4)
                | coded.f_code[1][1]) as i32,
            picture_coding_extension: extension(coded, !second),
            va_reserved: [0; 4],
        };
        buffers.push(display.parameters(
            context,
            va::VA_PICTURE_PARAMETER_BUFFER_TYPE,
            &parameters,
        )?);
        let matrices = IqMatrix {
            load_intra: 1,
            load_non_intra: 1,
            load_chroma_intra: 1,
            load_chroma_non_intra: 1,
            intra: self.matrices.intra,
            non_intra: self.matrices.non_intra,
            chroma_intra: self.matrices.chroma_intra,
            chroma_non_intra: self.matrices.chroma_non_intra,
            va_reserved: [0; 4],
        };
        buffers.push(display.parameters(context, va::VA_IQ_MATRIX_BUFFER_TYPE, &matrices)?);
        Ok(surface)
    }

    fn finish(&mut self, pool: &mut Pool) {
        self.order.close(pool);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structs_are_the_size_libva_declares() {
        assert_eq!(size_of::<PictureParameters>(), 40);
        assert_eq!(size_of::<IqMatrix>(), 288);
        assert_eq!(size_of::<SliceParameters>(), 48);
    }

    #[test]
    fn the_picture_coding_extension_is_packed_the_way_the_header_declares() {
        let coded = PictureCoding {
            intra_dc_precision: 2,
            picture_structure: 3,
            top_field_first: true,
            alternate_scan: true,
            progressive_frame: true,
            ..PictureCoding::default()
        };
        let bits = extension(&coded, true);
        assert_eq!(bits & 3, 2);
        assert_eq!((bits >> 2) & 3, 3);
        assert_eq!((bits >> 4) & 1, 1);
        assert_eq!((bits >> 9) & 1, 1);
        assert_eq!((bits >> 11) & 1, 1);
        assert_eq!((bits >> 12) & 1, 1);
    }
}
