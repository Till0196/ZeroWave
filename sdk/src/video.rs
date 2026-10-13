// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映像フィルター（`video` hook）のデータ型。プラグインはグラフ（[`Graph`]）を返し、ピクセルは本体の GPU がレンダリングする。
//! バインディング：`@group(0)` の 0 が `zw`、`1 + 2i`／`2 + 2i` が `reads[i]`、compute は `1 + 2n` が書く先。
//! Deinterlace は `@group(1)`、[`Pass::buffers`] は `@group(2)`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod process;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum At {
    #[default]
    Picture,
    /// 本体のデインタレースの代わり。置けるのは一つだけ。
    Deinterlace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "picture_only")]
    pub at: Vec<At>,
    /// 真なら番組が変わるたびに [`crate::plug::Video::key`] を問い合わせる。
    #[serde(default)]
    pub program: bool,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            at: picture_only(),
            program: false,
        }
    }
}

fn picture_only() -> Vec<At> {
    vec![At::Picture]
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    /// ARIB のジャンル（大分類、中分類）。
    #[serde(default)]
    pub genres: Vec<(u8, u8)>,
    #[serde(default)]
    pub network: u16,
    #[serde(default)]
    pub service: u16,
    /// 設定の `[[network]]` の名前。どれにも入らなければ空。
    #[serde(default)]
    pub band: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub recording: bool,
    #[serde(default)]
    pub structure: Structure,
    /// 1970-01-01 0時からの分（日本時間の壁時計）。
    #[serde(default)]
    pub minute: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Structure {
    #[default]
    Unknown,
    Frame,
    Field,
}

impl Program {
    pub fn minute_of_day(&self) -> u16 {
        self.minute.rem_euclid(1440) as u16
    }

    pub fn weekday(&self) -> u8 {
        (self.minute.div_euclid(1440) + 3).rem_euclid(7) as u8
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Env {
    pub at: At,
    pub format: Format,
    /// チェーンの二つ目からは前のフィルターの出力のサイズ。
    pub input: [u32; 2],
    /// レターボックスの内側の大きさ（画素）。
    pub target: [u32; 2],
    pub rate: [u32; 2],
    #[serde(default)]
    pub display: f64,
    pub device: Device,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    #[serde(default)]
    pub program: Option<Program>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    Building {
        total: u32,
    },
    Progress {
        done: u32,
        total: u32,
    },
    Built {
        seconds: f64,
    },
    Paused {
        at: At,
        ms: f64,
        budget: f64,
    },
    Failed {
        at: At,
        reason: String,
    },
    /// バックエンドが準備している間。`progress` は 0〜1、準備の間はバックエンドを使わずにレンダリングする。
    Preparing {
        #[serde(default)]
        progress: Option<f32>,
        #[serde(default)]
        said: String,
    },
    Prepared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub interlaced: bool,
    pub aspect: [u32; 2],
    pub transfer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Device {
    pub backend: String,
    pub compute: bool,
    pub float32_filterable: bool,
    pub max_texture: u32,
    #[serde(default)]
    pub host: Vec<String>,
    #[serde(default)]
    pub flows: Vec<FlowCaps>,
    #[serde(default)]
    pub shader_f16: bool,
    #[serde(default)]
    pub matrices: Vec<Matrix>,
    #[serde(default)]
    pub limits: Limits,
}

/// 協調行列のサイズ：`m×k` と `k×n` の積を `m×n` に足す。型は `f16`・`f32`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Matrix {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub ab: String,
    pub cr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub buffer: u64,
    pub storage_binding: u64,
    pub storage_buffers: u32,
    pub uniform_align: u32,
    pub storage_align: u32,
    pub workgroup_storage: u32,
    pub invocations: u32,
    pub groups: u32,
    pub subgroup: [u32; 2],
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            buffer: 256 << 20,
            storage_binding: 128 << 20,
            storage_buffers: 8,
            uniform_align: 256,
            storage_align: 256,
            workgroup_storage: 16 << 10,
            invocations: 256,
            groups: 65535,
            subgroup: [0, 0],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowCaps {
    pub who: String,
    /// 細かい順。出力先をフレームの 1/g（切り上げ）にすればグリッド g。
    pub grids: Vec<u32>,
    #[serde(default)]
    pub confidence: bool,
}

impl Device {
    pub fn flow_caps(&self) -> Option<&FlowCaps> {
        let who = self.host_for(HOST_FLOW)?;
        self.flows.iter().find(|caps| caps.who == who)
    }

    pub fn host_for(&self, job: &str) -> Option<&str> {
        self.host.iter().find_map(|entry| {
            entry
                .split_once('/')
                .filter(|(j, _)| *j == job)
                .map(|(_, who)| who)
        })
    }
}

/// [`Kind::Host`] の処理：前と今の間の `Zw::phase` のフレーム。バックエンドによっては 1〜2 入力遅れる（[`Pass::held`]）。
pub const HOST_INTERPOLATE: &str = "interpolate";

pub const HOST_PROCESS: &str = "process";

/// [`Kind::Host`] の処理：前と今の間のオプティカルフロー（`rgba16float`、フレームの 1/g、ピクセル単位）。
pub const HOST_FLOW: &str = "flow";

/// [`HOST_FLOW`] の確からしさ（`rg16float`、0〜1）。同じ入力の [`HOST_FLOW`] より後ろに置く。
pub const HOST_FLOW_CONFIDENCE: &str = "flow-confidence";

pub const HOST_FLOW_GRID: u32 = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Graph {
    pub output: [u32; 2],
    /// 0 は 1 と同じ、多くて [`MOST_OUTPUTS`]。
    #[serde(default)]
    pub outputs: u32,
    #[serde(default)]
    pub history: u32,
    /// 目標の fps へフレームレート変換する（0 なら無し）。`Zw::phase` に位置を入れる。`outputs` より先。
    #[serde(default)]
    pub retime: f64,
    /// フレームレート変換のとき、このパスから先だけを出力ごとにレンダリングする。0 なら全部。
    #[serde(default)]
    pub retime_from: usize,
    /// 先に生成してバッファに貯める長さ（ミリ秒）。遅延と引き換えに処理時間の上限がフレーム間隔の 85% になる。
    #[serde(default)]
    pub buffer_ms: u32,
    /// [`At::Deinterlace`] だけ：読むフレームの幅（前、後）。
    #[serde(default = "default_frames")]
    pub frames: [i32; 2],
    #[serde(default)]
    pub textures: BTreeMap<String, Texture>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buffers: BTreeMap<String, Buffer>,
    /// 一つも無ければ素通し。多くて [`MOST_PASSES`]。
    #[serde(default)]
    pub passes: Vec<Pass>,
    #[serde(default)]
    pub keep: bool,
    /// [`Buffer::blob`] の内容。JSON には載らず、別の API で本体へ渡る。
    #[serde(skip)]
    pub blobs: BTreeMap<String, Blob>,
}

pub const MOST_PASSES: usize = 1024;

pub const BUFFER_GROUP: u32 = 2;

/// 大きなバイト列（NN の重みなど）。書き出すときは長さだけ書く。
#[derive(Clone, PartialEq, Default)]
pub struct Blob(pub std::sync::Arc<Vec<u8>>);

impl Blob {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(std::sync::Arc::new(bytes))
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Blob({} バイト)", self.0.len())
    }
}

/// 内容は `data` か `blob` から一度だけアップロードし、無ければ 0。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Buffer {
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// [`Graph::blobs`] の一つの、この位置から `size` バイト。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobAt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BlobAt {
    pub name: String,
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Bind {
    pub binding: u32,
    pub name: String,
    #[serde(default)]
    pub access: Access,
    /// `size` が 0 なら終わりまで。位置は [`Limits`] の揃えの倍数。
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    #[default]
    Read,
    ReadWrite,
    Uniform,
}

fn default_frames() -> [i32; 2] {
    [-1, 1]
}

pub const MOST_OUTPUTS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Texture {
    pub size: Vec<u32>,
    /// 既定は `rgba16float`。mpv の綴り（`rgba16f` など）も読む。
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "yes")]
    pub linear: bool,
    #[serde(default)]
    pub keep: bool,
    #[serde(default)]
    pub previous: bool,
    /// その形式のままのバイト列の十六進。一度だけアップロードし、書き込めない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

fn default_format() -> String {
    String::from("rgba16float")
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Fragment,
    Compute,
    /// 本体が受け持つ。`source` は処理の名前（`interpolate/amf` のようにバックエンドも指定できる）。
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pass {
    pub name: String,
    #[serde(default)]
    pub kind: Kind,
    pub source: String,
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub reads: Vec<String>,
    pub writes: String,
    #[serde(default)]
    pub block: [u32; 2],
    /// compute だけ：ワークグループの数。0 でなければ `block` より先。続けば一つの compute パスに積む。
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub groups: [u32; 3],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buffers: Vec<Bind>,
    /// ワークグループのメモリーを書いてから読むシェーダーなら、偽にすると速い。
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub zero_workgroup: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<Process>,
    /// 本体が設定から読んだ値。guest からは書けない。
    #[serde(skip)]
    pub launch: Option<Launch>,
    /// 完了を待たずに要求するバックエンドだけ：何入力遅らせて出すか（1 か 2、0 は本体に任せる）。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub held: u32,
}

/// 本体が起動する外部プロセス。パスは持たず、設定のキーの名前を指定し、パスは本体がユーザーの値から取得する。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Process {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ways: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub dir: String,
    pub path: Vec<String>,
    pub ways: Vec<String>,
}

fn default_entry() -> String {
    String::from("main")
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_zero3(v: &[u32; 3]) -> bool {
    *v == [0; 3]
}

impl Default for Pass {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: Kind::default(),
            source: String::new(),
            entry: default_entry(),
            reads: Vec::new(),
            writes: String::new(),
            block: [0, 0],
            groups: [0; 3],
            buffers: Vec::new(),
            zero_workgroup: true,
            process: None,
            launch: None,
            held: 0,
        }
    }
}

impl Pass {
    pub fn dispatches(&self) -> bool {
        self.kind == Kind::Compute && self.groups != [0; 3]
    }
}

/// 本体が毎フレーム書く uniform（binding 0）。[`Zw::bytes`] とこの WGSL の文字列が同じ並び。
pub const ZW_WGSL: &str = "struct Zw {
    // このフィルターの入力の大きさ（画素）。
    input_size: vec2<f32>,
    // このパスが書く先の大きさ（画素）。
    output_size: vec2<f32>,
    // 何枚目の出力か（読み込んだときから数える）。
    frame: u32,
    // フレーム補間の位相：outputs が n なら 0, 1/n, …。
    phase: f32,
    // どちらのフィールドを見せるか：0 トップ、1 ボトム、2 フレーム（分けない）。
    field: u32,
    // 見せるフィールドが持つ最初の行（0 か 1）。
    first_row: u32,
    // 逆テレシネで復元したフレームの、前のフレームから取る行の偶奇（2 は無し）。
    mate_row: u32,
    // フィールドマッチでレンダリングする出力か（0 か 1）。
    matched: u32,
    // 0..1 の乱数（フレームごと）。
    random: f32,
    // 一つの入力から出す枚数。
    outputs: u32,
    // そのフレームの二つ目のフィールドか（0 か 1）。一つ目なら、時間で一つ前の
    // フィールドは前のフレーム（-1）に、一つ後は同じフレームにある。二つ目なら、
    // 一つ前は同じフレーム、一つ後は次のフレーム（1）にある。
    second: u32,
    // 予約（構造体を十六バイトの倍数に）。
    reserved0: u32,
    reserved1: u32,
    reserved2: u32,
};
";

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Zw {
    pub input_size: [f32; 2],
    pub output_size: [f32; 2],
    pub frame: u32,
    pub phase: f32,
    pub field: u32,
    pub first_row: u32,
    pub mate_row: u32,
    pub matched: u32,
    pub random: f32,
    pub outputs: u32,
    pub second: u32,
}

impl Zw {
    pub const SIZE: usize = 64;

    pub fn bytes(&self) -> [u8; Self::SIZE] {
        let words: [[u8; 4]; 13] = [
            self.input_size[0].to_le_bytes(),
            self.input_size[1].to_le_bytes(),
            self.output_size[0].to_le_bytes(),
            self.output_size[1].to_le_bytes(),
            self.frame.to_le_bytes(),
            self.phase.to_le_bytes(),
            self.field.to_le_bytes(),
            self.first_row.to_le_bytes(),
            self.mate_row.to_le_bytes(),
            self.matched.to_le_bytes(),
            self.random.to_le_bytes(),
            self.outputs.to_le_bytes(),
            self.second.to_le_bytes(),
        ];
        let mut out = [0u8; Self::SIZE];
        for (at, word) in words.iter().enumerate() {
            out[at * 4..at * 4 + 4].copy_from_slice(word);
        }
        out
    }
}

pub const DEINTERLACE_PRELUDE: &str = "
// 読めるフレームの幅（前、後）。Graph::frames と同じ。
fn zw_frames() -> vec2<i32>
// 輝度の大きさと、色差の大きさ（4:2:0 なので半分）。
fn zw_size() -> vec2<i32>
fn zw_chroma_size() -> vec2<i32>
// フレーム frame（0 が今、-1 が前、1 が後）の、その位置の符号値（0..1）。
// 縁を越えた位置は縁の値。色差は色差の座標で。
fn zw_luma(frame: i32, p: vec2<i32>) -> f32
fn zw_cb(frame: i32, p: vec2<i32>) -> f32
fn zw_cr(frame: i32, p: vec2<i32>) -> f32
// 符号値を RGB（放送の伝達特性のまま、0..1）に。行列とレンジは本体が知っている。
fn zw_rgb(y: f32, cb: f32, cr: f32) -> vec3<f32>
";

pub fn format_name(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "rgba16float" | "rgba16f" | "rgba16hf" => "rgba16float",
        "rgba32float" | "rgba32f" => "rgba32float",
        "rgba8unorm" | "rgba8" => "rgba8unorm",
        "r16float" | "r16f" | "r16hf" => "r16float",
        "r32float" | "r32f" => "r32float",
        "rg16float" | "rg16f" | "rg16hf" => "rg16float",
        "rg32float" | "rg32f" => "rg32float",
        "r8unorm" | "r8" => "r8unorm",
        "rg8unorm" | "rg8" => "rg8unorm",
        _ => return None,
    })
}

pub fn texel_bytes(format: &str) -> Option<usize> {
    Some(match format_name(format)? {
        "rgba16float" => 8,
        "rgba32float" => 16,
        "rgba8unorm" => 4,
        "r16float" => 2,
        "r32float" => 4,
        "rg16float" => 4,
        "rg32float" => 8,
        "r8unorm" => 1,
        "rg8unorm" => 2,
        _ => return None,
    })
}

pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .map(|b| (b as char).to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    Some(
        digits
            .chunks(2)
            .map(|pair| pair[0] << 4 | pair[1])
            .collect(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source<'a> {
    Picture(u32),
    Output(u32),
    Named(&'a str),
}

impl<'a> Source<'a> {
    pub fn of(name: &'a str) -> Self {
        let back = |rest: &str| rest.parse::<u32>().ok();
        if name == "picture" {
            return Self::Picture(0);
        }
        if let Some(k) = name.strip_prefix("picture:").and_then(back) {
            return Self::Picture(k);
        }
        if let Some(k) = name.strip_prefix("output:").and_then(back)
            && k > 0
        {
            return Self::Output(k);
        }
        Self::Named(name)
    }
}

pub fn ident(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

impl Graph {
    pub fn outputs(&self) -> u32 {
        if self.retime > 0.0 {
            return 1;
        }
        self.outputs.clamp(1, MOST_OUTPUTS)
    }

    pub fn retime(&self) -> Option<f64> {
        (self.retime.is_finite() && self.retime > 0.0).then_some(self.retime)
    }

    pub fn check(&self, at: At) -> Result<(), String> {
        if self.passes.is_empty() {
            return Ok(());
        }
        if self.output[0] == 0 || self.output[1] == 0 {
            return Err(String::from("出力の大きさが 0 です。"));
        }
        if self.passes.len() > MOST_PASSES {
            return Err(format!(
                "パスが多すぎます（{}、多くて {MOST_PASSES}）。",
                self.passes.len()
            ));
        }
        self.check_buffers()?;
        if !matches!(self.frames, [lo, hi] if (-2..=0).contains(&lo) && (0..=2).contains(&hi)) {
            return Err(format!(
                "frames の値が範囲外です（-2〜0、0〜2）：{:?}",
                self.frames
            ));
        }
        if at == At::Picture && self.frames != default_frames() {
            return Err(String::from("frames はデインターレースでだけ使えます。"));
        }
        for (name, texture) in &self.textures {
            if format_name(&texture.format).is_none() {
                return Err(format!("{name}：{} には対応していません。", texture.format));
            }
            if texture.size.is_empty() || texture.size.len() > 3 || texture.size.contains(&0) {
                return Err(format!(
                    "{name}：大きさが正しくありません（{:?}）。",
                    texture.size
                ));
            }
            if let Some(hex) = &texture.data {
                let texels: usize = texture.size.iter().map(|s| *s as usize).product();
                let want = texels * texel_bytes(&texture.format).unwrap_or(0) * 2;
                let have = hex.bytes().filter(|b| !b.is_ascii_whitespace()).count();
                if have != want {
                    return Err(format!(
                        "{name}：データの長さが合いません（{} バイト、{:?} の {} なら {} バイト）。",
                        have / 2,
                        texture.size,
                        texture.format,
                        want / 2
                    ));
                }
            }
            if texture.data.is_none() && texture.size.len() != 2 {
                return Err(format!(
                    "{name}：データの無いテクスチャは 2D だけ使えます。"
                ));
            }
            if matches!(name.as_str(), "output" | "picture") || name.contains(':') {
                return Err(format!("{name}：この名前は予約されているため使えません。"));
            }
        }
        let mut written: Vec<&str> = Vec::new();
        for pass in &self.passes {
            for read in &pass.reads {
                match Source::of(read) {
                    Source::Picture(k) | Source::Output(k) if k > self.history => {
                        return Err(format!(
                            "{}：{read} は history（{}）より前のフレームです。",
                            pass.name, self.history
                        ));
                    }
                    Source::Named(name) => {
                        if let Some(base) = name.strip_suffix(":1") {
                            if !self.textures.get(base).is_some_and(|t| t.previous) {
                                return Err(format!(
                                    "{}：{read} は previous ではありません。",
                                    pass.name
                                ));
                            }
                            continue;
                        }
                        let texture = self.textures.get(name);
                        if texture.is_none() && !written.contains(&name) {
                            return Err(format!("{}：{read} はまだ作られていません。", pass.name));
                        }
                    }
                    _ => {}
                }
                if *read == pass.writes {
                    return Err(format!(
                        "{}：読み込み先と書き込み先が同じです（{read}）。",
                        pass.name
                    ));
                }
            }
            if pass.writes.is_empty() && !pass.dispatches() {
                return Err(format!(
                    "{}：書き込み先がありません（バッファだけに書くなら compute で groups を指定してください）。",
                    pass.name
                ));
            }
            if !pass.buffers.is_empty() && pass.kind != Kind::Compute {
                return Err(format!(
                    "{}：バッファをバインドできるのは compute だけです。",
                    pass.name
                ));
            }
            if pass.writes != "output" && !pass.writes.is_empty() {
                match self.textures.get(&pass.writes) {
                    None => {
                        return Err(format!(
                            "{}：書き込み先の {} がありません。",
                            pass.name, pass.writes
                        ));
                    }
                    Some(texture) if texture.data.is_some() => {
                        return Err(format!(
                            "{}：データを持つ {} には書き込めません。",
                            pass.name, pass.writes
                        ));
                    }
                    Some(_) => {}
                }
            }
            if pass.kind == Kind::Host {
                let job = pass
                    .source
                    .split_once('/')
                    .map_or(pass.source.as_str(), |(job, _)| job);
                if ![HOST_INTERPOLATE, HOST_FLOW, HOST_FLOW_CONFIDENCE].contains(&job) {
                    return Err(format!(
                        "{}：{} はこのバージョンでは使えません。",
                        pass.name, pass.source
                    ));
                }
                if pass.source == format!("{HOST_INTERPOLATE}/{HOST_PROCESS}")
                    && pass.process.as_ref().is_none_or(|p| p.program.is_empty())
                {
                    return Err(format!("{}：起動する実行ファイルがありません。", pass.name));
                }
                if pass.reads.len() != 2 {
                    return Err(format!(
                        "{}：前と今の二枚のフレームを読む必要があります。",
                        pass.name
                    ));
                }
            }
            if pass.kind == Kind::Compute && !pass.dispatches() && pass.block.contains(&0) {
                return Err(format!("{}：compute の block が 0 です。", pass.name));
            }
            written.push(&pass.writes);
        }
        if self
            .passes
            .last()
            .is_some_and(|pass| pass.writes != "output")
        {
            return Err(String::from("最後のパスが output に書き込んでいません。"));
        }
        Ok(())
    }

    fn check_buffers(&self) -> Result<(), String> {
        for (name, buffer) in &self.buffers {
            if self.textures.contains_key(name) {
                return Err(format!("{name}：テクスチャと同じ名前です。"));
            }
            if buffer.size == 0 || !buffer.size.is_multiple_of(4) {
                return Err(format!(
                    "{name}：大きさは 4 の倍数のバイトにしてください（{}）。",
                    buffer.size
                ));
            }
            if buffer.data.is_some() && buffer.blob.is_some() {
                return Err(format!("{name}：内容は data か blob のどちらかです。"));
            }
            if let Some(hex) = &buffer.data {
                let have = hex.bytes().filter(|b| !b.is_ascii_whitespace()).count() as u64;
                if have > buffer.size * 2 {
                    return Err(format!(
                        "{name}：データが大きさを超えています（{} バイト）。",
                        have / 2
                    ));
                }
            }
            if let Some(at) = &buffer.blob
                && !self.blobs.is_empty()
            {
                let blob = self
                    .blobs
                    .get(&at.name)
                    .ok_or_else(|| format!("{name}：blob {} がありません。", at.name))?;
                if at.offset > blob.0.len() as u64 {
                    return Err(format!(
                        "{name}：blob {} の範囲外です（{}、長さ {}）。",
                        at.name,
                        at.offset,
                        blob.0.len()
                    ));
                }
            }
        }
        for pass in &self.passes {
            let mut seen: Vec<u32> = Vec::new();
            for bind in &pass.buffers {
                let buffer = self.buffers.get(&bind.name).ok_or_else(|| {
                    format!("{}：バッファ {} がありません。", pass.name, bind.name)
                })?;
                if seen.contains(&bind.binding) {
                    return Err(format!(
                        "{}：binding {} が重なっています。",
                        pass.name, bind.binding
                    ));
                }
                seen.push(bind.binding);
                let end = bind.offset.saturating_add(bind.size);
                if bind.offset >= buffer.size || end > buffer.size || !bind.size.is_multiple_of(4) {
                    return Err(format!(
                        "{}：{} の範囲が外れています（{}+{}、大きさ {}）。",
                        pass.name, bind.name, bind.offset, bind.size, buffer.size
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

pub fn from_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(name: &str, reads: &[&str], writes: &str) -> Pass {
        Pass {
            name: name.into(),
            kind: Kind::Fragment,
            source: String::new(),
            entry: "main".into(),
            reads: reads.iter().map(|s| s.to_string()).collect(),
            writes: writes.into(),
            ..Pass::default()
        }
    }

    fn graph(passes: Vec<Pass>) -> Graph {
        Graph {
            output: [1920, 1080],
            outputs: 1,
            history: 1,
            retime: 0.0,
            retime_from: 0,
            buffer_ms: 0,
            keep: false,
            frames: [-1, 1],
            textures: [(
                "a".to_string(),
                Texture {
                    size: vec![960, 540],
                    format: "rgba16f".into(),
                    linear: true,
                    keep: false,
                    previous: false,
                    data: None,
                },
            )]
            .into(),
            passes,
            ..Graph::default()
        }
    }

    #[test]
    fn a_well_formed_graph_passes_the_check() {
        let g = graph(vec![
            pass("one", &["picture"], "a"),
            pass("two", &["a", "picture:1", "output:1"], "output"),
        ]);
        assert_eq!(g.check(At::Picture), Ok(()));
        assert_eq!(Graph::default().check(At::Picture), Ok(()), "空は素通し");
    }

    #[test]
    fn the_previous_of_a_texture_needs_the_flag() {
        let reads = vec![
            pass("p", &["picture"], "a"),
            pass("q", &["a", "a:1"], "output"),
        ];
        assert!(graph(reads.clone()).check(At::Picture).is_err());
        let mut g = graph(reads);
        g.textures.get_mut("a").unwrap().previous = true;
        g.check(At::Picture).expect("previous なら読める");
    }

    #[test]
    fn the_check_names_what_is_wrong() {
        let unread = graph(vec![pass("one", &["b"], "output")]);
        assert!(
            unread
                .check(At::Picture)
                .unwrap_err()
                .contains("まだ作られていません")
        );
        let same = graph(vec![pass("one", &["a"], "a"), pass("x", &[], "output")]);
        assert!(same.check(At::Picture).unwrap_err().contains("同じ"));
        let far = graph(vec![pass("one", &["picture:2"], "output")]);
        assert!(far.check(At::Picture).unwrap_err().contains("history"));
        let last = graph(vec![pass("one", &["picture"], "a")]);
        assert!(last.check(At::Picture).unwrap_err().contains("output"));
        let mut wide = graph(vec![pass("one", &["picture"], "output")]);
        wide.frames = [-2, 2];
        assert!(
            wide.check(At::Picture).is_err(),
            "picture の領域は拡張できない"
        );
        assert_eq!(wide.check(At::Deinterlace), Ok(()));
    }

    fn buffered() -> Graph {
        let mut net = pass("net", &[], "");
        net.kind = Kind::Compute;
        net.groups = [4, 2, 1];
        net.buffers = vec![
            Bind {
                binding: 0,
                name: "w".into(),
                ..Bind::default()
            },
            Bind {
                binding: 4,
                name: "params".into(),
                access: Access::Uniform,
                offset: 256,
                size: 96,
            },
        ];
        let mut g = graph(vec![net, pass("show", &["picture"], "output")]);
        g.buffers.insert(
            "w".into(),
            Buffer {
                size: 16,
                blob: Some(BlobAt {
                    name: "weights".into(),
                    offset: 8,
                }),
                ..Buffer::default()
            },
        );
        g.buffers.insert(
            "params".into(),
            Buffer {
                size: 512,
                data: Some("00ff".into()),
                ..Buffer::default()
            },
        );
        g.blobs.insert("weights".into(), Blob::new(vec![0u8; 24]));
        g
    }

    #[test]
    fn a_compute_pass_may_write_only_buffers() {
        let g = buffered();
        assert_eq!(g.check(At::Picture), Ok(()));
        let json = to_json(&g);
        let back: Graph = from_json(&json).unwrap();
        assert!(back.blobs.is_empty(), "blob は JSON に載らない");
        assert_eq!(back.buffers, g.buffers);
        assert_eq!(back.passes, g.passes);
        assert!(format!("{:?}", g.blobs).contains("24 バイト"));
        let mut bad = g.clone();
        bad.passes[0].kind = Kind::Fragment;
        assert!(bad.check(At::Picture).is_err());
    }

    #[test]
    fn buffers_are_checked() {
        let mut g = buffered();
        g.passes[0].buffers[1].offset = 480;
        assert!(g.check(At::Picture).unwrap_err().contains("範囲"));
        let mut g = buffered();
        g.passes[0].buffers[1].binding = 0;
        assert!(g.check(At::Picture).unwrap_err().contains("重なって"));
        let mut g = buffered();
        g.passes[0].buffers[0].name = "nothing".into();
        assert!(g.check(At::Picture).unwrap_err().contains("ありません"));
        let mut g = buffered();
        g.buffers.get_mut("w").unwrap().size = 6;
        assert!(g.check(At::Picture).unwrap_err().contains("4 の倍数"));
        let mut g = buffered();
        g.buffers
            .get_mut("w")
            .unwrap()
            .blob
            .as_mut()
            .unwrap()
            .offset = 99;
        assert!(g.check(At::Picture).unwrap_err().contains("外"));
        let mut g = buffered();
        let net = g.passes[0].clone();
        g.passes = std::iter::repeat_n(net, MOST_PASSES)
            .chain(g.passes.pop())
            .collect();
        assert!(g.check(At::Picture).unwrap_err().contains("多すぎ"));
    }

    #[test]
    fn preparing_goes_through_json() {
        for event in [
            Event::Preparing {
                progress: Some(0.5),
                said: String::from("エンジン"),
            },
            Event::Preparing {
                progress: None,
                said: String::new(),
            },
            Event::Prepared,
        ] {
            let json = to_json(&event);
            assert_eq!(from_json::<Event>(&json), Ok(event));
        }
        assert_eq!(
            String::from_utf8(to_json(&Event::Prepared)).unwrap(),
            r#"{"kind":"prepared"}"#
        );
    }

    #[test]
    fn an_outside_process_needs_a_program() {
        let host = |process: Option<Process>| {
            let mut p = pass("host", &["picture:1", "picture"], "output");
            p.kind = Kind::Host;
            p.source = format!("{HOST_INTERPOLATE}/{HOST_PROCESS}");
            p.process = process;
            let mut g = graph(vec![p]);
            g.history = 1;
            g.check(At::Picture)
        };
        assert!(host(None).unwrap_err().contains("実行ファイル"));
        assert_eq!(
            host(Some(Process {
                program: String::from("a.exe"),
                ..Process::default()
            })),
            Ok(())
        );
        let json = String::from_utf8(to_json(&pass("p", &[], "output"))).unwrap();
        assert!(!json.contains("process"), "{json}");
        assert!(!json.contains("held"), "{json}");
    }

    #[test]
    fn the_uniform_is_written_in_the_order_the_wgsl_declares() {
        let zw = Zw {
            input_size: [1440.0, 1080.0],
            output_size: [1920.0, 1080.0],
            frame: 7,
            phase: 0.5,
            field: 1,
            first_row: 1,
            mate_row: 2,
            matched: 0,
            random: 0.25,
            outputs: 2,
            second: 1,
        };
        let bytes = zw.bytes();
        assert_eq!(&bytes[0..4], &1440.0f32.to_le_bytes());
        assert_eq!(&bytes[16..20], &7u32.to_le_bytes());
        assert_eq!(&bytes[20..24], &0.5f32.to_le_bytes());
        assert_eq!(&bytes[44..48], &2u32.to_le_bytes());
        assert_eq!(&bytes[48..52], &1u32.to_le_bytes());
        assert_eq!(bytes.len(), Zw::SIZE);
        let order = [
            "input_size",
            "output_size",
            "frame",
            "phase",
            "field",
            "first_row",
            "mate_row",
            "matched",
            "random",
            "outputs",
            "second",
        ];
        let mut at = 0;
        for name in order {
            let found = ZW_WGSL[at..].find(&format!("    {name}:")).expect(name) + at;
            at = found + 1;
        }
    }

    #[test]
    fn names_say_what_they_point_at() {
        assert_eq!(Source::of("picture"), Source::Picture(0));
        assert_eq!(Source::of("picture:2"), Source::Picture(2));
        assert_eq!(Source::of("output:1"), Source::Output(1));
        assert_eq!(Source::of("output"), Source::Named("output"));
        assert_eq!(Source::of("lut"), Source::Named("lut"));
        assert_eq!(ident("picture:1"), "picture_1");
        assert_eq!(from_hex("0a ff\n10"), Some(vec![0x0a, 0xff, 0x10]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(format_name("rgba16f"), Some("rgba16float"));
        assert_eq!(texel_bytes("r32f"), Some(4));
    }
}
