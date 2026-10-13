// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! H.264, handed to the hardware. D3D12 takes the short slice format; what is left is
//! clause 8.2 ([`mediadecoder::video::h264::dpb`]) written into `DXVA_PicParams_H264`.

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::h264::dpb::Marked;
use mediadecoder::video::h264::params::{self, Header, Pps, Sps};
use windows::Win32::Media::MediaFoundation::{
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL,
};

use crate::decode::Error;
use crate::decode::d3d12::dx;
use crate::decode::d3d12::dxva::{PicEntry, PicParamsH264, QmatrixH264, SliceShort};
use crate::decode::d3d12::pool::Pool;
use crate::decode::d3d12::{Held, References, Session, argument, slice_argument};
use crate::decode::frames;

/// ドライバーに渡す POC に足す値。DXVA では 0 が「参照でない」の意味で、POC 0 のフィールドを
/// 無いと解釈するドライバーがある。差は変わらない。
const POC_BASE: i32 = 1 << 16;

#[derive(Debug)]
pub struct H264 {
    framing: Framing,
    sets: params::Sets,
    order: frames::H264Order<usize>,
    report: u32,
    recovery: Recovery,
}

/// IDR でない所から始まったら、2 つ目の I までフレームを渡さない（参照としてデコードはする）。
/// 存在しない参照を指す B の状態を次の I まで引きずるドライバーがある。
#[derive(Debug, Default)]
enum Recovery {
    #[default]
    Unknown,
    Done,
    Waiting {
        intra: u32,
        anchor: Option<i32>,
    },
}

impl H264 {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            sets: params::Sets::default(),
            order: frames::H264Order::new(),
            report: 0,
            recovery: Recovery::Unknown,
        }
    }

    pub fn take(&mut self) -> Option<Held> {
        self.order.take()
    }

    pub fn flush(&mut self, pool: &mut Pool) {
        self.order.flush(pool);
    }

    pub fn reset(&mut self, pool: &mut Pool) {
        self.recovery = Recovery::Unknown;
        self.order.reset(pool);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        device: &mut dx::Device,
        session: &mut Session,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format above 4:2:0"));
        }
        let mut parameters = PicParamsH264::default();
        let mut matrix = QmatrixH264 {
            four: [[16; 16]; 6],
            eight: [[16; 64]; 2],
        };
        let mut slices: Vec<SliceShort> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        let mut slot = None;
        let mut used: Vec<usize> = Vec::new();
        let mut damage = None;

        for nal in Nals::new(data, self.framing) {
            let kind = nal.header & 0x1f;
            let idc = u32::from(nal.header >> 5) & 3;
            let body = data.get(nal.body.clone()).unwrap_or(&[]);
            match kind {
                7 | 8 => self.sets.absorb(kind, body),
                1 | 5 => {
                    let raw = data
                        .get(nal.body.start.saturating_sub(1)..nal.body.end)
                        .unwrap_or(&[]);
                    match self.slice(
                        session,
                        tag,
                        picture,
                        format,
                        kind == 5,
                        idc,
                        body,
                        slot.is_some(),
                    ) {
                        Ok(Some((at, made, weights, named))) => {
                            slot = Some(at);
                            parameters = made;
                            matrix = weights;
                            used = named;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            damage.get_or_insert(error);
                            continue;
                        }
                    }
                    if slot.is_none() {
                        continue;
                    }
                    slices.push(SliceShort {
                        location: bytes.len() as u32,
                        bytes: (raw.len() + 3) as u32,
                        bad_chopping: 0,
                    });
                    bytes.extend_from_slice(&[0, 0, 1]);
                    bytes.extend_from_slice(raw);
                }
                _ => {}
            }
        }

        let Some(slot) = slot else {
            return match damage {
                Some(error) => Err(error),
                None => Ok(()),
            };
        };
        let arguments = [
            argument(
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
                &parameters,
            ),
            argument(
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
                &matrix,
            ),
            slice_argument(D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL, &slices),
        ];
        // D3D11 がビットストリームとスライス情報に添えるマクロブロック数（フィールドなら半分）。
        let macroblocks = (format.width.div_ceil(16) * format.height.div_ceil(16))
            >> u32::from(picture.structure != mediadecoder::es::Structure::Frame);
        session.submit(
            device,
            slot,
            References::BySlot(&used),
            &bytes,
            &arguments,
            macroblocks,
        )?;
        self.order.finish(&mut session.pool);
        match damage {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn slice(
        &mut self,
        session: &mut Session,
        tag: u64,
        picture: &Picture,
        format: Format,
        idr: bool,
        idc: u32,
        body: &[u8],
        started: bool,
    ) -> Result<Option<(usize, PicParamsH264, QmatrixH264, Vec<usize>)>, Error> {
        let data = unescape(body, body.len());
        let found = params::header(&data, idr, idc, |id| self.sets.find(id));
        let Some((header, pps, sps)) = found else {
            return Err(Error::Damaged(
                "a slice with no parameter set to read it by",
            ));
        };
        if header.redundant != 0 {
            return Ok(None);
        }
        if started {
            return Ok(None);
        }
        // 途中から始まるピクチャー（`first_mb_in_slice != 0`）はデコードしない。ドライバーはエラーを返さずに何も書かない。
        if header.first_mb != 0 {
            return Ok(None);
        }
        // サーフェスはストリームが示すサイズで作った。SPS がより大きいサイズを示すとドライバーがサーフェスの外へ書く。
        if sps.mb_width * 16 > (format.width as usize).next_multiple_of(16)
            || sps.mb_height() * 16 > (format.height as usize).next_multiple_of(16)
        {
            return Err(Error::Damaged(
                "a sequence larger than the decoder was opened for",
            ));
        }

        self.order
            .start(&mut session.pool, tag, picture, &header, &sps)?;
        let Some(current) = self.order.current.clone() else {
            return Err(Error::Damaged("a slice with no picture"));
        };
        let at = self
            .order
            .dpb
            .index(current.serial)
            .ok_or(Error::Damaged("a picture that fell out of the buffer"))?;
        self.order.dpb.number_frames(&sps, header.frame_num);
        let slot = self.order.dpb.stores[at].slot;
        let references: Vec<usize> = self
            .order
            .dpb
            .stores
            .iter()
            .filter(|other| other.reference())
            .take(16)
            .map(|other| other.slot)
            .collect();
        session.pool.prepare(slot, &references)?;
        let (parameters, used) = self.picture_parameters(&session.pool, &header, &sps, &pps, at);
        // 存在しない参照からは予測させない（真緑が出る）。
        if used.is_empty() && !matches!(header.kind, params::Kind::I | params::Kind::Si) {
            return Ok(None);
        }
        let store = &self.order.dpb.stores[at];
        let second = header.field_pic && store.present[usize::from(!header.bottom_field)];
        if !second {
            let parity = usize::from(header.field_pic && header.bottom_field);
            let poc = store.poc[parity];
            let intra = { parameters.bit_fields } & 0x8000 != 0;
            if matches!(self.recovery, Recovery::Unknown) {
                self.recovery = if idr {
                    Recovery::Done
                } else {
                    Recovery::Waiting {
                        intra: 0,
                        anchor: None,
                    }
                };
            }
            let hide = match &mut self.recovery {
                Recovery::Waiting {
                    intra: seen,
                    anchor,
                } => {
                    if intra {
                        *seen += 1;
                        if *seen == 2 {
                            *anchor = Some(poc);
                        }
                    }
                    match *anchor {
                        None => true,
                        Some(at) if poc < at => true,
                        Some(at) => {
                            if poc > at {
                                self.recovery = Recovery::Done;
                            }
                            false
                        }
                    }
                }
                _ => false,
            };
            session.pool.hide(store.slot, hide);
        }
        Ok(Some((
            self.order.dpb.stores[at].slot,
            parameters,
            weights(&sps, &pps),
            used,
        )))
    }

    fn picture_parameters(
        &mut self,
        pool: &Pool,
        header: &Header,
        sps: &Sps,
        pps: &Pps,
        at: usize,
    ) -> (PicParamsH264, Vec<usize>) {
        let store = &self.order.dpb.stores[at];
        let mut parameters = PicParamsH264 {
            width_in_mbs_minus1: (sps.mb_width.max(1) - 1) as u16,
            // The height of the frame, even for a field (clause 7.4.2.1.1).
            height_in_mbs_minus1: (sps.mb_height().max(1) - 1) as u16,
            current: pool.index(store.slot).map_or(PicEntry::NONE, |index| {
                PicEntry::new(index, header.field_pic && header.bottom_field)
            }),
            num_ref_frames: sps.max_ref_frames as u8,
            bit_fields: PicParamsH264::fields(
                header.field_pic,
                sps.mb_adaptive && !header.field_pic,
                sps.chroma,
                header.nal_ref_idc != 0,
                pps.constrained_intra,
                pps.weighted_pred,
                pps.weighted_bipred,
                sps.frame_mbs_only,
                pps.transform_8x8,
                // MinLumaBiPredSize8x8 (clause A.3.3.2).
                sps.level >= 31,
                header.kind.intra() && header.all_same,
            ),
            bit_depth_luma_minus8: sps.depth_luma.saturating_sub(8) as u8,
            bit_depth_chroma_minus8: sps.depth_chroma.saturating_sub(8) as u8,
            status_report: {
                self.report = self.report.wrapping_add(1).max(1);
                self.report
            },
            current_order: [
                if store.has_poc[0] {
                    store.poc[0] + POC_BASE
                } else {
                    0
                },
                if store.has_poc[1] {
                    store.poc[1] + POC_BASE
                } else {
                    0
                },
            ],
            pic_init_qs_minus26: (pps.init_qs - 26) as i8,
            chroma_qp_index_offset: pps.chroma_qp_offset[0] as i8,
            second_chroma_qp_index_offset: pps.chroma_qp_offset[1] as i8,
            reserved16: 3,
            continuation: 1,
            pic_init_qp_minus26: (pps.init_qp - 26) as i8,
            num_ref_idx_l0_active_minus1: pps.num_ref_idx[0].max(1) as u8 - 1,
            num_ref_idx_l1_active_minus1: pps.num_ref_idx[1].max(1) as u8 - 1,
            frame_num: header.frame_num as u16,
            log2_max_frame_num_minus4: (sps.log2_max_frame_num.saturating_sub(4)) as u8,
            pic_order_cnt_type: sps.poc_type as u8,
            log2_max_pic_order_cnt_lsb_minus4: if sps.poc_type == 0 {
                sps.log2_max_poc_lsb.saturating_sub(4) as u8
            } else {
                0
            },
            delta_pic_order_always_zero_flag: u8::from(sps.poc_type == 1 && sps.poc_always_zero),
            direct_8x8_inference_flag: u8::from(sps.direct_8x8_inference),
            entropy_coding_mode_flag: u8::from(pps.cabac),
            pic_order_present_flag: u8::from(pps.bottom_field_order_present),
            deblocking_filter_control_present_flag: u8::from(pps.deblocking_control),
            redundant_pic_cnt_present_flag: u8::from(pps.redundant_present),
            ..PicParamsH264::default()
        };

        // Every reference, which for a second field includes the frame being decoded.
        let mut count = 0;
        let mut used = Vec::new();
        for other in &self.order.dpb.stores {
            if !other.reference() || count == 16 {
                continue;
            }
            let Some(index) = pool.index(other.slot) else {
                continue;
            };
            let long = other.marked.contains(&Marked::Long);
            parameters.reference_list[count] = PicEntry::new(index, long);
            parameters.order_list[count] = [
                if other.marked[0] != Marked::No && other.has_poc[0] {
                    other.poc[0] + POC_BASE
                } else {
                    0
                },
                if other.marked[1] != Marked::No && other.has_poc[1] {
                    other.poc[1] + POC_BASE
                } else {
                    0
                },
            ];
            parameters.frame_num_list[count] = if long {
                other.long_index as u16
            } else {
                other.frame_num as u16
            };
            for side in 0..2 {
                if other.marked[side] != Marked::No {
                    parameters.used_for_reference |= 1 << (2 * count + side);
                }
            }
            used.push(other.slot);
            count += 1;
        }
        (parameters, used)
    }
}

/// The scaling lists in bitstream scan order (`dxva.h` wants the syntax elements).
fn weights(sps: &Sps, pps: &Pps) -> QmatrixH264 {
    use crate::decode::d3d12::dxva::{ZIGZAG_4X4, ZIGZAG_8X8};

    let weights = params::weights(sps, pps);
    QmatrixH264 {
        four: std::array::from_fn(|list| {
            std::array::from_fn(|at| weights.four[list][ZIGZAG_4X4[at]])
        }),
        eight: std::array::from_fn(|list| {
            std::array::from_fn(|at| weights.eight[list][ZIGZAG_8X8[at]])
        }),
    }
}
