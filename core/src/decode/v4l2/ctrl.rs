// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Stateless decoding controls from `v4l2-controls.h`. References are named by output
//! buffer timestamp (ns). Pre-6.0 HEVC controls are refused by name ([`Kind::legacy`]).

#![allow(dead_code)]

const V4L2_CTRL_CLASS_CODEC: u32 = 0x0099_0000;
const V4L2_CTRL_CLASS_CODEC_STATELESS: u32 = 0x00a4_0000;
const CODEC_BASE: u32 = V4L2_CTRL_CLASS_CODEC | 0x900;
const STATELESS_BASE: u32 = V4L2_CTRL_CLASS_CODEC_STATELESS | 0x900;

pub const V4L2_CID_STATELESS_H264_DECODE_MODE: u32 = STATELESS_BASE;
pub const V4L2_CID_STATELESS_H264_START_CODE: u32 = STATELESS_BASE + 1;
pub const V4L2_CID_STATELESS_H264_SPS: u32 = STATELESS_BASE + 2;
pub const V4L2_CID_STATELESS_H264_PPS: u32 = STATELESS_BASE + 3;
pub const V4L2_CID_STATELESS_H264_SCALING_MATRIX: u32 = STATELESS_BASE + 4;
pub const V4L2_CID_STATELESS_H264_PRED_WEIGHTS: u32 = STATELESS_BASE + 5;
pub const V4L2_CID_STATELESS_H264_SLICE_PARAMS: u32 = STATELESS_BASE + 6;
pub const V4L2_CID_STATELESS_H264_DECODE_PARAMS: u32 = STATELESS_BASE + 7;

pub const V4L2_CID_STATELESS_MPEG2_SEQUENCE: u32 = STATELESS_BASE + 220;
pub const V4L2_CID_STATELESS_MPEG2_PICTURE: u32 = STATELESS_BASE + 221;
pub const V4L2_CID_STATELESS_MPEG2_QUANTISATION: u32 = STATELESS_BASE + 222;

pub const V4L2_CID_STATELESS_HEVC_SPS: u32 = STATELESS_BASE + 400;
pub const V4L2_CID_STATELESS_HEVC_PPS: u32 = STATELESS_BASE + 401;
pub const V4L2_CID_STATELESS_HEVC_SLICE_PARAMS: u32 = STATELESS_BASE + 402;
pub const V4L2_CID_STATELESS_HEVC_SCALING_MATRIX: u32 = STATELESS_BASE + 403;
pub const V4L2_CID_STATELESS_HEVC_DECODE_PARAMS: u32 = STATELESS_BASE + 404;
pub const V4L2_CID_STATELESS_HEVC_DECODE_MODE: u32 = STATELESS_BASE + 405;
pub const V4L2_CID_STATELESS_HEVC_START_CODE: u32 = STATELESS_BASE + 406;
/// `u32` dynamic array: each substream's length in bytes (`entry_point_offset_minus1 + 1`).
pub const V4L2_CID_STATELESS_HEVC_ENTRY_POINT_OFFSETS: u32 = STATELESS_BASE + 407;

pub const V4L2_CID_MPEG_VIDEO_HEVC_SPS: u32 = CODEC_BASE + 1008;
pub const V4L2_CID_MPEG_VIDEO_HEVC_PPS: u32 = CODEC_BASE + 1009;
pub const V4L2_CID_MPEG_VIDEO_HEVC_SLICE_PARAMS: u32 = CODEC_BASE + 1010;
pub const V4L2_CID_MPEG_VIDEO_HEVC_SCALING_MATRIX: u32 = CODEC_BASE + 1011;
pub const V4L2_CID_MPEG_VIDEO_HEVC_DECODE_PARAMS: u32 = CODEC_BASE + 1012;
pub const V4L2_CID_MPEG_VIDEO_HEVC_DECODE_MODE: u32 = CODEC_BASE + 1015;
pub const V4L2_CID_MPEG_VIDEO_HEVC_START_CODE: u32 = CODEC_BASE + 1016;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Naming {
    Stateless,
    MpegVideo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mpeg2Sequence,
    Mpeg2Picture,
    Mpeg2Quantisation,
    H264Sps,
    H264Pps,
    H264ScalingMatrix,
    H264PredWeights,
    H264SliceParams,
    H264DecodeParams,
    H264DecodeMode,
    H264StartCode,
    HevcSps,
    HevcPps,
    HevcSliceParams,
    HevcScalingMatrix,
    HevcDecodeParams,
    HevcDecodeMode,
    HevcStartCode,
    HevcEntryPointOffsets,
}

impl Kind {
    pub fn id(self) -> u32 {
        match self {
            Self::Mpeg2Sequence => V4L2_CID_STATELESS_MPEG2_SEQUENCE,
            Self::Mpeg2Picture => V4L2_CID_STATELESS_MPEG2_PICTURE,
            Self::Mpeg2Quantisation => V4L2_CID_STATELESS_MPEG2_QUANTISATION,
            Self::H264Sps => V4L2_CID_STATELESS_H264_SPS,
            Self::H264Pps => V4L2_CID_STATELESS_H264_PPS,
            Self::H264ScalingMatrix => V4L2_CID_STATELESS_H264_SCALING_MATRIX,
            Self::H264PredWeights => V4L2_CID_STATELESS_H264_PRED_WEIGHTS,
            Self::H264SliceParams => V4L2_CID_STATELESS_H264_SLICE_PARAMS,
            Self::H264DecodeParams => V4L2_CID_STATELESS_H264_DECODE_PARAMS,
            Self::H264DecodeMode => V4L2_CID_STATELESS_H264_DECODE_MODE,
            Self::H264StartCode => V4L2_CID_STATELESS_H264_START_CODE,
            Self::HevcSps => V4L2_CID_STATELESS_HEVC_SPS,
            Self::HevcPps => V4L2_CID_STATELESS_HEVC_PPS,
            Self::HevcSliceParams => V4L2_CID_STATELESS_HEVC_SLICE_PARAMS,
            Self::HevcScalingMatrix => V4L2_CID_STATELESS_HEVC_SCALING_MATRIX,
            Self::HevcDecodeParams => V4L2_CID_STATELESS_HEVC_DECODE_PARAMS,
            Self::HevcDecodeMode => V4L2_CID_STATELESS_HEVC_DECODE_MODE,
            Self::HevcStartCode => V4L2_CID_STATELESS_HEVC_START_CODE,
            Self::HevcEntryPointOffsets => V4L2_CID_STATELESS_HEVC_ENTRY_POINT_OFFSETS,
        }
    }

    pub fn legacy(self) -> Option<u32> {
        Some(match self {
            Self::HevcSps => V4L2_CID_MPEG_VIDEO_HEVC_SPS,
            Self::HevcPps => V4L2_CID_MPEG_VIDEO_HEVC_PPS,
            Self::HevcSliceParams => V4L2_CID_MPEG_VIDEO_HEVC_SLICE_PARAMS,
            Self::HevcScalingMatrix => V4L2_CID_MPEG_VIDEO_HEVC_SCALING_MATRIX,
            Self::HevcDecodeParams => V4L2_CID_MPEG_VIDEO_HEVC_DECODE_PARAMS,
            Self::HevcDecodeMode => V4L2_CID_MPEG_VIDEO_HEVC_DECODE_MODE,
            Self::HevcStartCode => V4L2_CID_MPEG_VIDEO_HEVC_START_CODE,
            _ => return None,
        })
    }

    pub fn id_for(self, naming: Naming) -> u32 {
        match naming {
            Naming::Stateless => self.id(),
            Naming::MpegVideo => self.legacy().unwrap_or_else(|| self.id()),
        }
    }

    /// Bytes this build writes; held against the kernel's `elem_size`.
    pub fn size(self) -> usize {
        match self {
            Self::Mpeg2Sequence => size_of::<Mpeg2Sequence>(),
            Self::Mpeg2Picture => size_of::<Mpeg2Picture>(),
            Self::Mpeg2Quantisation => size_of::<Mpeg2Quantisation>(),
            Self::H264Sps => size_of::<H264Sps>(),
            Self::H264Pps => size_of::<H264Pps>(),
            Self::H264ScalingMatrix => size_of::<H264ScalingMatrix>(),
            Self::H264PredWeights => size_of::<H264PredWeights>(),
            Self::H264SliceParams => size_of::<H264SliceParams>(),
            Self::H264DecodeParams => size_of::<H264DecodeParams>(),
            Self::HevcSps => size_of::<HevcSps>(),
            Self::HevcPps => size_of::<HevcPps>(),
            Self::HevcSliceParams => size_of::<HevcSliceParams>(),
            Self::HevcScalingMatrix => size_of::<HevcScalingMatrix>(),
            Self::HevcDecodeParams => size_of::<HevcDecodeParams>(),
            Self::H264DecodeMode | Self::H264StartCode => 4,
            Self::HevcDecodeMode | Self::HevcStartCode => 4,
            // One element; the control is an array of them.
            Self::HevcEntryPointOffsets => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mpeg2Sequence => "MPEG2_SEQUENCE",
            Self::Mpeg2Picture => "MPEG2_PICTURE",
            Self::Mpeg2Quantisation => "MPEG2_QUANTISATION",
            Self::H264Sps => "H264_SPS",
            Self::H264Pps => "H264_PPS",
            Self::H264ScalingMatrix => "H264_SCALING_MATRIX",
            Self::H264PredWeights => "H264_PRED_WEIGHTS",
            Self::H264SliceParams => "H264_SLICE_PARAMS",
            Self::H264DecodeParams => "H264_DECODE_PARAMS",
            Self::H264DecodeMode => "H264_DECODE_MODE",
            Self::H264StartCode => "H264_START_CODE",
            Self::HevcSps => "HEVC_SPS",
            Self::HevcPps => "HEVC_PPS",
            Self::HevcSliceParams => "HEVC_SLICE_PARAMS",
            Self::HevcScalingMatrix => "HEVC_SCALING_MATRIX",
            Self::HevcDecodeParams => "HEVC_DECODE_PARAMS",
            Self::HevcDecodeMode => "HEVC_DECODE_MODE",
            Self::HevcStartCode => "HEVC_START_CODE",
            Self::HevcEntryPointOffsets => "HEVC_ENTRY_POINT_OFFSETS",
        }
    }
}

/// A control for `VIDIOC_S_EXT_CTRLS`; the payload is copied to outlive its source.
#[derive(Debug, Clone)]
pub struct Ctl {
    pub kind: Kind,
    pub bytes: Vec<u8>,
    pub value: Option<i32>,
}

/// # Safety
/// `repr(C)` で整数だけが隙間なく並ぶ（パディングが無い）構造体にだけ付ける。
pub unsafe trait Plain: Copy {}

macro_rules! plain {
    ($($name:ident),* $(,)?) => {
        // SAFETY: どれも uapi と同じレイアウトの `repr(C)` で、整数だけが隙間なく並ぶ。
        $(unsafe impl Plain for $name {})*
    };
}

plain!(
    Mpeg2Sequence,
    Mpeg2Picture,
    Mpeg2Quantisation,
    H264Sps,
    H264Pps,
    H264ScalingMatrix,
    H264PredWeights,
    H264SliceParams,
    H264DecodeParams,
    HevcSps,
    HevcPps,
    HevcSliceParams,
    HevcDecodeParams,
    HevcScalingMatrix,
);

/// すべて 0 の値。`Plain` は整数だけなので 0 で正しい。
fn zeroed<T: Plain>() -> T {
    // SAFETY: `Plain` の前提どおり整数だけが並び、すべて 0 が正しい値。
    unsafe { std::mem::zeroed() }
}

impl Ctl {
    pub fn of<T: Plain>(kind: Kind, payload: &T) -> Self {
        debug_assert_eq!(size_of::<T>(), kind.size());
        // SAFETY: `Plain` の前提どおりパディングも不正なビットも無い。
        let bytes = unsafe {
            std::slice::from_raw_parts((payload as *const T).cast::<u8>(), size_of::<T>()).to_vec()
        };
        Self {
            kind,
            bytes,
            value: None,
        }
    }

    pub fn words(kind: Kind, values: &[u32]) -> Self {
        Self {
            kind,
            bytes: values
                .iter()
                .flat_map(|value| value.to_ne_bytes())
                .collect(),
            value: None,
        }
    }

    pub fn integer(kind: Kind, value: i32) -> Self {
        Self {
            kind,
            bytes: Vec::new(),
            value: Some(value),
        }
    }
}

pub const V4L2_MPEG2_SEQ_FLAG_PROGRESSIVE: u8 = 0x01;

pub const V4L2_MPEG2_PIC_CODING_TYPE_I: u8 = 1;
pub const V4L2_MPEG2_PIC_CODING_TYPE_P: u8 = 2;
pub const V4L2_MPEG2_PIC_CODING_TYPE_B: u8 = 3;

pub const V4L2_MPEG2_PIC_TOP_FIELD: u8 = 0x1;
pub const V4L2_MPEG2_PIC_BOTTOM_FIELD: u8 = 0x2;
pub const V4L2_MPEG2_PIC_FRAME: u8 = 0x3;

pub const V4L2_MPEG2_PIC_FLAG_TOP_FIELD_FIRST: u32 = 0x0001;
pub const V4L2_MPEG2_PIC_FLAG_FRAME_PRED_DCT: u32 = 0x0002;
pub const V4L2_MPEG2_PIC_FLAG_CONCEALMENT_MV: u32 = 0x0004;
pub const V4L2_MPEG2_PIC_FLAG_Q_SCALE_TYPE: u32 = 0x0008;
pub const V4L2_MPEG2_PIC_FLAG_INTRA_VLC: u32 = 0x0010;
pub const V4L2_MPEG2_PIC_FLAG_ALT_SCAN: u32 = 0x0020;
pub const V4L2_MPEG2_PIC_FLAG_REPEAT_FIRST: u32 = 0x0040;
pub const V4L2_MPEG2_PIC_FLAG_PROGRESSIVE: u32 = 0x0080;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Mpeg2Sequence {
    pub horizontal_size: u16,
    pub vertical_size: u16,
    pub vbv_buffer_size: u32,
    pub profile_and_level_indication: u16,
    pub chroma_format: u8,
    pub flags: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Mpeg2Picture {
    pub backward_ref_ts: u64,
    pub forward_ref_ts: u64,
    pub flags: u32,
    pub f_code: [[u8; 2]; 2],
    pub picture_coding_type: u8,
    pub picture_structure: u8,
    pub intra_dc_precision: u8,
    pub reserved: [u8; 5],
}

/// Matrices in zigzag scan.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Mpeg2Quantisation {
    pub intra_quantiser_matrix: [u8; 64],
    pub non_intra_quantiser_matrix: [u8; 64],
    pub chroma_intra_quantiser_matrix: [u8; 64],
    pub chroma_non_intra_quantiser_matrix: [u8; 64],
}

pub const V4L2_H264_NUM_DPB_ENTRIES: usize = 16;
pub const V4L2_H264_REF_LIST_LEN: usize = 2 * V4L2_H264_NUM_DPB_ENTRIES;

pub const V4L2_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE: u32 = 0x01;
pub const V4L2_H264_SPS_FLAG_QPPRIME_Y_ZERO_TRANSFORM_BYPASS: u32 = 0x02;
pub const V4L2_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO: u32 = 0x04;
pub const V4L2_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED: u32 = 0x08;
pub const V4L2_H264_SPS_FLAG_FRAME_MBS_ONLY: u32 = 0x10;
pub const V4L2_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD: u32 = 0x20;
pub const V4L2_H264_SPS_FLAG_DIRECT_8X8_INFERENCE: u32 = 0x40;

pub const V4L2_H264_PPS_FLAG_ENTROPY_CODING_MODE: u16 = 0x0001;
pub const V4L2_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT: u16 = 0x0002;
pub const V4L2_H264_PPS_FLAG_WEIGHTED_PRED: u16 = 0x0004;
pub const V4L2_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT: u16 = 0x0008;
pub const V4L2_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED: u16 = 0x0010;
pub const V4L2_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT: u16 = 0x0020;
pub const V4L2_H264_PPS_FLAG_TRANSFORM_8X8_MODE: u16 = 0x0040;
pub const V4L2_H264_PPS_FLAG_SCALING_MATRIX_PRESENT: u16 = 0x0080;

pub const V4L2_H264_TOP_FIELD_REF: u8 = 0x1;
pub const V4L2_H264_BOTTOM_FIELD_REF: u8 = 0x2;
pub const V4L2_H264_FRAME_REF: u8 = 0x3;

pub const V4L2_H264_SLICE_TYPE_P: u8 = 0;
pub const V4L2_H264_SLICE_TYPE_B: u8 = 1;
pub const V4L2_H264_SLICE_TYPE_I: u8 = 2;
pub const V4L2_H264_SLICE_TYPE_SP: u8 = 3;
pub const V4L2_H264_SLICE_TYPE_SI: u8 = 4;

pub const V4L2_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED: u32 = 0x01;
pub const V4L2_H264_SLICE_FLAG_SP_FOR_SWITCH: u32 = 0x02;

pub const V4L2_H264_DPB_ENTRY_FLAG_VALID: u32 = 0x01;
pub const V4L2_H264_DPB_ENTRY_FLAG_ACTIVE: u32 = 0x02;
pub const V4L2_H264_DPB_ENTRY_FLAG_LONG_TERM: u32 = 0x04;
pub const V4L2_H264_DPB_ENTRY_FLAG_FIELD: u32 = 0x08;

pub const V4L2_H264_DECODE_PARAM_FLAG_IDR_PIC: u32 = 0x01;
pub const V4L2_H264_DECODE_PARAM_FLAG_FIELD_PIC: u32 = 0x02;
pub const V4L2_H264_DECODE_PARAM_FLAG_BOTTOM_FIELD: u32 = 0x04;
pub const V4L2_H264_DECODE_PARAM_FLAG_PFRAME: u32 = 0x08;
pub const V4L2_H264_DECODE_PARAM_FLAG_BFRAME: u32 = 0x10;

pub const V4L2_STATELESS_H264_DECODE_MODE_SLICE_BASED: i32 = 0;
pub const V4L2_STATELESS_H264_DECODE_MODE_FRAME_BASED: i32 = 1;
pub const V4L2_STATELESS_H264_START_CODE_NONE: i32 = 0;
pub const V4L2_STATELESS_H264_START_CODE_ANNEX_B: i32 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H264Sps {
    pub profile_idc: u8,
    pub constraint_set_flags: u8,
    pub level_idc: u8,
    pub seq_parameter_set_id: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub log2_max_frame_num_minus4: u8,
    pub pic_order_cnt_type: u8,
    pub log2_max_pic_order_cnt_lsb_minus4: u8,
    pub max_num_ref_frames: u8,
    pub num_ref_frames_in_pic_order_cnt_cycle: u8,
    pub offset_for_ref_frame: [i32; 255],
    pub offset_for_non_ref_pic: i32,
    pub offset_for_top_to_bottom_field: i32,
    pub pic_width_in_mbs_minus1: u16,
    pub pic_height_in_map_units_minus1: u16,
    pub flags: u32,
}

impl Default for H264Sps {
    fn default() -> Self {
        zeroed()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct H264Pps {
    pub pic_parameter_set_id: u8,
    pub seq_parameter_set_id: u8,
    pub num_slice_groups_minus1: u8,
    pub num_ref_idx_l0_default_active_minus1: u8,
    pub num_ref_idx_l1_default_active_minus1: u8,
    pub weighted_bipred_idc: u8,
    pub pic_init_qp_minus26: i8,
    pub pic_init_qs_minus26: i8,
    pub chroma_qp_index_offset: i8,
    pub second_chroma_qp_index_offset: i8,
    pub flags: u16,
}

/// In the block's raster ([`mediadecoder::video::h264::params::weights`]).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H264ScalingMatrix {
    pub scaling_list_4x4: [[u8; 16]; 6],
    pub scaling_list_8x8: [[u8; 64]; 6],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H264WeightFactors {
    pub luma_weight: [i16; 32],
    pub luma_offset: [i16; 32],
    pub chroma_weight: [[i16; 2]; 32],
    pub chroma_offset: [[i16; 2]; 32],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H264PredWeights {
    pub luma_log2_weight_denom: u16,
    pub chroma_log2_weight_denom: u16,
    pub weight_factors: [H264WeightFactors; 2],
}

impl Default for H264PredWeights {
    fn default() -> Self {
        zeroed()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct H264Reference {
    pub fields: u8,
    pub index: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H264SliceParams {
    /// Slice header length in bits, unescaped, from the unit header byte.
    pub header_bit_size: u32,
    pub first_mb_in_slice: u32,
    pub slice_type: u8,
    pub colour_plane_id: u8,
    pub redundant_pic_cnt: u8,
    pub cabac_init_idc: u8,
    pub slice_qp_delta: i8,
    pub slice_qs_delta: i8,
    pub disable_deblocking_filter_idc: u8,
    pub slice_alpha_c0_offset_div2: i8,
    pub slice_beta_offset_div2: i8,
    pub num_ref_idx_l0_active_minus1: u8,
    pub num_ref_idx_l1_active_minus1: u8,
    pub reserved: u8,
    pub ref_pic_list0: [H264Reference; V4L2_H264_REF_LIST_LEN],
    pub ref_pic_list1: [H264Reference; V4L2_H264_REF_LIST_LEN],
    pub flags: u32,
}

impl Default for H264SliceParams {
    fn default() -> Self {
        zeroed()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct H264DpbEntry {
    pub reference_ts: u64,
    pub pic_num: u32,
    pub frame_num: u16,
    pub fields: u8,
    pub reserved: [u8; 5],
    pub top_field_order_cnt: i32,
    pub bottom_field_order_cnt: i32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H264DecodeParams {
    pub dpb: [H264DpbEntry; V4L2_H264_NUM_DPB_ENTRIES],
    pub nal_ref_idc: u16,
    pub frame_num: u16,
    pub top_field_order_cnt: i32,
    pub bottom_field_order_cnt: i32,
    pub idr_pic_id: u16,
    pub pic_order_cnt_lsb: u16,
    pub delta_pic_order_cnt_bottom: i32,
    pub delta_pic_order_cnt0: i32,
    pub delta_pic_order_cnt1: i32,
    pub dec_ref_pic_marking_bit_size: u32,
    pub pic_order_cnt_bit_size: u32,
    pub slice_group_change_cycle: u32,
    pub reserved: u32,
    pub flags: u32,
}

impl Default for H264DecodeParams {
    fn default() -> Self {
        zeroed()
    }
}

pub const V4L2_HEVC_DPB_ENTRIES_NUM_MAX: usize = 16;

pub const V4L2_HEVC_SLICE_TYPE_B: u8 = 0;
pub const V4L2_HEVC_SLICE_TYPE_P: u8 = 1;
pub const V4L2_HEVC_SLICE_TYPE_I: u8 = 2;

pub const V4L2_HEVC_SPS_FLAG_SEPARATE_COLOUR_PLANE: u64 = 1 << 0;
pub const V4L2_HEVC_SPS_FLAG_SCALING_LIST_ENABLED: u64 = 1 << 1;
pub const V4L2_HEVC_SPS_FLAG_AMP_ENABLED: u64 = 1 << 2;
pub const V4L2_HEVC_SPS_FLAG_SAMPLE_ADAPTIVE_OFFSET: u64 = 1 << 3;
pub const V4L2_HEVC_SPS_FLAG_PCM_ENABLED: u64 = 1 << 4;
pub const V4L2_HEVC_SPS_FLAG_PCM_LOOP_FILTER_DISABLED: u64 = 1 << 5;
pub const V4L2_HEVC_SPS_FLAG_LONG_TERM_REF_PICS_PRESENT: u64 = 1 << 6;
pub const V4L2_HEVC_SPS_FLAG_SPS_TEMPORAL_MVP_ENABLED: u64 = 1 << 7;
pub const V4L2_HEVC_SPS_FLAG_STRONG_INTRA_SMOOTHING_ENABLED: u64 = 1 << 8;

pub const V4L2_HEVC_PPS_FLAG_DEPENDENT_SLICE_SEGMENT_ENABLED: u64 = 1 << 0;
pub const V4L2_HEVC_PPS_FLAG_OUTPUT_FLAG_PRESENT: u64 = 1 << 1;
pub const V4L2_HEVC_PPS_FLAG_SIGN_DATA_HIDING_ENABLED: u64 = 1 << 2;
pub const V4L2_HEVC_PPS_FLAG_CABAC_INIT_PRESENT: u64 = 1 << 3;
pub const V4L2_HEVC_PPS_FLAG_CONSTRAINED_INTRA_PRED: u64 = 1 << 4;
pub const V4L2_HEVC_PPS_FLAG_TRANSFORM_SKIP_ENABLED: u64 = 1 << 5;
pub const V4L2_HEVC_PPS_FLAG_CU_QP_DELTA_ENABLED: u64 = 1 << 6;
pub const V4L2_HEVC_PPS_FLAG_PPS_SLICE_CHROMA_QP_OFFSETS_PRESENT: u64 = 1 << 7;
pub const V4L2_HEVC_PPS_FLAG_WEIGHTED_PRED: u64 = 1 << 8;
pub const V4L2_HEVC_PPS_FLAG_WEIGHTED_BIPRED: u64 = 1 << 9;
pub const V4L2_HEVC_PPS_FLAG_TRANSQUANT_BYPASS_ENABLED: u64 = 1 << 10;
pub const V4L2_HEVC_PPS_FLAG_TILES_ENABLED: u64 = 1 << 11;
pub const V4L2_HEVC_PPS_FLAG_ENTROPY_CODING_SYNC_ENABLED: u64 = 1 << 12;
pub const V4L2_HEVC_PPS_FLAG_LOOP_FILTER_ACROSS_TILES_ENABLED: u64 = 1 << 13;
pub const V4L2_HEVC_PPS_FLAG_PPS_LOOP_FILTER_ACROSS_SLICES_ENABLED: u64 = 1 << 14;
pub const V4L2_HEVC_PPS_FLAG_DEBLOCKING_FILTER_OVERRIDE_ENABLED: u64 = 1 << 15;
pub const V4L2_HEVC_PPS_FLAG_PPS_DISABLE_DEBLOCKING_FILTER: u64 = 1 << 16;
pub const V4L2_HEVC_PPS_FLAG_LISTS_MODIFICATION_PRESENT: u64 = 1 << 17;
pub const V4L2_HEVC_PPS_FLAG_SLICE_SEGMENT_HEADER_EXTENSION_PRESENT: u64 = 1 << 18;
pub const V4L2_HEVC_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT: u64 = 1 << 19;
pub const V4L2_HEVC_PPS_FLAG_UNIFORM_SPACING: u64 = 1 << 20;

pub const V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_SAO_LUMA: u64 = 1 << 0;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_SAO_CHROMA: u64 = 1 << 1;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_TEMPORAL_MVP_ENABLED: u64 = 1 << 2;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_MVD_L1_ZERO: u64 = 1 << 3;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_CABAC_INIT: u64 = 1 << 4;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_COLLOCATED_FROM_L0: u64 = 1 << 5;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_USE_INTEGER_MV: u64 = 1 << 6;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_DEBLOCKING_FILTER_DISABLED: u64 = 1 << 7;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_LOOP_FILTER_ACROSS_SLICES_ENABLED: u64 = 1 << 8;
pub const V4L2_HEVC_SLICE_PARAMS_FLAG_DEPENDENT_SLICE_SEGMENT: u64 = 1 << 9;

pub const V4L2_HEVC_DPB_ENTRY_LONG_TERM_REFERENCE: u8 = 0x01;

pub const V4L2_HEVC_DECODE_PARAM_FLAG_IRAP_PIC: u64 = 0x1;
pub const V4L2_HEVC_DECODE_PARAM_FLAG_IDR_PIC: u64 = 0x2;
pub const V4L2_HEVC_DECODE_PARAM_FLAG_NO_OUTPUT_OF_PRIOR: u64 = 0x4;

pub const V4L2_STATELESS_HEVC_DECODE_MODE_SLICE_BASED: i32 = 0;
pub const V4L2_STATELESS_HEVC_DECODE_MODE_FRAME_BASED: i32 = 1;
pub const V4L2_STATELESS_HEVC_START_CODE_NONE: i32 = 0;
pub const V4L2_STATELESS_HEVC_START_CODE_ANNEX_B: i32 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct HevcSps {
    pub video_parameter_set_id: u8,
    pub seq_parameter_set_id: u8,
    pub pic_width_in_luma_samples: u16,
    pub pic_height_in_luma_samples: u16,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub log2_max_pic_order_cnt_lsb_minus4: u8,
    pub sps_max_dec_pic_buffering_minus1: u8,
    pub sps_max_num_reorder_pics: u8,
    pub sps_max_latency_increase_plus1: u8,
    pub log2_min_luma_coding_block_size_minus3: u8,
    pub log2_diff_max_min_luma_coding_block_size: u8,
    pub log2_min_luma_transform_block_size_minus2: u8,
    pub log2_diff_max_min_luma_transform_block_size: u8,
    pub max_transform_hierarchy_depth_inter: u8,
    pub max_transform_hierarchy_depth_intra: u8,
    pub pcm_sample_bit_depth_luma_minus1: u8,
    pub pcm_sample_bit_depth_chroma_minus1: u8,
    pub log2_min_pcm_luma_coding_block_size_minus3: u8,
    pub log2_diff_max_min_pcm_luma_coding_block_size: u8,
    pub num_short_term_ref_pic_sets: u8,
    pub num_long_term_ref_pics_sps: u8,
    pub chroma_format_idc: u8,
    pub sps_max_sub_layers_minus1: u8,
    pub reserved: [u8; 6],
    pub flags: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HevcPps {
    pub pic_parameter_set_id: u8,
    pub num_extra_slice_header_bits: u8,
    pub num_ref_idx_l0_default_active_minus1: u8,
    pub num_ref_idx_l1_default_active_minus1: u8,
    pub init_qp_minus26: i8,
    pub diff_cu_qp_delta_depth: u8,
    pub pps_cb_qp_offset: i8,
    pub pps_cr_qp_offset: i8,
    pub num_tile_columns_minus1: u8,
    pub num_tile_rows_minus1: u8,
    pub column_width_minus1: [u8; 20],
    pub row_height_minus1: [u8; 22],
    pub pps_beta_offset_div2: i8,
    pub pps_tc_offset_div2: i8,
    pub log2_parallel_merge_level_minus2: u8,
    pub reserved: u8,
    pub flags: u64,
}

impl Default for HevcPps {
    fn default() -> Self {
        zeroed()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct HevcDpbEntry {
    pub timestamp: u64,
    pub flags: u8,
    pub field_pic: u8,
    pub reserved: u16,
    pub pic_order_cnt_val: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HevcPredWeightTable {
    pub delta_luma_weight_l0: [i8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub luma_offset_l0: [i8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub delta_chroma_weight_l0: [[i8; 2]; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub chroma_offset_l0: [[i8; 2]; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub delta_luma_weight_l1: [i8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub luma_offset_l1: [i8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub delta_chroma_weight_l1: [[i8; 2]; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub chroma_offset_l1: [[i8; 2]; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub luma_log2_weight_denom: u8,
    pub delta_chroma_log2_weight_denom: i8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HevcSliceParams {
    pub bit_size: u32,
    /// Bytes to the slice data, escapes included.
    pub data_byte_offset: u32,
    pub num_entry_point_offsets: u32,
    pub nal_unit_type: u8,
    pub nuh_temporal_id_plus1: u8,
    pub slice_type: u8,
    pub colour_plane_id: u8,
    pub slice_pic_order_cnt: i32,
    pub num_ref_idx_l0_active_minus1: u8,
    pub num_ref_idx_l1_active_minus1: u8,
    pub collocated_ref_idx: u8,
    pub five_minus_max_num_merge_cand: u8,
    pub slice_qp_delta: i8,
    pub slice_cb_qp_offset: i8,
    pub slice_cr_qp_offset: i8,
    pub slice_act_y_qp_offset: i8,
    pub slice_act_cb_qp_offset: i8,
    pub slice_act_cr_qp_offset: i8,
    pub slice_beta_offset_div2: i8,
    pub slice_tc_offset_div2: i8,
    pub pic_struct: u8,
    pub reserved0: [u8; 3],
    pub slice_segment_addr: u32,
    pub ref_idx_l0: [u8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub ref_idx_l1: [u8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub short_term_ref_pic_set_size: u16,
    pub long_term_ref_pic_set_size: u16,
    pub pred_weight_table: HevcPredWeightTable,
    pub reserved1: [u8; 2],
    pub flags: u64,
}

impl Default for HevcSliceParams {
    fn default() -> Self {
        zeroed()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HevcDecodeParams {
    pub pic_order_cnt_val: i32,
    pub short_term_ref_pic_set_size: u16,
    pub long_term_ref_pic_set_size: u16,
    pub num_active_dpb_entries: u8,
    pub num_poc_st_curr_before: u8,
    pub num_poc_st_curr_after: u8,
    pub num_poc_lt_curr: u8,
    /// Indices into `dpb`, despite the `poc` name.
    pub poc_st_curr_before: [u8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub poc_st_curr_after: [u8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub poc_lt_curr: [u8; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub num_delta_pocs_of_ref_rps_idx: u8,
    pub reserved: [u8; 3],
    pub dpb: [HevcDpbEntry; V4L2_HEVC_DPB_ENTRIES_NUM_MAX],
    pub flags: u64,
}

impl Default for HevcDecodeParams {
    fn default() -> Self {
        zeroed()
    }
}

/// In the block's raster, not the diagonal scan the kernel docs describe.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HevcScalingMatrix {
    pub scaling_list_4x4: [[u8; 16]; 6],
    pub scaling_list_8x8: [[u8; 64]; 6],
    pub scaling_list_16x16: [[u8; 64]; 6],
    pub scaling_list_32x32: [[u8; 64]; 2],
    pub scaling_list_dc_coef_16x16: [u8; 6],
    pub scaling_list_dc_coef_32x32: [u8; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controls_are_the_size_the_kernel_declares() {
        assert_eq!(size_of::<Mpeg2Sequence>(), 12);
        assert_eq!(size_of::<Mpeg2Picture>(), 32);
        assert_eq!(size_of::<Mpeg2Quantisation>(), 256);
        assert_eq!(size_of::<H264Sps>(), 1048);
        assert_eq!(size_of::<H264Pps>(), 12);
        assert_eq!(size_of::<H264ScalingMatrix>(), 480);
        assert_eq!(size_of::<H264WeightFactors>(), 384);
        assert_eq!(size_of::<H264PredWeights>(), 772);
        assert_eq!(size_of::<H264SliceParams>(), 152);
        assert_eq!(size_of::<H264DpbEntry>(), 32);
        assert_eq!(size_of::<H264DecodeParams>(), 560);
        assert_eq!(size_of::<H264Reference>(), 2);
        assert_eq!(size_of::<HevcSps>(), 40);
        assert_eq!(size_of::<HevcPps>(), 64);
        assert_eq!(size_of::<HevcDpbEntry>(), 16);
        assert_eq!(size_of::<HevcPredWeightTable>(), 194);
        assert_eq!(size_of::<HevcSliceParams>(), 280);
        assert_eq!(size_of::<HevcDecodeParams>(), 328);
        assert_eq!(size_of::<HevcScalingMatrix>(), 1000);
    }

    #[test]
    fn the_fields_are_where_the_kernel_puts_them() {
        assert_eq!(std::mem::offset_of!(H264Sps, flags), 1044);
        assert_eq!(std::mem::offset_of!(H264SliceParams, ref_pic_list0), 20);
        assert_eq!(std::mem::offset_of!(H264SliceParams, flags), 148);
        assert_eq!(std::mem::offset_of!(H264DecodeParams, flags), 556);
        assert_eq!(std::mem::offset_of!(HevcSliceParams, pred_weight_table), 76);
        assert_eq!(std::mem::offset_of!(HevcSliceParams, flags), 272);
        assert_eq!(std::mem::offset_of!(HevcDecodeParams, dpb), 64);
        assert_eq!(std::mem::offset_of!(HevcDecodeParams, flags), 320);
    }

    #[test]
    fn every_control_reports_the_size_of_its_own_structure() {
        assert_eq!(Kind::HevcSliceParams.size(), size_of::<HevcSliceParams>());
        assert_eq!(Kind::H264DecodeParams.size(), size_of::<H264DecodeParams>());
        assert_eq!(Kind::Mpeg2Picture.size(), size_of::<Mpeg2Picture>());
    }

    #[test]
    fn the_control_identifiers_are_the_ones_the_header_computes() {
        assert_eq!(V4L2_CID_STATELESS_H264_SPS, 0x00a4_0902);
        assert_eq!(V4L2_CID_STATELESS_MPEG2_SEQUENCE, 0x00a4_09dc);
        assert_eq!(V4L2_CID_STATELESS_HEVC_SPS, 0x00a4_0a90);
        assert_eq!(V4L2_CID_MPEG_VIDEO_HEVC_SPS, 0x0099_0cf0);
        assert_eq!(Kind::HevcSps.id_for(Naming::MpegVideo), 0x0099_0cf0);
        assert_eq!(Kind::HevcSps.id_for(Naming::Stateless), 0x00a4_0a90);
        assert_eq!(Kind::H264Sps.legacy(), None);
        assert_eq!(
            Kind::Mpeg2Picture.id_for(Naming::MpegVideo),
            Kind::Mpeg2Picture.id()
        );
    }

    #[test]
    fn a_control_carries_the_bytes_of_its_structure() {
        let sequence = Mpeg2Sequence {
            horizontal_size: 0x0201,
            vertical_size: 0x0403,
            ..Mpeg2Sequence::default()
        };
        let control = Ctl::of(Kind::Mpeg2Sequence, &sequence);
        assert_eq!(control.bytes.len(), 12);
        assert_eq!(&control.bytes[..4], &[0x01, 0x02, 0x03, 0x04]);
    }
}
