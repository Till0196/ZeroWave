// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 構造体のレイアウトがヘッダーのとおりかのテスト。

use super::*;
use std::mem::offset_of;

/// 数は `nv-codec-headers` を gcc（LP64）と mingw（LLP64）で取り出したもの。
/// ずれると `CUDA_ERROR_NOT_SUPPORTED` が返り、デコーダーが無いように見える。
#[test]
fn the_layouts_are_what_the_headers_say_on_this_machine() {
    assert_eq!(size_of::<DecodeCaps>(), 88);
    assert_eq!(offset_of!(DecodeCaps, codec), 0);
    assert_eq!(offset_of!(DecodeCaps, chroma_format), 4);
    assert_eq!(offset_of!(DecodeCaps, bit_depth_minus8), 8);
    assert_eq!(offset_of!(DecodeCaps, reserved1), 12);
    assert_eq!(offset_of!(DecodeCaps, supported), 24);
    assert_eq!(offset_of!(DecodeCaps, num_nvdecs), 25);
    assert_eq!(offset_of!(DecodeCaps, output_format_mask), 26);
    assert_eq!(offset_of!(DecodeCaps, max_width), 28);
    assert_eq!(offset_of!(DecodeCaps, max_height), 32);
    assert_eq!(offset_of!(DecodeCaps, max_mb_count), 36);
    assert_eq!(offset_of!(DecodeCaps, min_width), 40);
    assert_eq!(offset_of!(DecodeCaps, min_height), 42);
    assert_eq!(offset_of!(DecodeCaps, histogram_supported), 44);
    assert_eq!(offset_of!(DecodeCaps, counter_bit_depth), 45);
    assert_eq!(offset_of!(DecodeCaps, max_histogram_bins), 46);
    assert_eq!(offset_of!(DecodeCaps, decode_stats_supported), 48);
    assert_eq!(offset_of!(DecodeCaps, reserved4), 49);
    assert_eq!(offset_of!(DecodeCaps, reserved3), 52);

    #[cfg(not(windows))]
    assert_eq!(size_of::<DecodeCreateInfo>(), 176);
    #[cfg(windows)]
    assert_eq!(size_of::<DecodeCreateInfo>(), 112);
    assert_eq!(offset_of!(DecodeCreateInfo, width), 0);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, height), 8);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, height), 4);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, num_decode_surfaces), 16);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, num_decode_surfaces), 8);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, codec), 24);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, codec), 12);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, chroma_format), 28);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, chroma_format), 16);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, creation_flags), 32);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, creation_flags), 20);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, bit_depth_minus8), 40);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, bit_depth_minus8), 24);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, intra_decode_only), 48);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, intra_decode_only), 28);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, max_width), 56);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, max_width), 32);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, max_height), 64);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, max_height), 36);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, reserved1), 72);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, reserved1), 40);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, display_area), 80);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, display_area), 44);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, output_format), 88);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, output_format), 52);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, deinterlace_mode), 92);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, deinterlace_mode), 56);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, target_width), 96);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, target_width), 60);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, target_height), 104);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, target_height), 64);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, num_output_surfaces), 112);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, num_output_surfaces), 68);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, vid_lock), 120);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, vid_lock), 72);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, target_rect), 128);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, target_rect), 80);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, enable_histogram), 136);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, enable_histogram), 88);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, enable_decode_features), 144);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, enable_decode_features), 92);
    #[cfg(not(windows))]
    assert_eq!(offset_of!(DecodeCreateInfo, reserved2), 152);
    #[cfg(windows)]
    assert_eq!(offset_of!(DecodeCreateInfo, reserved2), 96);

    assert_eq!(size_of::<PicParams>(), 4280);
    assert_eq!(offset_of!(PicParams, width_in_mbs), 0);
    assert_eq!(offset_of!(PicParams, height_in_mbs), 4);
    assert_eq!(offset_of!(PicParams, curr_pic_idx), 8);
    assert_eq!(offset_of!(PicParams, field_pic_flag), 12);
    assert_eq!(offset_of!(PicParams, bottom_field_flag), 16);
    assert_eq!(offset_of!(PicParams, second_field), 20);
    assert_eq!(offset_of!(PicParams, bitstream_data_len), 24);
    assert_eq!(offset_of!(PicParams, bitstream_data), 32);
    assert_eq!(offset_of!(PicParams, num_slices), 40);
    assert_eq!(offset_of!(PicParams, slice_data_offsets), 48);
    assert_eq!(offset_of!(PicParams, ref_pic_flag), 56);
    assert_eq!(offset_of!(PicParams, intra_pic_flag), 60);
    assert_eq!(offset_of!(PicParams, timestamp), 64);
    assert_eq!(offset_of!(PicParams, reserved), 72);
    assert_eq!(offset_of!(PicParams, codec), 184);

    assert_eq!(size_of::<Mpeg2PicParams>(), 192);
    assert_eq!(offset_of!(Mpeg2PicParams, forward_ref_idx), 0);
    assert_eq!(offset_of!(Mpeg2PicParams, backward_ref_idx), 4);
    assert_eq!(offset_of!(Mpeg2PicParams, picture_coding_type), 8);
    assert_eq!(offset_of!(Mpeg2PicParams, full_pel_forward_vector), 12);
    assert_eq!(offset_of!(Mpeg2PicParams, full_pel_backward_vector), 16);
    assert_eq!(offset_of!(Mpeg2PicParams, f_code), 20);
    assert_eq!(offset_of!(Mpeg2PicParams, intra_dc_precision), 36);
    assert_eq!(offset_of!(Mpeg2PicParams, frame_pred_frame_dct), 40);
    assert_eq!(offset_of!(Mpeg2PicParams, concealment_motion_vectors), 44);
    assert_eq!(offset_of!(Mpeg2PicParams, q_scale_type), 48);
    assert_eq!(offset_of!(Mpeg2PicParams, intra_vlc_format), 52);
    assert_eq!(offset_of!(Mpeg2PicParams, alternate_scan), 56);
    assert_eq!(offset_of!(Mpeg2PicParams, top_field_first), 60);
    assert_eq!(offset_of!(Mpeg2PicParams, quant_matrix_intra), 64);
    assert_eq!(offset_of!(Mpeg2PicParams, quant_matrix_inter), 128);

    assert_eq!(size_of::<H264DpbEntry>(), 28);
    assert_eq!(offset_of!(H264DpbEntry, pic_idx), 0);
    assert_eq!(offset_of!(H264DpbEntry, frame_idx), 4);
    assert_eq!(offset_of!(H264DpbEntry, is_long_term), 8);
    assert_eq!(offset_of!(H264DpbEntry, not_existing), 12);
    assert_eq!(offset_of!(H264DpbEntry, used_for_reference), 16);
    assert_eq!(offset_of!(H264DpbEntry, field_order_cnt), 20);

    assert_eq!(size_of::<H264PicParams>(), 984);
    assert_eq!(offset_of!(H264PicParams, log2_max_frame_num_minus4), 0);
    assert_eq!(offset_of!(H264PicParams, pic_order_cnt_type), 4);
    assert_eq!(
        offset_of!(H264PicParams, log2_max_pic_order_cnt_lsb_minus4),
        8
    );
    assert_eq!(
        offset_of!(H264PicParams, delta_pic_order_always_zero_flag),
        12
    );
    assert_eq!(offset_of!(H264PicParams, frame_mbs_only_flag), 16);
    assert_eq!(offset_of!(H264PicParams, direct_8x8_inference_flag), 20);
    assert_eq!(offset_of!(H264PicParams, num_ref_frames), 24);
    assert_eq!(
        offset_of!(H264PicParams, residual_colour_transform_flag),
        28
    );
    assert_eq!(offset_of!(H264PicParams, bit_depth_luma_minus8), 29);
    assert_eq!(offset_of!(H264PicParams, bit_depth_chroma_minus8), 30);
    assert_eq!(
        offset_of!(H264PicParams, qpprime_y_zero_transform_bypass_flag),
        31
    );
    assert_eq!(offset_of!(H264PicParams, entropy_coding_mode_flag), 32);
    assert_eq!(offset_of!(H264PicParams, pic_order_present_flag), 36);
    assert_eq!(offset_of!(H264PicParams, num_ref_idx_l0_active_minus1), 40);
    assert_eq!(offset_of!(H264PicParams, num_ref_idx_l1_active_minus1), 44);
    assert_eq!(offset_of!(H264PicParams, weighted_pred_flag), 48);
    assert_eq!(offset_of!(H264PicParams, weighted_bipred_idc), 52);
    assert_eq!(offset_of!(H264PicParams, pic_init_qp_minus26), 56);
    assert_eq!(
        offset_of!(H264PicParams, deblocking_filter_control_present_flag),
        60
    );
    assert_eq!(
        offset_of!(H264PicParams, redundant_pic_cnt_present_flag),
        64
    );
    assert_eq!(offset_of!(H264PicParams, transform_8x8_mode_flag), 68);
    assert_eq!(offset_of!(H264PicParams, mbaff_frame_flag), 72);
    assert_eq!(offset_of!(H264PicParams, constrained_intra_pred_flag), 76);
    assert_eq!(offset_of!(H264PicParams, chroma_qp_index_offset), 80);
    assert_eq!(offset_of!(H264PicParams, second_chroma_qp_index_offset), 84);
    assert_eq!(offset_of!(H264PicParams, ref_pic_flag), 88);
    assert_eq!(offset_of!(H264PicParams, frame_num), 92);
    assert_eq!(offset_of!(H264PicParams, curr_field_order_cnt), 96);
    assert_eq!(offset_of!(H264PicParams, dpb), 104);
    assert_eq!(offset_of!(H264PicParams, weight_scale_4x4), 552);
    assert_eq!(offset_of!(H264PicParams, weight_scale_8x8), 648);
    assert_eq!(offset_of!(H264PicParams, fmo_aso_enable), 776);
    assert_eq!(offset_of!(H264PicParams, num_slice_groups_minus1), 777);
    assert_eq!(offset_of!(H264PicParams, slice_group_map_type), 778);
    assert_eq!(offset_of!(H264PicParams, pic_init_qs_minus26), 779);
    assert_eq!(
        offset_of!(H264PicParams, slice_group_change_rate_minus1),
        780
    );
    assert_eq!(offset_of!(H264PicParams, fmo), 784);
    assert_eq!(offset_of!(H264PicParams, reserved), 796);

    assert_eq!(size_of::<HevcPicParams>(), 1484);
    assert_eq!(offset_of!(HevcPicParams, pic_width_in_luma_samples), 0);
    assert_eq!(offset_of!(HevcPicParams, pic_height_in_luma_samples), 4);
    assert_eq!(
        offset_of!(HevcPicParams, log2_min_luma_coding_block_size_minus3),
        8
    );
    assert_eq!(
        offset_of!(HevcPicParams, pcm_sample_bit_depth_chroma_minus1),
        16
    );
    assert_eq!(offset_of!(HevcPicParams, num_short_term_ref_pic_sets), 24);
    assert_eq!(offset_of!(HevcPicParams, bit_depth_luma_minus8), 32);
    assert_eq!(offset_of!(HevcPicParams, mv_hevc_enable), 38);
    assert_eq!(offset_of!(HevcPicParams, reserved1), 46);
    assert_eq!(
        offset_of!(HevcPicParams, dependent_slice_segments_enabled_flag),
        48
    );
    assert_eq!(offset_of!(HevcPicParams, constrained_intra_pred_flag), 56);
    assert_eq!(
        offset_of!(HevcPicParams, loop_filter_across_tiles_enabled_flag),
        64
    );
    assert_eq!(
        offset_of!(HevcPicParams, deblocking_filter_override_enabled_flag),
        72
    );
    assert_eq!(offset_of!(HevcPicParams, column_width_minus1), 80);
    assert_eq!(offset_of!(HevcPicParams, row_height_minus1), 122);
    assert_eq!(offset_of!(HevcPicParams, sps_range_extension_flag), 164);
    assert_eq!(offset_of!(HevcPicParams, explicit_rdpcm_enabled_flag), 168);
    assert_eq!(
        offset_of!(HevcPicParams, cabac_bypass_alignment_enabled_flag),
        172
    );
    assert_eq!(
        offset_of!(HevcPicParams, diff_cu_chroma_qp_offset_depth),
        176
    );
    assert_eq!(offset_of!(HevcPicParams, cb_qp_offset_list), 178);
    assert_eq!(offset_of!(HevcPicParams, cr_qp_offset_list), 184);
    assert_eq!(offset_of!(HevcPicParams, reserved2), 190);
    assert_eq!(offset_of!(HevcPicParams, reserved3), 192);
    assert_eq!(
        offset_of!(HevcPicParams, num_bits_for_short_term_rps_in_slice),
        224
    );
    assert_eq!(
        offset_of!(HevcPicParams, num_delta_pocs_of_ref_rps_idx),
        228
    );
    assert_eq!(offset_of!(HevcPicParams, num_poc_total_curr), 232);
    assert_eq!(offset_of!(HevcPicParams, num_poc_st_curr_before), 236);
    assert_eq!(offset_of!(HevcPicParams, num_poc_st_curr_after), 240);
    assert_eq!(offset_of!(HevcPicParams, num_poc_lt_curr), 244);
    assert_eq!(offset_of!(HevcPicParams, curr_pic_order_cnt_val), 248);
    assert_eq!(offset_of!(HevcPicParams, ref_pic_idx), 252);
    assert_eq!(offset_of!(HevcPicParams, pic_order_cnt_val), 316);
    assert_eq!(offset_of!(HevcPicParams, is_long_term), 380);
    assert_eq!(offset_of!(HevcPicParams, ref_pic_set_st_curr_before), 396);
    assert_eq!(offset_of!(HevcPicParams, ref_pic_set_st_curr_after), 404);
    assert_eq!(offset_of!(HevcPicParams, ref_pic_set_lt_curr), 412);
    assert_eq!(offset_of!(HevcPicParams, ref_pic_set_inter_layer0), 420);
    assert_eq!(offset_of!(HevcPicParams, ref_pic_set_inter_layer1), 428);
    assert_eq!(offset_of!(HevcPicParams, reserved4), 436);
    assert_eq!(offset_of!(HevcPicParams, scaling_list_4x4), 484);
    assert_eq!(offset_of!(HevcPicParams, scaling_list_8x8), 580);
    assert_eq!(offset_of!(HevcPicParams, scaling_list_16x16), 964);
    assert_eq!(offset_of!(HevcPicParams, scaling_list_32x32), 1348);
    assert_eq!(offset_of!(HevcPicParams, scaling_list_dc_16x16), 1476);
    assert_eq!(offset_of!(HevcPicParams, scaling_list_dc_32x32), 1482);

    assert_eq!(size_of::<ProcParams>(), 264);
    assert_eq!(offset_of!(ProcParams, progressive_frame), 0);
    assert_eq!(offset_of!(ProcParams, second_field), 4);
    assert_eq!(offset_of!(ProcParams, top_field_first), 8);
    assert_eq!(offset_of!(ProcParams, unpaired_field), 12);
    assert_eq!(offset_of!(ProcParams, reserved_flags), 16);
    assert_eq!(offset_of!(ProcParams, reserved_zero), 20);
    assert_eq!(offset_of!(ProcParams, raw_input_dptr), 24);
    assert_eq!(offset_of!(ProcParams, raw_input_pitch), 32);
    assert_eq!(offset_of!(ProcParams, raw_input_format), 36);
    assert_eq!(offset_of!(ProcParams, raw_output_dptr), 40);
    assert_eq!(offset_of!(ProcParams, raw_output_pitch), 48);
    assert_eq!(offset_of!(ProcParams, reserved1), 52);
    assert_eq!(offset_of!(ProcParams, output_stream), 56);
    assert_eq!(offset_of!(ProcParams, reserved), 64);
    assert_eq!(offset_of!(ProcParams, histogram_dptr), 248);
    assert_eq!(offset_of!(ProcParams, proc_ext), 256);

    assert_eq!(size_of::<DecodeStatus>(), 192);
    assert_eq!(offset_of!(DecodeStatus, status), 0);
    assert_eq!(offset_of!(DecodeStatus, reserved), 4);
    assert_eq!(offset_of!(DecodeStatus, reserved_ptr), 128);

    assert_eq!(size_of::<Memcpy2D>(), 128);
    assert_eq!(offset_of!(Memcpy2D, src_x_in_bytes), 0);
    assert_eq!(offset_of!(Memcpy2D, src_y), 8);
    assert_eq!(offset_of!(Memcpy2D, src_memory_type), 16);
    assert_eq!(offset_of!(Memcpy2D, src_host), 24);
    assert_eq!(offset_of!(Memcpy2D, src_device), 32);
    assert_eq!(offset_of!(Memcpy2D, src_array), 40);
    assert_eq!(offset_of!(Memcpy2D, src_pitch), 48);
    assert_eq!(offset_of!(Memcpy2D, dst_x_in_bytes), 56);
    assert_eq!(offset_of!(Memcpy2D, dst_y), 64);
    assert_eq!(offset_of!(Memcpy2D, dst_memory_type), 72);
    assert_eq!(offset_of!(Memcpy2D, dst_host), 80);
    assert_eq!(offset_of!(Memcpy2D, dst_device), 88);
    assert_eq!(offset_of!(Memcpy2D, dst_array), 96);
    assert_eq!(offset_of!(Memcpy2D, dst_pitch), 104);
    assert_eq!(offset_of!(Memcpy2D, width_in_bytes), 112);
    assert_eq!(offset_of!(Memcpy2D, height), 120);
}

#[test]
fn the_c_long_is_the_width_the_machine_gives_it() {
    assert_eq!(size_of::<CuUlong>(), size_of::<std::ffi::c_ulong>());
    #[cfg(windows)]
    assert_eq!(size_of::<CuUlong>(), 4);
    #[cfg(not(windows))]
    assert_eq!(size_of::<CuUlong>(), 8);
}

#[test]
fn every_codec_fits_the_union() {
    assert!(size_of::<Mpeg2PicParams>() <= 4096);
    assert!(size_of::<H264PicParams>() <= 4096);
    assert!(size_of::<HevcPicParams>() <= 4096);
    assert!(align_of::<H264PicParams>() <= align_of::<CodecSpecific>());
}

#[cfg(windows)]
#[test]
fn the_interop_structs_are_what_the_header_says() {
    assert_eq!(size_of::<ExternalMemoryHandleDesc>(), 104);
    assert_eq!(offset_of!(ExternalMemoryHandleDesc, handle), 8);
    assert_eq!(offset_of!(ExternalMemoryHandleDesc, size), 24);
    assert_eq!(offset_of!(ExternalMemoryHandleDesc, flags), 32);
    assert_eq!(offset_of!(ExternalMemoryHandleDesc, reserved), 36);
    assert_eq!(size_of::<ExternalHandle>(), 16);
    assert_eq!(offset_of!(ExternalHandle, win32_name), 8);

    assert_eq!(size_of::<Array3dDescriptor>(), 40);
    assert_eq!(offset_of!(Array3dDescriptor, height), 8);
    assert_eq!(offset_of!(Array3dDescriptor, depth), 16);
    assert_eq!(offset_of!(Array3dDescriptor, format), 24);
    assert_eq!(offset_of!(Array3dDescriptor, num_channels), 28);
    assert_eq!(offset_of!(Array3dDescriptor, flags), 32);

    assert_eq!(size_of::<ExternalMemoryMipmappedArrayDesc>(), 120);
    assert_eq!(offset_of!(ExternalMemoryMipmappedArrayDesc, array_desc), 8);
    assert_eq!(offset_of!(ExternalMemoryMipmappedArrayDesc, num_levels), 48);
    assert_eq!(offset_of!(ExternalMemoryMipmappedArrayDesc, reserved), 52);

    assert_eq!(size_of::<ExternalSemaphoreHandleDesc>(), 96);
    assert_eq!(offset_of!(ExternalSemaphoreHandleDesc, handle), 8);
    assert_eq!(offset_of!(ExternalSemaphoreHandleDesc, flags), 24);
    assert_eq!(offset_of!(ExternalSemaphoreHandleDesc, reserved), 28);

    assert_eq!(size_of::<ExternalSemaphoreParams>(), 144);
    assert_eq!(offset_of!(ExternalSemaphoreParams, params_reserved), 8);
    assert_eq!(offset_of!(ExternalSemaphoreParams, flags), 72);
    assert_eq!(offset_of!(ExternalSemaphoreParams, reserved), 76);
}

#[cfg(unix)]
#[test]
fn the_memory_structs_are_the_size_the_driver_api_declares() {
    assert_eq!(size_of::<MemLocation>(), 8);
    assert_eq!(size_of::<MemAllocationProp>(), 32);
    assert_eq!(offset_of!(MemAllocationProp, location), 8);
    assert_eq!(offset_of!(MemAllocationProp, win32_handle_meta_data), 16);
    assert_eq!(offset_of!(MemAllocationProp, compression_type), 24);
    assert_eq!(size_of::<MemAccessDesc>(), 12);
}
