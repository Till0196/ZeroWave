// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! スライス層より上で、デコーダーには必要でプレイヤーには不要なもの（量子化マトリクスと
//! ピクチャー符号化拡張）。
//! マトリクスの順は API で違う（VA-API・DXVA・V4L2 はスキャン順、NVDEC とソフトウェアはラスター順）。
//! 間違えても映像は出て PSNR が二十数 dB 落ちるだけなので、[`Order`] は明示で受け取る。

use crate::bits::Reader;

use super::tables;

/// Figure 7-2. `ZIGZAG[at]` is the raster place of scan place `at`.
pub const ZIGZAG: [u8; 64] = tables::ZIGZAG;

/// Table 7-3, raster order.
pub const DEFAULT_INTRA: [u8; 64] = tables::DEFAULT_INTRA;

/// Table 7-4 (flat, so either order).
pub const DEFAULT_NON_INTRA: [u8; 64] = tables::DEFAULT_NON_INTRA;

pub const SEQUENCE_EXTENSION: u32 = 1;
pub const QUANT_MATRIX_EXTENSION: u32 = 3;
pub const PICTURE_CODING_EXTENSION: u32 = 8;

/// The extension id of a `0xb5` body (its first four bits).
pub fn extension_id(bytes: &[u8]) -> u32 {
    u32::from(bytes.first().copied().unwrap_or(0) >> 4)
}

pub fn scanned(raster: &[u8; 64]) -> [u8; 64] {
    std::array::from_fn(|at| raster[ZIGZAG[at] as usize])
}

pub fn raster(scanned: &[u8; 64]) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (at, &value) in scanned.iter().enumerate() {
        out[ZIGZAG[at] as usize] = value;
    }
    out
}

/// Which order the four matrices of a [`Matrices`] are kept in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Zigzag, as sent. VA-API, DXVA and V4L2.
    AsSent,
    /// NVDEC and the software decoder.
    Raster,
}

/// Whether the chroma matrices are kept apart from the luma ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// As sent, for APIs that take four matrices.
    AsSent,
    /// The luma matrix of the same kind (4:2:0 chroma).
    LikeLuma,
}

/// The four quantiser matrices in force until a sequence header or matrix extension.
#[derive(Debug, Clone, Copy)]
pub struct Matrices {
    pub intra: [u8; 64],
    pub non_intra: [u8; 64],
    pub chroma_intra: [u8; 64],
    pub chroma_non_intra: [u8; 64],
    order: Order,
    chroma: Chroma,
}

impl Matrices {
    pub fn new(order: Order, chroma: Chroma) -> Self {
        let intra = default_intra(order);
        Self {
            intra,
            non_intra: DEFAULT_NON_INTRA,
            chroma_intra: intra,
            chroma_non_intra: DEFAULT_NON_INTRA,
            order,
            chroma,
        }
    }

    pub fn read_sequence(&mut self, bytes: &[u8]) {
        let mut reader = Reader::new(bytes);
        reader.skip(12 + 12 + 4 + 4 + 18 + 1 + 10 + 1);
        let order = self.order;
        // 6.3.11: chroma follows luma. A header without a matrix restores the defaults.
        self.intra = if reader.bit() == 1 {
            read_matrix(&mut reader, order)
        } else {
            default_intra(order)
        };
        self.chroma_intra = self.intra;
        self.non_intra = if reader.bit() == 1 {
            read_matrix(&mut reader, order)
        } else {
            DEFAULT_NON_INTRA
        };
        self.chroma_non_intra = self.non_intra;
    }

    pub fn read_extension(&mut self, bytes: &[u8]) {
        let mut reader = Reader::new(bytes);
        reader.skip(4); // extension_start_code_identifier
        let order = self.order;
        for at in 0..4 {
            if reader.bit() != 1 {
                continue;
            }
            let matrix = read_matrix(&mut reader, order);
            match (at, self.chroma) {
                (0, Chroma::LikeLuma) => {
                    self.intra = matrix;
                    self.chroma_intra = matrix;
                }
                (0, Chroma::AsSent) => self.intra = matrix,
                (1, Chroma::LikeLuma) => {
                    self.non_intra = matrix;
                    self.chroma_non_intra = matrix;
                }
                (1, Chroma::AsSent) => self.non_intra = matrix,
                // Read to step over the bits, then dropped.
                (_, Chroma::LikeLuma) => {}
                (2, Chroma::AsSent) => self.chroma_intra = matrix,
                (_, Chroma::AsSent) => self.chroma_non_intra = matrix,
            }
        }
    }
}

fn default_intra(order: Order) -> [u8; 64] {
    match order {
        Order::AsSent => scanned(&DEFAULT_INTRA),
        Order::Raster => DEFAULT_INTRA,
    }
}

fn read_matrix(reader: &mut Reader<'_>, order: Order) -> [u8; 64] {
    let sent: [u8; 64] = std::array::from_fn(|_| reader.bits(8) as u8);
    match order {
        Order::AsSent => sent,
        Order::Raster => raster(&sent),
    }
}

/// What only the V4L2 sequence control wants from the sequence extension.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sequence {
    pub profile_and_level: u16,
    pub chroma_format: u8,
    pub progressive: bool,
}

impl Sequence {
    pub fn read(bytes: &[u8]) -> Self {
        let mut reader = Reader::new(bytes);
        reader.skip(4); // extension_start_code_identifier
        let profile_and_level = reader.bits(8) as u16;
        let progressive = reader.bit() == 1;
        let chroma_format = reader.bits(2) as u8;
        Self {
            profile_and_level,
            chroma_format,
            progressive,
        }
    }
}

/// Picture header and picture coding extension fields a decoder needs.
#[derive(Debug, Clone, Copy, Default)]
pub struct PictureCoding {
    /// 1 = I, 2 = P, 3 = B; 0 if no picture header was read.
    pub kind: u32,
    /// By direction, then axis.
    pub f_code: [[u32; 2]; 2],
    /// 0..=3 for 8..=11 bits.
    pub intra_dc_precision: u32,
    /// 1 top field, 2 bottom field, 3 frame.
    pub picture_structure: u32,
    pub top_field_first: bool,
    pub frame_pred_frame_dct: bool,
    pub concealment_motion_vectors: bool,
    pub q_scale_type: bool,
    pub intra_vlc_format: bool,
    pub alternate_scan: bool,
    pub repeat_first_field: bool,
    /// Only DXVA wants it.
    pub chroma_420_type: bool,
    pub progressive_frame: bool,
}

impl PictureCoding {
    pub fn read(bytes: &[u8]) -> Self {
        let mut coding = Self::default();
        coding.read_extension(bytes);
        coding
    }

    pub fn read_extension(&mut self, bytes: &[u8]) {
        let mut reader = Reader::new(bytes);
        reader.skip(4); // extension_start_code_identifier
        for direction in 0..2 {
            for axis in 0..2 {
                self.f_code[direction][axis] = reader.bits(4);
            }
        }
        self.intra_dc_precision = reader.bits(2);
        self.picture_structure = reader.bits(2);
        self.top_field_first = reader.bit() == 1;
        self.frame_pred_frame_dct = reader.bit() == 1;
        self.concealment_motion_vectors = reader.bit() == 1;
        self.q_scale_type = reader.bit() == 1;
        self.intra_vlc_format = reader.bit() == 1;
        self.alternate_scan = reader.bit() == 1;
        self.repeat_first_field = reader.bit() == 1;
        self.chroma_420_type = reader.bit() == 1;
        self.progressive_frame = reader.bit() == 1;
    }
}

/// The coding type; the f-codes here are overwritten by the extension.
pub fn read_picture(bytes: &[u8]) -> u32 {
    let mut reader = Reader::new(bytes);
    reader.skip(10); // temporal_reference
    reader.bits(3)
}

/// Where a slice really ends: before the next `00 00 01`, not its leading zeroes; returns the
/// end and how many zeroes to append. MPEG-2 slices have no stop bit, so the zeroes may be
/// its own: a driver keeping to the length drops the last macroblock. The last slice gets two.
pub fn slice_end(data: &[u8], end: usize) -> (usize, usize) {
    let mut at = end;
    while at < data.len() && data[at] == 0 {
        if data.get(at..at + 3) == Some(&[0, 0, 1][..]) {
            return (at, 0);
        }
        at += 1;
    }
    if at == data.len() { (at, 2) } else { (end, 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slice_keeps_the_zeroes_before_the_next_start_code() {
        let data = [0xaa, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02];
        let (_, body) = crate::es::StartCodes::new(&[0, 0, 1, 1, 0xaa, 0, 0, 0, 0, 1, 2])
            .next()
            .unwrap();
        assert_eq!(body.end, 5);
        assert_eq!(slice_end(&data, 1), (3, 0));
        let plain = [0xaa, 0x00, 0x00, 0x01, 0x02];
        assert_eq!(slice_end(&plain, 1), (1, 0));
        assert_eq!(slice_end(&[0xaa], 1), (1, 2));
    }

    #[test]
    fn the_matrices_are_written_in_the_scan_they_are_sent_in() {
        let matrices = Matrices::new(Order::AsSent, Chroma::AsSent);
        assert_eq!(&matrices.intra[..4], &[8, 16, 16, 19]);
        assert_eq!(matrices.intra[63], 83);
    }

    #[test]
    fn the_scan_and_the_raster_are_inverses() {
        let sent: [u8; 64] = std::array::from_fn(|at| at as u8);
        assert_eq!(scanned(&raster(&sent)), sent);
        assert_eq!(raster(&scanned(&sent)), sent);
    }

    #[test]
    fn the_default_matrix_in_the_scan_reads_down_the_diagonal() {
        let matrices = Matrices::new(Order::AsSent, Chroma::AsSent);
        assert_eq!(&matrices.intra[..4], &[8, 16, 16, 19]);
        assert_eq!(matrices.intra[63], 83);
        // NVDEC と VA-API の違い。ここを間違えると PSNR が二十数 dB 落ちる。
        assert_eq!(raster(&matrices.intra), DEFAULT_INTRA);
        assert_eq!(
            Matrices::new(Order::Raster, Chroma::AsSent).intra,
            DEFAULT_INTRA
        );
    }

    #[test]
    fn a_downloaded_matrix_is_read_in_the_order_it_is_kept() {
        // identifier, load_intra, 64 weights, three unset flags.
        let mut bits: Vec<u32> = Vec::new();
        let mut write = |value: u32, count: u32| {
            for shift in (0..count).rev() {
                bits.push((value >> shift) & 1);
            }
        };
        write(QUANT_MATRIX_EXTENSION, 4);
        write(1, 1);
        for at in 0..64u32 {
            write(at, 8);
        }
        write(0, 3);
        let bytes: Vec<u8> = bits
            .chunks(8)
            .map(|chunk| chunk.iter().fold(0u8, |byte, &bit| (byte << 1) | bit as u8))
            .collect();
        let sent: [u8; 64] = std::array::from_fn(|at| at as u8);

        let mut as_sent = Matrices::new(Order::AsSent, Chroma::AsSent);
        as_sent.read_extension(&bytes);
        assert_eq!(as_sent.intra, sent);

        let mut in_raster = Matrices::new(Order::Raster, Chroma::AsSent);
        in_raster.read_extension(&bytes);
        assert_eq!(in_raster.intra, raster(&sent));
        assert_eq!(in_raster.chroma_intra, DEFAULT_INTRA);

        let mut like_luma = Matrices::new(Order::Raster, Chroma::LikeLuma);
        like_luma.read_extension(&bytes);
        assert_eq!(like_luma.chroma_intra, raster(&sent));
    }
}
