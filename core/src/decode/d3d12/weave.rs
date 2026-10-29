// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一枚が一フィールドのピクチャー（HEVC の `field_seq_flag`）を、二つずつ一枚のフレームに合成する。
//! ビデオプロセスはフィールドの対を受け取らないので、合成したフレームを渡してデインターレースさせる。
//! 3D のキューで、フレームの各プレーンをレンダーターゲットにして一行おきに描く（コピーのエンジンは遅い）。
//!
//! ビデオプロセスと同じく、デコードと同じスレッドで呼ぶ。

use std::sync::Arc;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SHADER_MACRO, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, PCSTR, s};

use super::Frame;
use super::dx::{self, Owned, failed};
use super::process::Deinterlacer;
use crate::decode::{About, Decoded, Error};
use mediadecoder::es::Structure;

/// 出力の上限。ビデオプロセスが参照に持つ分と、書いている途中の分があれば足りる。
const MOST_OUTS: usize = 16;

/// 頂点を持たない全面の三角形と、出力の行の偶奇でトップかボトムの同じ列を読むピクセルシェーダー。
const SHADER: &str = r"
Texture2D<T> top : register(t0);
Texture2D<T> bottom : register(t1);

float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 at = float2((id << 1) & 2, id & 2);
    return float4(at * float2(2, -2) + float2(-1, 1), 0, 1);
}

T ps(float4 at : SV_Position) : SV_Target {
    int3 from = int3(int(at.x), int(at.y) >> 1, 0);
    return (int(at.y) & 1) != 0 ? bottom.Load(from) : top.Load(from);
}
";

pub(super) struct Weaver {
    raw: ID3D12Device,
    queue: ID3D12CommandQueue,
    list: ID3D12GraphicsCommandList,
    /// リストを作ったアロケーター。リストより先に放さない。
    _made_with: ID3D12CommandAllocator,
    fence: ID3D12Fence,
    value: u64,
    event: Owned,
    root: ID3D12RootSignature,
    pipelines: Option<Pipelines>,
    /// 出力ごとに四つ（トップの輝度・ボトムの輝度・トップの色差・ボトムの色差）。
    views: ID3D12DescriptorHeap,
    view_step: usize,
    /// 出力ごとに二つ（輝度・色差）。
    targets: ID3D12DescriptorHeap,
    target_step: usize,
    outs: Vec<Out>,
    /// 対になるフィールドを待っているフィールド（トップなら真）。
    pending: Option<(Frame, About, bool)>,
}

struct Pipelines {
    ten_bit: bool,
    luma: ID3D12PipelineState,
    chroma: ID3D12PipelineState,
}

struct Out {
    texture: ID3D12Resource,
    size: [u32; 2],
    ten_bit: bool,
    allocator: ID3D12CommandAllocator,
    /// ビデオプロセスが持っている間は 2 以上。
    lease: Arc<()>,
    written: u64,
    /// 読んだフィールド。書き終えるまでデコーダーに返さない。
    holding: Vec<Frame>,
}

impl std::fmt::Debug for Weaver {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Weaver(outs: {})", self.outs.len())
    }
}

impl Weaver {
    pub fn new(raw: &ID3D12Device) -> Result<Self, Error> {
        let allocator = dx::allocator(raw, D3D12_COMMAND_LIST_TYPE_DIRECT)?;
        // SAFETY: 生きているデバイスで、記述はどれもローカル。
        unsafe {
            let queue: ID3D12CommandQueue = raw
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .map_err(|error| failed("フィールドの合成のキュー", error))?;
            let list: ID3D12GraphicsCommandList = raw
                .CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_DIRECT,
                    &allocator,
                    None::<&ID3D12PipelineState>,
                )
                .map_err(|error| failed("CreateCommandList", error))?;
            list.Close().map_err(|error| failed("Close", error))?;
            let fence = raw
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .map_err(|error| failed("CreateFence", error))?;
            let views: ID3D12DescriptorHeap = raw
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: (MOST_OUTS * 4) as u32,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
                .map_err(|error| failed("CreateDescriptorHeap", error))?;
            let targets: ID3D12DescriptorHeap = raw
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: (MOST_OUTS * 2) as u32,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                    NodeMask: 0,
                })
                .map_err(|error| failed("CreateDescriptorHeap", error))?;
            Ok(Self {
                raw: raw.clone(),
                queue,
                list,
                _made_with: allocator,
                fence,
                value: 0,
                event: Owned::event()?,
                root: root_signature(raw)?,
                pipelines: None,
                view_step: raw
                    .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                    as usize,
                views,
                target_step: raw.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV)
                    as usize,
                targets,
                outs: Vec::new(),
                pending: None,
            })
        }
    }

    /// 一フィールド受け取り、対がそろえば合成したフレームを返す。同じパリティーが続いたら先のフィールドは捨てる。
    pub fn push(&mut self, frame: &Frame, about: &About) -> Result<Option<(Frame, About)>, Error> {
        self.release_read();
        let top = match about.structure {
            Structure::Top => true,
            Structure::Bottom => false,
            Structure::Frame => self.pending.as_ref().is_none_or(|(_, _, top)| !top),
        };
        let (first, first_about, first_top) = match self.pending.take() {
            Some((first, first_about, first_top))
                if first_top != top
                    && first.width == frame.width
                    && first.height == frame.height
                    && first.ten_bit == frame.ten_bit =>
            {
                (first, first_about, first_top)
            }
            _ => {
                self.pending = Some((frame.clone(), *about, top));
                return Ok(None);
            }
        };
        let (upper, lower) = if first_top {
            (&first, frame)
        } else {
            (frame, &first)
        };
        let size = [frame.width, frame.height * 2];
        self.prepare(frame.ten_bit)?;
        let at = self.out_for(size, frame.ten_bit)?;
        self.draw(upper, lower, at)?;
        let out = &mut self.outs[at];
        out.holding = vec![first.clone(), frame.clone()];
        let woven = Frame {
            device: self.raw.clone(),
            texture: out.texture.clone(),
            width: size[0],
            height: size[1],
            ten_bit: frame.ten_bit,
            lease: Some(out.lease.clone()),
            // ビデオプロセスが自分のキューで書き終わりを待つ。
            ready: Some((self.fence.clone(), out.written)),
        };
        let about = About {
            format: crate::decode::weave::woven(first_about.format),
            structure: Structure::Frame,
            top_field_first: first_top,
            repeat_first_field: false,
            progressive: false,
            ..first_about
        };
        Ok(Some((woven, about)))
    }

    /// 前の番組のフィールドを捨てる。書き終えるのを待って、読んでいたフィールドもデコーダーに返す。
    pub fn reset(&mut self) {
        self.pending = None;
        if self.event.wait_fence(&self.fence, self.value).is_ok() {
            for out in &mut self.outs {
                out.holding.clear();
            }
        }
    }

    fn release_read(&mut self) {
        let done = dx::completed(&self.fence);
        for out in &mut self.outs {
            if out.written <= done {
                out.holding.clear();
            }
        }
    }

    /// ビット深度に合うパイプラインを用意する。
    fn prepare(&mut self, ten_bit: bool) -> Result<(), Error> {
        if self
            .pipelines
            .as_ref()
            .is_some_and(|made| made.ten_bit == ten_bit)
        {
            return Ok(());
        }
        let (luma, chroma) = plane_formats(ten_bit);
        let vs = compile(s!("vs"), s!("vs_5_0"), s!("float"))?;
        let made = Pipelines {
            ten_bit,
            luma: pipeline(
                &self.raw,
                &self.root,
                &vs,
                &compile(s!("ps"), s!("ps_5_0"), s!("float"))?,
                luma,
            )?,
            chroma: pipeline(
                &self.raw,
                &self.root,
                &vs,
                &compile(s!("ps"), s!("ps_5_0"), s!("float2"))?,
                chroma,
            )?,
        };
        self.pipelines = Some(made);
        Ok(())
    }

    /// 空いている出力。全部持たれていれば増やす。
    fn out_for(&mut self, size: [u32; 2], ten_bit: bool) -> Result<usize, Error> {
        // 大きさか形式の違う出力は、空いたものから捨てる。
        self.outs.retain(|out| {
            (out.size == size && out.ten_bit == ten_bit) || Arc::strong_count(&out.lease) > 1
        });
        if let Some(at) = self.outs.iter().position(|out| {
            out.size == size && out.ten_bit == ten_bit && Arc::strong_count(&out.lease) == 1
        }) {
            self.event.wait_fence(&self.fence, self.outs[at].written)?;
            self.outs[at].holding.clear();
            return Ok(at);
        }
        if self.outs.len() >= MOST_OUTS {
            return Err(Error::Unavailable(String::from(
                "フィールドの合成の出力が足りない",
            )));
        }
        let format = if ten_bit {
            DXGI_FORMAT_P010
        } else {
            DXGI_FORMAT_NV12
        };
        self.outs.push(Out {
            texture: target(&self.raw, format, size)?,
            size,
            ten_bit,
            allocator: dx::allocator(&self.raw, D3D12_COMMAND_LIST_TYPE_DIRECT)?,
            lease: Arc::new(()),
            written: 0,
            holding: Vec::new(),
        });
        Ok(self.outs.len() - 1)
    }

    fn draw(&mut self, upper: &Frame, lower: &Frame, at: usize) -> Result<(), Error> {
        let Some(pipelines) = self.pipelines.as_ref() else {
            return Ok(());
        };
        let (luma, chroma) = plane_formats(upper.ten_bit);
        let out = &self.outs[at];
        let [width, height] = out.size;
        // SAFETY: 記述子はこの出力の分だけを書き換え、その出力の前の描画は `out_for` が待った。
        // 記録するリソースはどれも書き終えるまで `holding` と `outs` が持つ。
        unsafe {
            let views =
                self.views.GetCPUDescriptorHandleForHeapStart().ptr + at * 4 * self.view_step;
            for (slot, (frame, plane, format)) in [
                (upper, 0, luma),
                (lower, 0, luma),
                (upper, 1, chroma),
                (lower, 1, chroma),
            ]
            .into_iter()
            .enumerate()
            {
                self.raw.CreateShaderResourceView(
                    &frame.texture,
                    Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                        Format: format,
                        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                            Texture2D: D3D12_TEX2D_SRV {
                                MostDetailedMip: 0,
                                MipLevels: 1,
                                PlaneSlice: plane,
                                ResourceMinLODClamp: 0.0,
                            },
                        },
                    }),
                    D3D12_CPU_DESCRIPTOR_HANDLE {
                        ptr: views + slot * self.view_step,
                    },
                );
            }
            let targets =
                self.targets.GetCPUDescriptorHandleForHeapStart().ptr + at * 2 * self.target_step;
            let target_of = |plane: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: targets + plane * self.target_step,
            };
            for (plane, format) in [(0, luma), (1, chroma)] {
                self.raw.CreateRenderTargetView(
                    &out.texture,
                    Some(&D3D12_RENDER_TARGET_VIEW_DESC {
                        Format: format,
                        ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                        Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 {
                            Texture2D: D3D12_TEX2D_RTV {
                                MipSlice: 0,
                                PlaneSlice: plane as u32,
                            },
                        },
                    }),
                    target_of(plane),
                );
            }
            let tables = self.views.GetGPUDescriptorHandleForHeapStart().ptr
                + (at * 4 * self.view_step) as u64;

            out.allocator
                .Reset()
                .map_err(|error| failed("Reset", error))?;
            self.list
                .Reset(&out.allocator, None::<&ID3D12PipelineState>)
                .map_err(|error| failed("Reset", error))?;
            // フィールド（デコーダーの同時アクセスのテクスチャ）にはバリアを打たず、COMMON からの暗黙の昇格で読む。
            self.list.ResourceBarrier(&[dx::transition(
                &out.texture,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )]);
            self.list.SetGraphicsRootSignature(&self.root);
            self.list.SetDescriptorHeaps(&[Some(self.views.clone())]);
            self.list
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            for (plane, pipeline, [wide, tall]) in [
                (0, &pipelines.luma, [width, height]),
                (1, &pipelines.chroma, [width / 2, height / 2]),
            ] {
                self.list.SetPipelineState(pipeline);
                let target = target_of(plane);
                self.list.OMSetRenderTargets(1, Some(&target), false, None);
                self.list.RSSetViewports(&[D3D12_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: wide as f32,
                    Height: tall as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                }]);
                self.list.RSSetScissorRects(&[RECT {
                    left: 0,
                    top: 0,
                    right: wide as i32,
                    bottom: tall as i32,
                }]);
                self.list.SetGraphicsRootDescriptorTable(
                    0,
                    D3D12_GPU_DESCRIPTOR_HANDLE {
                        ptr: tables + (plane * 2 * self.view_step) as u64,
                    },
                );
                self.list.DrawInstanced(3, 1, 0, 0);
            }
            self.list.ResourceBarrier(&[dx::transition(
                &out.texture,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_COMMON,
            )]);
            self.list.Close().map_err(|error| failed("Close", error))?;
            let list: ID3D12CommandList = self
                .list
                .cast()
                .map_err(|error| failed("ID3D12CommandList", error))?;
            for (fence, value) in [upper, lower]
                .iter()
                .filter_map(|frame| frame.ready.as_ref())
            {
                self.queue
                    .Wait(fence, *value)
                    .map_err(|error| failed("Wait", error))?;
            }
            self.queue.ExecuteCommandLists(&[Some(list)]);
            self.value += 1;
            self.queue
                .Signal(&self.fence, self.value)
                .map_err(|error| failed("Signal", error))?;
        }
        self.outs[at].written = self.value;
        Ok(())
    }
}

/// フィールドを合成してからビデオプロセスでデインターレースする（QSV のデコーダーのフィールド）。
#[derive(Debug)]
pub(crate) struct Fields {
    weaver: Weaver,
    deinterlacer: Deinterlacer,
}

impl Fields {
    pub fn new(raw: &ID3D12Device, bob: bool) -> Result<Self, Error> {
        Ok(Self {
            weaver: Weaver::new(raw)?,
            deinterlacer: Deinterlacer::new(raw, bob)?,
        })
    }

    /// 一フィールド受け取り、出せるようになったフレームを返す。
    pub fn push(&mut self, frame: &Frame, about: &About) -> Result<Vec<Decoded>, Error> {
        match self.weaver.push(frame, about)? {
            Some((woven, about)) => {
                let size = [woven.width, woven.height];
                self.deinterlacer.push(woven, about, size)
            }
            None => Ok(Vec::new()),
        }
    }

    /// 残りを出す。対のそろわないフィールドは捨てる。
    pub fn flush(&mut self) -> Vec<Decoded> {
        self.deinterlacer.flush()
    }

    pub fn reset(&mut self) {
        self.weaver.reset();
        self.deinterlacer.reset();
    }
}

impl Drop for Weaver {
    fn drop(&mut self) {
        let _ = self.event.wait_fence(&self.fence, self.value);
    }
}

/// 輝度と色差のプレーンの形式。
fn plane_formats(ten_bit: bool) -> (DXGI_FORMAT, DXGI_FORMAT) {
    if ten_bit {
        (DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM)
    } else {
        (DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM)
    }
}

/// 描き込めるフレーム。ビデオプロセスはデコーダーのテクスチャと同じく COMMON から読む。
fn target(
    raw: &ID3D12Device,
    format: DXGI_FORMAT,
    size: [u32; 2],
) -> Result<ID3D12Resource, Error> {
    let properties = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(size[0]),
        Height: size[1],
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
            | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    };
    let mut texture: Option<ID3D12Resource> = None;
    // SAFETY: 記述はどれもローカルで、`texture` には一度だけ書かれる。
    unsafe {
        raw.CreateCommittedResource(
            &properties,
            D3D12_HEAP_FLAG_NONE,
            &description,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut texture,
        )
    }
    .map_err(|error| failed("フィールドの合成の出力", error))?;
    texture.ok_or_else(|| Error::Unavailable(String::from("フィールドの合成の出力が作れない")))
}

/// SRV 二つ（t0・t1）の表が一つだけのルートシグネチャー。
fn root_signature(raw: &ID3D12Device) -> Result<ID3D12RootSignature, Error> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 2,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let parameter = D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: 1,
                pDescriptorRanges: &range,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let description = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 1,
        pParameters: &parameter,
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let mut blob: Option<ID3DBlob> = None;
    // SAFETY: 記述が指す範囲と引数はこの関数の間生きている。
    unsafe {
        D3D12SerializeRootSignature(&description, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)
            .map_err(|error| failed("D3D12SerializeRootSignature", error))?;
        let blob =
            blob.ok_or_else(|| Error::Unavailable(String::from("ルートシグネチャーが空")))?;
        raw.CreateRootSignature(0, bytes(&blob))
            .map_err(|error| failed("CreateRootSignature", error))
    }
}

fn compile(entry: PCSTR, profile: PCSTR, kind: PCSTR) -> Result<ID3DBlob, Error> {
    let defines = [
        D3D_SHADER_MACRO {
            Name: s!("T"),
            Definition: kind,
        },
        D3D_SHADER_MACRO::default(),
    ];
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: ソースと定義はこの関数の間生きている。
    let made = unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            s!("weave"),
            Some(defines.as_ptr()),
            None,
            entry,
            profile,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(error) = made {
        let said = errors
            // SAFETY: コンパイラーが返したブロブ。
            .map(|blob| String::from_utf8_lossy(unsafe { bytes(&blob) }).into_owned())
            .unwrap_or_default();
        return Err(Error::Unavailable(format!("D3DCompile: {error} {said}")));
    }
    code.ok_or_else(|| Error::Unavailable(String::from("シェーダーが空")))
}

fn pipeline(
    raw: &ID3D12Device,
    root: &ID3D12RootSignature,
    vs: &ID3DBlob,
    ps: &ID3DBlob,
    format: DXGI_FORMAT,
) -> Result<ID3D12PipelineState, Error> {
    let shader = |blob: &ID3DBlob| D3D12_SHADER_BYTECODE {
        // SAFETY: ブロブはこの関数の間生きている。
        pShaderBytecode: unsafe { blob.GetBufferPointer() },
        // SAFETY: 同上。
        BytecodeLength: unsafe { blob.GetBufferSize() },
    };
    // 混ぜずにそのまま書く。
    let mut blend = D3D12_BLEND_DESC::default();
    blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: false.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ZERO,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
    formats[0] = format;
    let description = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: dx::borrow(root),
        VS: shader(vs),
        PS: shader(ps),
        BlendState: blend,
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            DepthClipEnable: true.into(),
            ..Default::default()
        },
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        RTVFormats: formats,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        ..Default::default()
    };
    // SAFETY: 記述が指すものはどれもこの関数の間生きている。
    unsafe { raw.CreateGraphicsPipelineState(&description) }
        .map_err(|error| failed("CreateGraphicsPipelineState", error))
}

/// ブロブの中身。
///
/// # Safety
/// ブロブが生きている間だけ使う。
unsafe fn bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: ブロブの指す範囲をそのまま返す。
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}
