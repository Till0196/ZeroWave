// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ドライバーへ渡す `dxva.h` の構造体（`windows` クレートに無いもの）。
//! メンバーが一つずれてもエラーにならず映像が乱れるので、サイズを `const` で確かめる。どれも packed(1)
//! （でないとスライス情報の末尾にパディングが入り、二つ目以降がずれる）。

/// 参照リストの一項目。`Index7Bits` はプールのスロット番号。`0xff` は使わない項目。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PicEntry(pub u8);

impl PicEntry {
    pub fn new(index: usize, flag: bool) -> Self {
        Self((index as u8 & 0x7f) | (u8::from(flag) << 7))
    }

    pub const NONE: Self = Self(0xff);
}

impl Default for PicEntry {
    fn default() -> Self {
        Self::NONE
    }
}

pub fn bit(value: bool, at: u32) -> u32 {
    u32::from(value) << at
}

// H.264

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct PicParamsH264 {
    pub width_in_mbs_minus1: u16,
    pub height_in_mbs_minus1: u16,
    pub current: PicEntry,
    pub num_ref_frames: u8,
    pub bit_fields: u16,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    /// `Reserved16Bits`。予約だが 3 でなければならない（0 だとフレームが書かれないドライバーがある）。
    pub reserved16: u16,
    pub status_report: u32,
    pub reference_list: [PicEntry; 16],
    pub current_order: [i32; 2],
    pub order_list: [[i32; 2]; 16],
    pub pic_init_qs_minus26: i8,
    pub chroma_qp_index_offset: i8,
    pub second_chroma_qp_index_offset: i8,
    /// 後ろのメンバーを埋めたことを示すフラグ。0 だとドライバーがここで読むのをやめる。
    pub continuation: u8,
    pub pic_init_qp_minus26: i8,
    pub num_ref_idx_l0_active_minus1: u8,
    pub num_ref_idx_l1_active_minus1: u8,
    pub reserved8_a: u8,
    pub frame_num_list: [u16; 16],
    /// 参照フレームごとに 2 ビット：偶数ビットがトップ、奇数ビットがボトムフィールド。
    pub used_for_reference: u32,
    pub non_existing: u16,
    pub frame_num: u16,
    pub log2_max_frame_num_minus4: u8,
    pub pic_order_cnt_type: u8,
    pub log2_max_pic_order_cnt_lsb_minus4: u8,
    pub delta_pic_order_always_zero_flag: u8,
    pub direct_8x8_inference_flag: u8,
    pub entropy_coding_mode_flag: u8,
    pub pic_order_present_flag: u8,
    pub num_slice_groups_minus1: u8,
    pub slice_group_map_type: u8,
    pub deblocking_filter_control_present_flag: u8,
    pub redundant_pic_cnt_present_flag: u8,
    pub reserved8_b: u8,
    pub slice_group_change_rate_minus1: u16,
    pub slice_group_map: [u8; 810],
}

impl Default for PicParamsH264 {
    fn default() -> Self {
        // SAFETY: 整数とその配列だけで、0 は埋めないメンバーの値。
        let mut out: Self = unsafe { std::mem::zeroed() };
        out.current = PicEntry::NONE;
        out.reference_list = [PicEntry::NONE; 16];
        out
    }
}

impl PicParamsH264 {
    #[allow(clippy::too_many_arguments)]
    pub fn fields(
        field_pic: bool,
        mbaff: bool,
        chroma_format_idc: u32,
        reference: bool,
        constrained_intra: bool,
        weighted_pred: bool,
        weighted_bipred_idc: u32,
        frame_mbs_only: bool,
        transform_8x8: bool,
        min_luma_bipred_8x8: bool,
        intra: bool,
    ) -> u16 {
        let packed = bit(field_pic, 0)
            | bit(mbaff, 1)
            | ((chroma_format_idc & 3) << 4)
            | bit(reference, 6)
            | bit(constrained_intra, 7)
            | bit(weighted_pred, 8)
            | ((weighted_bipred_idc & 3) << 9)
            | bit(true, 11)
            | bit(frame_mbs_only, 12)
            | bit(transform_8x8, 13)
            | bit(min_luma_bipred_8x8, 14)
            | bit(intra, 15);
        packed as u16
    }
}

/// `DXVA_Qmatrix_H264`。ビットストリームのスキャン順で渡すので、逆スキャンを戻して入れる。
#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct QmatrixH264 {
    pub four: [[u8; 16]; 6],
    pub eight: [[u8; 64]; 2],
}

/// `DXVA_Slice_H264_Short`。D3D12 は H.264・HEVC で Short 形式だけを受け付ける。
#[repr(C, packed(1))]
#[derive(Clone, Copy, Default)]
pub struct SliceShort {
    pub location: u32,
    pub bytes: u32,
    pub bad_chopping: u16,
}

// HEVC

pub const FRAMES: usize = 15;

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct PicParamsHevc {
    pub width_in_min_cbs: u16,
    pub height_in_min_cbs: u16,
    pub format_fields: u16,
    pub current: PicEntry,
    pub max_dec_pic_buffering_minus1: u8,
    pub log2_min_luma_cb_minus3: u8,
    pub log2_diff_max_min_luma_cb: u8,
    pub log2_min_tb_minus2: u8,
    pub log2_diff_max_min_tb: u8,
    pub max_transform_hierarchy_depth_inter: u8,
    pub max_transform_hierarchy_depth_intra: u8,
    pub num_short_term_ref_pic_sets: u8,
    pub num_long_term_ref_pics_sps: u8,
    pub num_ref_idx_l0_default_active_minus1: u8,
    pub num_ref_idx_l1_default_active_minus1: u8,
    pub init_qp_minus26: i8,
    pub num_delta_pocs_of_ref_rps_idx: u8,
    pub bits_for_short_term_rps_in_slice: u16,
    pub reserved16: u16,
    pub tool_flags: u32,
    pub picture_flags: u32,
    pub pps_cb_qp_offset: i8,
    pub pps_cr_qp_offset: i8,
    pub num_tile_columns_minus1: u8,
    pub num_tile_rows_minus1: u8,
    pub column_width_minus1: [u16; 19],
    pub row_height_minus1: [u16; 21],
    pub diff_cu_qp_delta_depth: u8,
    pub pps_beta_offset_div2: i8,
    pub pps_tc_offset_div2: i8,
    pub log2_parallel_merge_level_minus2: u8,
    pub current_order: i32,
    pub reference_list: [PicEntry; FRAMES],
    pub reserved8: u8,
    pub order_list: [i32; FRAMES],
    pub before: [u8; 8],
    pub after: [u8; 8],
    pub long_term: [u8; 8],
    pub reserved16_b: u16,
    pub reserved16_c: u16,
    pub status_report: u32,
}

impl Default for PicParamsHevc {
    fn default() -> Self {
        // SAFETY: 上と同じく整数とその配列だけ。
        let mut out: Self = unsafe { std::mem::zeroed() };
        out.current = PicEntry::NONE;
        out.reference_list = [PicEntry::NONE; FRAMES];
        out.before = [0xff; 8];
        out.after = [0xff; 8];
        out.long_term = [0xff; 8];
        out
    }
}

impl PicParamsHevc {
    pub fn format_fields(
        chroma_format_idc: u32,
        depth_luma_minus8: u32,
        depth_chroma_minus8: u32,
        log2_max_poc_lsb_minus4: u32,
        no_reordering: bool,
    ) -> u16 {
        ((chroma_format_idc & 3)
            | ((depth_luma_minus8 & 7) << 3)
            | ((depth_chroma_minus8 & 7) << 6)
            | ((log2_max_poc_lsb_minus4 & 15) << 9)
            | bit(no_reordering, 13)) as u16
    }
}

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct QmatrixHevc {
    pub four: [[u8; 16]; 6],
    pub eight: [[u8; 64]; 6],
    pub sixteen: [[u8; 64]; 6],
    pub thirty_two: [[u8; 64]; 2],
    pub dc_sixteen: [u8; 6],
    pub dc_thirty_two: [u8; 2],
}

// MPEG-2

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct QmatrixData {
    pub new_matrix: [u8; 4],
    pub matrix: [[u16; 64]; 4],
}

impl Default for QmatrixData {
    fn default() -> Self {
        Self {
            new_matrix: [0; 4],
            matrix: [[0; 64]; 4],
        }
    }
}

#[repr(C, packed(1))]
#[derive(Clone, Copy, Default)]
pub struct SliceInfo {
    pub horizontal: u16,
    pub vertical: u16,
    pub bits_in_buffer: u32,
    pub location: u32,
    pub start_code_bit_offset: u8,
    pub reserved: u8,
    /// 最初のマクロブロックの位置。スライスのスタートコードの先頭からのビット数。
    pub mb_bit_offset: u16,
    pub macroblocks: u16,
    pub quantiser_scale_code: u16,
    pub bad_chopping: u16,
}

// スキャン順

/// テーブル 8-13：4x4 のジグザグスキャン（送られる順のラスター位置）。
pub const ZIGZAG_4X4: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// テーブル 8-14：8x8 のジグザグスキャン（MPEG-2 と同じ）。
pub const ZIGZAG_8X8: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// 6.5.3：正方ブロックの右上がり対角スキャン。
pub fn diagonal<const N: usize>(side: usize) -> [usize; N] {
    let mut out = [0usize; N];
    let (mut x, mut y, mut at) = (0usize, 0usize, 0usize);
    loop {
        loop {
            if x < side && y < side {
                out[at] = y * side + x;
                at += 1;
            }
            if y == 0 {
                break;
            }
            y -= 1;
            x += 1;
        }
        y = x + 1;
        x = 0;
        if at >= side * side {
            return out;
        }
    }
}

const _: () = assert!(size_of::<PicParamsH264>() == 1040);
const _: () = assert!(size_of::<QmatrixH264>() == 224);
const _: () = assert!(size_of::<SliceShort>() == 10);
const _: () = assert!(size_of::<PicParamsHevc>() == 232);
const _: () = assert!(size_of::<QmatrixHevc>() == 1000);
const _: () = assert!(size_of::<QmatrixData>() == 516);
const _: () = assert!(size_of::<SliceInfo>() == 22);

const _: () =
    assert!(size_of::<windows::Win32::Media::MediaFoundation::DXVA_PictureParameters>() == 44);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structs_are_the_size_the_header_declares() {
        assert_eq!(size_of::<PicParamsH264>(), 1040);
        assert_eq!(size_of::<QmatrixH264>(), 224);
        assert_eq!(size_of::<SliceShort>(), 10);
        assert_eq!(size_of::<PicParamsHevc>(), 232);
        assert_eq!(size_of::<QmatrixHevc>(), 1000);
        assert_eq!(size_of::<QmatrixData>(), 516);
        assert_eq!(size_of::<SliceInfo>(), 22);
    }

    #[test]
    fn the_diagonal_scan_walks_up_and_to_the_right() {
        let four: [usize; 16] = diagonal(4);
        assert_eq!(&four[..6], &[0, 4, 1, 8, 5, 2]);
        assert_eq!(four[15], 15);
        let eight: [usize; 64] = diagonal(8);
        assert_eq!(&eight[..4], &[0, 8, 1, 16]);
        assert_eq!(eight[63], 63);
        let mut seen = eight;
        seen.sort_unstable();
        assert!(seen.iter().enumerate().all(|(at, found)| at == *found));
    }
}
