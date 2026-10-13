// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードと共有の構造体（`nv-codec-headers` から移したもの。そこに無い CUDA の型は NVIDIA の公開ドキュメントから）。

use super::*;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodeCaps {
    pub codec: c_int,
    pub chroma_format: c_int,
    pub bit_depth_minus8: c_uint,
    pub reserved1: [c_uint; 3],
    pub supported: u8,
    pub num_nvdecs: u8,
    pub output_format_mask: u16,
    pub max_width: c_uint,
    pub max_height: c_uint,
    pub max_mb_count: c_uint,
    pub min_width: u16,
    pub min_height: u16,
    pub histogram_supported: u8,
    pub counter_bit_depth: u8,
    pub max_histogram_bins: u16,
    pub decode_stats_supported: u8,
    pub reserved4: [u8; 3],
    pub reserved3: [c_uint; 9],
}

/// `CUVIDDECODECREATEINFO`。レイアウトはプラットフォームで変わる（Linux 176 バイト、Windows 112）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodeCreateInfo {
    pub width: CuUlong,
    pub height: CuUlong,
    pub num_decode_surfaces: CuUlong,
    pub codec: c_int,
    pub chroma_format: c_int,
    pub creation_flags: CuUlong,
    pub bit_depth_minus8: CuUlong,
    pub intra_decode_only: CuUlong,
    pub max_width: CuUlong,
    pub max_height: CuUlong,
    pub reserved1: CuUlong,
    pub display_area: [i16; 4],
    pub output_format: c_int,
    pub deinterlace_mode: c_int,
    pub target_width: CuUlong,
    pub target_height: CuUlong,
    pub num_output_surfaces: CuUlong,
    pub vid_lock: *mut c_void,
    pub target_rect: [i16; 4],
    pub enable_histogram: CuUlong,
    pub enable_decode_features: CuUlong,
    pub reserved2: [CuUlong; 3],
}

/// `CUVIDPICPARAMS` のコーデックごとの共用体（`CodecReserved[1024]`）。
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct CodecSpecific(pub [u8; 4096]);

impl Default for CodecSpecific {
    fn default() -> Self {
        Self([0; 4096])
    }
}

impl std::fmt::Debug for CodecSpecific {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("CodecSpecific")
    }
}

impl CodecSpecific {
    pub fn put<T: Copy>(&mut self, value: &T) {
        assert!(size_of::<T>() <= 4096);
        // SAFETY: `T` は `repr(C)` の値型で、格納先はそれより広く、アラインメントは 8 以下（`mod tests`）。
        unsafe {
            std::ptr::copy_nonoverlapping(
                (value as *const T).cast::<u8>(),
                self.0.as_mut_ptr(),
                size_of::<T>(),
            );
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PicParams {
    pub width_in_mbs: c_int,
    pub height_in_mbs: c_int,
    pub curr_pic_idx: c_int,
    pub field_pic_flag: c_int,
    pub bottom_field_flag: c_int,
    pub second_field: c_int,
    pub bitstream_data_len: c_uint,
    pub bitstream_data: *const u8,
    pub num_slices: c_uint,
    pub slice_data_offsets: *const c_uint,
    pub ref_pic_flag: c_int,
    pub intra_pic_flag: c_int,
    pub timestamp: i64,
    pub reserved: [c_uint; 28],
    pub codec: CodecSpecific,
}

impl Default for PicParams {
    fn default() -> Self {
        Self {
            width_in_mbs: 0,
            height_in_mbs: 0,
            curr_pic_idx: 0,
            field_pic_flag: 0,
            bottom_field_flag: 0,
            second_field: 0,
            bitstream_data_len: 0,
            bitstream_data: std::ptr::null(),
            num_slices: 0,
            slice_data_offsets: std::ptr::null(),
            ref_pic_flag: 0,
            intra_pic_flag: 0,
            timestamp: 0,
            reserved: [0; 28],
            codec: CodecSpecific::default(),
        }
    }
}

/// 長い配列を持ち `Default` を derive できない、整数と整数の配列だけの構造体に全ビット 0 の既定値を付ける。
macro_rules! zeroed_default {
    ($($name:ident),+) => {$(
        impl Default for $name {
            fn default() -> Self {
                // SAFETY: 整数と整数の配列だけの `repr(C)` で、全ビット 0 は正しい値。
                unsafe { std::mem::zeroed() }
            }
        }
    )+};
}

zeroed_default!(Mpeg2PicParams, H264PicParams, HevcPicParams);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Mpeg2PicParams {
    pub forward_ref_idx: c_int,
    pub backward_ref_idx: c_int,
    pub picture_coding_type: c_int,
    pub full_pel_forward_vector: c_int,
    pub full_pel_backward_vector: c_int,
    pub f_code: [[c_int; 2]; 2],
    pub intra_dc_precision: c_int,
    pub frame_pred_frame_dct: c_int,
    pub concealment_motion_vectors: c_int,
    pub q_scale_type: c_int,
    pub intra_vlc_format: c_int,
    pub alternate_scan: c_int,
    pub top_field_first: c_int,
    /// ラスター順。zig-zag ではない。
    pub quant_matrix_intra: [u8; 64],
    pub quant_matrix_inter: [u8; 64],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct H264DpbEntry {
    /// 参照しているサーフェスの番号。使っていないスロットは -1。
    pub pic_idx: c_int,
    pub frame_idx: c_int,
    pub is_long_term: c_int,
    pub not_existing: c_int,
    pub used_for_reference: c_int,
    pub field_order_cnt: [c_int; 2],
}

/// `CUVIDH264PICPARAMS`。末尾の SVC/MVC 共用体は 0 のまま。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H264PicParams {
    pub log2_max_frame_num_minus4: c_int,
    pub pic_order_cnt_type: c_int,
    pub log2_max_pic_order_cnt_lsb_minus4: c_int,
    pub delta_pic_order_always_zero_flag: c_int,
    pub frame_mbs_only_flag: c_int,
    pub direct_8x8_inference_flag: c_int,
    pub num_ref_frames: c_int,
    pub residual_colour_transform_flag: u8,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub qpprime_y_zero_transform_bypass_flag: u8,
    pub entropy_coding_mode_flag: c_int,
    pub pic_order_present_flag: c_int,
    pub num_ref_idx_l0_active_minus1: c_int,
    pub num_ref_idx_l1_active_minus1: c_int,
    pub weighted_pred_flag: c_int,
    pub weighted_bipred_idc: c_int,
    pub pic_init_qp_minus26: c_int,
    pub deblocking_filter_control_present_flag: c_int,
    pub redundant_pic_cnt_present_flag: c_int,
    pub transform_8x8_mode_flag: c_int,
    pub mbaff_frame_flag: c_int,
    pub constrained_intra_pred_flag: c_int,
    pub chroma_qp_index_offset: c_int,
    pub second_chroma_qp_index_offset: c_int,
    pub ref_pic_flag: c_int,
    pub frame_num: c_int,
    pub curr_field_order_cnt: [c_int; 2],
    pub dpb: [H264DpbEntry; 16],
    pub weight_scale_4x4: [[u8; 16]; 6],
    pub weight_scale_8x8: [[u8; 64]; 2],
    pub fmo_aso_enable: u8,
    pub num_slice_groups_minus1: u8,
    pub slice_group_map_type: u8,
    pub pic_init_qs_minus26: i8,
    pub slice_group_change_rate_minus1: c_uint,
    pub fmo: u64,
    /// 下位ビットがフラグ。上のビットが立っていないとドライバーは下のビットを読まない。
    pub mb_adaptive_frame_field_flag: c_uint,
    pub reserved: [c_uint; 11],
    pub ext: [u8; 144],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HevcPicParams {
    pub pic_width_in_luma_samples: c_int,
    pub pic_height_in_luma_samples: c_int,
    pub log2_min_luma_coding_block_size_minus3: u8,
    pub log2_diff_max_min_luma_coding_block_size: u8,
    pub log2_min_transform_block_size_minus2: u8,
    pub log2_diff_max_min_transform_block_size: u8,
    pub pcm_enabled_flag: u8,
    pub log2_min_pcm_luma_coding_block_size_minus3: u8,
    pub log2_diff_max_min_pcm_luma_coding_block_size: u8,
    pub pcm_sample_bit_depth_luma_minus1: u8,
    pub pcm_sample_bit_depth_chroma_minus1: u8,
    pub pcm_loop_filter_disabled_flag: u8,
    pub strong_intra_smoothing_enabled_flag: u8,
    pub max_transform_hierarchy_depth_intra: u8,
    pub max_transform_hierarchy_depth_inter: u8,
    pub amp_enabled_flag: u8,
    pub separate_colour_plane_flag: u8,
    pub log2_max_pic_order_cnt_lsb_minus4: u8,
    pub num_short_term_ref_pic_sets: u8,
    pub long_term_ref_pics_present_flag: u8,
    pub num_long_term_ref_pics_sps: u8,
    pub sps_temporal_mvp_enabled_flag: u8,
    pub sample_adaptive_offset_enabled_flag: u8,
    pub scaling_list_enable_flag: u8,
    pub irap_pic_flag: u8,
    pub idr_pic_flag: u8,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub log2_max_transform_skip_block_size_minus2: u8,
    pub log2_sao_offset_scale_luma: u8,
    pub log2_sao_offset_scale_chroma: u8,
    pub high_precision_offsets_enabled_flag: u8,
    pub mv_hevc_enable: u8,
    pub nuh_layer_id: u8,
    pub default_ref_layers_active_flag: u8,
    pub num_direct_ref_layers: u8,
    pub max_one_active_ref_layer_flag: u8,
    pub poc_lsb_not_present_flag: u8,
    pub num_active_ref_layer_pics0: u8,
    pub num_active_ref_layer_pics1: u8,
    pub reserved1: [u8; 2],
    pub dependent_slice_segments_enabled_flag: u8,
    pub slice_segment_header_extension_present_flag: u8,
    pub sign_data_hiding_enabled_flag: u8,
    pub cu_qp_delta_enabled_flag: u8,
    pub diff_cu_qp_delta_depth: u8,
    pub init_qp_minus26: i8,
    pub pps_cb_qp_offset: i8,
    pub pps_cr_qp_offset: i8,
    pub constrained_intra_pred_flag: u8,
    pub weighted_pred_flag: u8,
    pub weighted_bipred_flag: u8,
    pub transform_skip_enabled_flag: u8,
    pub transquant_bypass_enabled_flag: u8,
    pub entropy_coding_sync_enabled_flag: u8,
    pub log2_parallel_merge_level_minus2: u8,
    pub num_extra_slice_header_bits: u8,
    pub loop_filter_across_tiles_enabled_flag: u8,
    pub loop_filter_across_slices_enabled_flag: u8,
    pub output_flag_present_flag: u8,
    pub num_ref_idx_l0_default_active_minus1: u8,
    pub num_ref_idx_l1_default_active_minus1: u8,
    pub lists_modification_present_flag: u8,
    pub cabac_init_present_flag: u8,
    pub pps_slice_chroma_qp_offsets_present_flag: u8,
    pub deblocking_filter_override_enabled_flag: u8,
    pub pps_deblocking_filter_disabled_flag: u8,
    pub pps_beta_offset_div2: i8,
    pub pps_tc_offset_div2: i8,
    pub tiles_enabled_flag: u8,
    pub uniform_spacing_flag: u8,
    pub num_tile_columns_minus1: u8,
    pub num_tile_rows_minus1: u8,
    pub column_width_minus1: [u16; 21],
    pub row_height_minus1: [u16; 21],
    pub sps_range_extension_flag: u8,
    pub transform_skip_rotation_enabled_flag: u8,
    pub transform_skip_context_enabled_flag: u8,
    pub implicit_rdpcm_enabled_flag: u8,
    pub explicit_rdpcm_enabled_flag: u8,
    pub extended_precision_processing_flag: u8,
    pub intra_smoothing_disabled_flag: u8,
    pub persistent_rice_adaptation_enabled_flag: u8,
    pub cabac_bypass_alignment_enabled_flag: u8,
    pub pps_range_extension_flag: u8,
    pub cross_component_prediction_enabled_flag: u8,
    pub chroma_qp_offset_list_enabled_flag: u8,
    pub diff_cu_chroma_qp_offset_depth: u8,
    pub chroma_qp_offset_list_len_minus1: u8,
    pub cb_qp_offset_list: [i8; 6],
    pub cr_qp_offset_list: [i8; 6],
    pub reserved2: [u8; 2],
    pub reserved3: [c_uint; 8],
    pub num_bits_for_short_term_rps_in_slice: c_int,
    pub num_delta_pocs_of_ref_rps_idx: c_int,
    pub num_poc_total_curr: c_int,
    pub num_poc_st_curr_before: c_int,
    pub num_poc_st_curr_after: c_int,
    pub num_poc_lt_curr: c_int,
    pub curr_pic_order_cnt_val: c_int,
    pub ref_pic_idx: [c_int; 16],
    pub pic_order_cnt_val: [c_int; 16],
    pub is_long_term: [u8; 16],
    pub ref_pic_set_st_curr_before: [u8; 8],
    pub ref_pic_set_st_curr_after: [u8; 8],
    pub ref_pic_set_lt_curr: [u8; 8],
    pub ref_pic_set_inter_layer0: [u8; 8],
    pub ref_pic_set_inter_layer1: [u8; 8],
    pub reserved4: [c_uint; 12],
    /// 対角順の量子化行列。H.264 のラスター順とは違う。
    pub scaling_list_4x4: [[u8; 16]; 6],
    pub scaling_list_8x8: [[u8; 64]; 6],
    pub scaling_list_16x16: [[u8; 64]; 6],
    pub scaling_list_32x32: [[u8; 64]; 2],
    pub scaling_list_dc_16x16: [u8; 6],
    pub scaling_list_dc_32x32: [u8; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ProcParams {
    pub progressive_frame: c_int,
    pub second_field: c_int,
    pub top_field_first: c_int,
    pub unpaired_field: c_int,
    pub reserved_flags: c_uint,
    pub reserved_zero: c_uint,
    pub raw_input_dptr: u64,
    pub raw_input_pitch: c_uint,
    pub raw_input_format: c_uint,
    pub raw_output_dptr: u64,
    pub raw_output_pitch: c_uint,
    pub reserved1: c_uint,
    pub output_stream: CUstream,
    pub reserved: [c_uint; 46],
    pub histogram_dptr: *mut u64,
    pub proc_ext: *mut c_void,
}

impl Default for ProcParams {
    fn default() -> Self {
        Self {
            progressive_frame: 0,
            second_field: 0,
            top_field_first: 0,
            unpaired_field: 0,
            reserved_flags: 0,
            reserved_zero: 0,
            raw_input_dptr: 0,
            raw_input_pitch: 0,
            raw_input_format: 0,
            raw_output_dptr: 0,
            raw_output_pitch: 0,
            reserved1: 0,
            output_stream: std::ptr::null_mut(),
            reserved: [0; 46],
            histogram_dptr: std::ptr::null_mut(),
            proc_ext: std::ptr::null_mut(),
        }
    }
}

/// `CUVIDGETDECODESTATUS`。本当にデコードできたかを問い合わせる構造体。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodeStatus {
    pub status: c_int,
    pub reserved: [c_uint; 31],
    pub reserved_ptr: [*mut c_void; 8],
}

pub const CUVID_DECODE_STATUS_SUCCESS: c_int = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Memcpy2D {
    pub src_x_in_bytes: usize,
    pub src_y: usize,
    pub src_memory_type: c_int,
    pub src_host: *const c_void,
    pub src_device: CUdeviceptr,
    pub src_array: *mut c_void,
    pub src_pitch: usize,
    pub dst_x_in_bytes: usize,
    pub dst_y: usize,
    pub dst_memory_type: c_int,
    pub dst_host: *mut c_void,
    pub dst_device: CUdeviceptr,
    pub dst_array: *mut c_void,
    pub dst_pitch: usize,
    pub width_in_bytes: usize,
    pub height: usize,
}

impl Default for Memcpy2D {
    fn default() -> Self {
        Self {
            src_x_in_bytes: 0,
            src_y: 0,
            src_memory_type: 0,
            src_host: std::ptr::null(),
            src_device: 0,
            src_array: std::ptr::null_mut(),
            src_pitch: 0,
            dst_x_in_bytes: 0,
            dst_y: 0,
            dst_memory_type: 0,
            dst_host: std::ptr::null_mut(),
            dst_device: 0,
            dst_array: std::ptr::null_mut(),
            dst_pitch: 0,
            width_in_bytes: 0,
            height: 0,
        }
    }
}

#[cfg(windows)]
pub type CUexternalMemory = *mut c_void;
#[cfg(windows)]
pub type CUexternalSemaphore = *mut c_void;
#[cfg(windows)]
pub type CUarray = *mut c_void;
#[cfg(windows)]
pub type CUmipmappedArray = *mut c_void;

/// `CUDA_EXTERNAL_MEMORY_DEDICATED`。D3D12 のリソースには必ず要る。
#[cfg(windows)]
pub const CUDA_EXTERNAL_MEMORY_DEDICATED: c_uint = 0x1;
#[cfg(windows)]
pub const CU_EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE: c_int = 5;
#[cfg(windows)]
pub const CU_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE: c_int = 4;
#[cfg(windows)]
pub const CU_AD_FORMAT_NV12: c_int = 0xb0;
#[cfg(windows)]
pub const CU_AD_FORMAT_P016: c_int = 0xa1;
#[cfg(windows)]
pub const CUDA_ARRAY3D_SURFACE_LDST: c_uint = 0x02;
#[cfg(windows)]
pub const CUDA_ARRAY3D_VIDEO_ENCODE_DECODE: c_uint = 0x100;
#[cfg(windows)]
pub const CU_MEMORYTYPE_ARRAY: c_int = 3;

/// 外部メモリー・フェンスのハンドルの共用体。名前は使わない（NT のハンドルを直に渡す）。
#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalHandle {
    pub win32_handle: *mut c_void,
    pub win32_name: *const c_void,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalMemoryHandleDesc {
    pub kind: c_int,
    pub handle: ExternalHandle,
    pub size: u64,
    pub flags: c_uint,
    pub reserved: [c_uint; 16],
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Array3dDescriptor {
    pub width: usize,
    pub height: usize,
    pub depth: usize,
    pub format: c_int,
    pub num_channels: c_uint,
    pub flags: c_uint,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalMemoryMipmappedArrayDesc {
    pub offset: u64,
    pub array_desc: Array3dDescriptor,
    pub num_levels: c_uint,
    pub reserved: [c_uint; 16],
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalSemaphoreHandleDesc {
    pub kind: c_int,
    pub handle: ExternalHandle,
    pub flags: c_uint,
    pub reserved: [c_uint; 16],
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalMemoryBufferDesc {
    pub offset: u64,
    pub size: u64,
    pub flags: c_uint,
    pub reserved: [c_uint; 16],
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalSemaphoreParams {
    pub fence_value: u64,
    pub params_reserved: [c_uint; 16],
    pub flags: c_uint,
    pub reserved: [c_uint; 16],
}

#[cfg(unix)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MemLocation {
    pub kind: c_int,
    pub id: c_int,
}

#[cfg(unix)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MemAllocationProp {
    pub kind: c_int,
    pub requested_handle_types: c_int,
    pub location: MemLocation,
    pub win32_handle_meta_data: *mut c_void,
    pub compression_type: u8,
    pub gpu_direct_rdma_capable: u8,
    pub usage: u16,
    pub reserved: [u8; 4],
}

#[cfg(unix)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MemAccessDesc {
    pub location: MemLocation,
    pub flags: c_int,
}
