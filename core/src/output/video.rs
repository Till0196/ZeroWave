// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードしたフレームから画面のフレームまで：アップロード、フィールドのスケジュール、デインタレース、色変換。
//! [`filter`] は [`shader`] と同じ計算の Rust 版で、ゴールデンテストで突き合わせる。

pub mod colour;
pub mod field;
pub mod fieldmatch;
pub mod filter;
#[cfg(feature = "wgpu")]
pub mod gpu;
#[cfg(all(feature = "wgpu", feature = "plugins"))]
pub mod hook;
#[cfg(all(feature = "wgpu", feature = "plugins"))]
pub mod host;
/// Drawing from a picture a platform decoder wrote, without a copy.
#[cfg(feature = "wgpu")]
pub mod import;
pub mod pan_scan;
pub mod schedule;
pub mod shader;
pub mod telecine;

/// 表示側が同時に保持してよいデコーダーのサーフェスの数（GPU でレンダリングし終えるまでの分も含む）。
/// フィルターが参照中のサーフェスへデコードされてはならない。デコーダーのリングバッファの大きさもこれで決まる。
pub const HELD_BY_FILTER: usize = 8;

pub use colour::{Adjust, Mapping, Matrix, Primaries, Space, Transfer};
pub use field::{Cadence, Output, Parity, Plan, Source};
pub use filter::comb;

/// フィールドが持たない行の埋め方。マシンごとに負荷が大きく違うので、起動時の計測で既定を選ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Deinterlace {
    /// エンコードされたまま出す。本当にプログレッシブな物だけに正しい。
    Weave,
    #[default]
    Bob,
    /// 動き適応（直線）。設定では `yadif`。
    Yadif,
    /// 動き適応（三次）。設定では `bwdif`。
    Bwdif,
}

impl Deinterlace {
    /// 品質の順（起動時の選択はこれを上から辿る）。負荷の順とは違う。
    pub const LADDER: [Self; 3] = [Self::Bwdif, Self::Yadif, Self::Bob];

    pub fn name(self) -> &'static str {
        match self {
            Self::Weave => "weave",
            Self::Bob => "bob",
            Self::Yadif => "yadif",
            Self::Bwdif => "bwdif",
        }
    }
}

/// フレームを画面の大きさへコピーするときの拡大縮小（`blit.wgsl`）。縮小ではカーネルを広げ、読む点は一辺 12 まで。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scaler {
    Nearest,
    Bilinear,
    /// 縮小の既定。リンギングがほとんど出ない。
    #[default]
    Mitchell,
    CatmullRom,
    Spline36,
    /// 拡大の既定。リンギングが出やすいので antiring と組み合わせる。
    Lanczos3,
    EwaLanczos,
}

impl Scaler {
    pub const ALL: [Self; 7] = [
        Self::Nearest,
        Self::Bilinear,
        Self::Mitchell,
        Self::CatmullRom,
        Self::Spline36,
        Self::Lanczos3,
        Self::EwaLanczos,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Bilinear => "bilinear",
            Self::Mitchell => "mitchell",
            Self::CatmullRom => "catmull-rom",
            Self::Spline36 => "spline36",
            Self::Lanczos3 => "lanczos3",
            Self::EwaLanczos => "ewa-lanczos",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scaler| scaler.name() == name)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest => "Nearest",
            Self::Bilinear => "Bilinear",
            Self::Mitchell => "Mitchell",
            Self::CatmullRom => "Catmull-Rom",
            Self::Spline36 => "Spline36",
            Self::Lanczos3 => "Lanczos3",
            Self::EwaLanczos => "EWA Lanczos",
        }
    }

    /// `blit.wgsl` の `scaled` の番号。0 はバイリニアで、字幕や OSD もこれ。
    pub fn code(self) -> u32 {
        match self {
            Self::Bilinear => 0,
            Self::Nearest => 1,
            Self::Mitchell => 2,
            Self::CatmullRom => 3,
            Self::Spline36 => 4,
            Self::Lanczos3 => 5,
            Self::EwaLanczos => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scaling {
    pub upscale: Scaler,
    pub downscale: Scaler,
    pub antiring: bool,
    /// 拡大・縮小の方式を GPU の性能で選ぶ（[`Scaling::resolved`]）。選ぶまでは `upscale`・`downscale` は使わない。
    pub auto_up: bool,
    pub auto_down: bool,
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            upscale: Scaler::Lanczos3,
            downscale: Scaler::Mitchell,
            antiring: true,
            auto_up: true,
            auto_down: true,
        }
    }
}

impl Scaling {
    /// 自動の方式を決める。`strength` は測った GPU で間に合うデインタレース（測っていなければ `None` で、軽い方式）。
    /// 拡大縮小はデインタレースと同じく毎フレーム画面の大きさで掛かるので、同じ物差しで選ぶ。
    pub fn resolved(self, strength: Option<Deinterlace>) -> Self {
        let upscale = if self.auto_up {
            match strength {
                Some(Deinterlace::Bwdif) => Scaler::Lanczos3,
                Some(Deinterlace::Yadif) => Scaler::CatmullRom,
                _ => Scaler::Bilinear,
            }
        } else {
            self.upscale
        };
        let downscale = if self.auto_down {
            match strength {
                Some(Deinterlace::Bwdif | Deinterlace::Yadif) => Scaler::Mitchell,
                _ => Scaler::Bilinear,
            }
        } else {
            self.downscale
        };
        Self {
            upscale,
            downscale,
            auto_up: false,
            auto_down: false,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub deinterlace: Deinterlace,
    /// デインタレースの方式を明示的に指定した（計測の結果ではない）。GLES 2.0 のパスは DMA-BUF なら指定の無い
    /// weave をフィールドだけの bob に替える。
    pub deinterlace_chosen: bool,
    /// 2:3 プルダウンを見つけたらフィルムのフレームを組み直す（ストリームにフィルムのフラグが無い）。
    pub telecine: bool,
    pub matrix: Matrix,
    pub limited: bool,
    /// 既定は [`Transfer::of_format`]。
    pub transfer: Transfer,
    pub primaries: Primaries,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            deinterlace: Deinterlace::Bob,
            deinterlace_chosen: false,
            telecine: true,
            matrix: Matrix::Bt709,
            limited: true,
            transfer: Transfer::Sdr,
            primaries: Primaries::Bt709,
        }
    }
}

impl Settings {
    /// デインタレーサーはフレームのフラグではなくシーケンスヘッダーで選ぶ
    /// （MPEG-2 の `progressive_frame` は当てにならない）。
    pub fn of_format(format: mediadecoder::es::Format) -> Self {
        Self::adapting(format, adapt_progressive())
    }

    /// `adapt` が偽なら、プログレッシブのシーケンスもインターレースと同じに扱う。
    pub fn adapting(format: mediadecoder::es::Format, adapt: bool) -> Self {
        // HEVC の `field_seq_flag` のフレームは一フィールドで全行そろっている。
        let whole = (format.progressive && adapt) || format.field_pictures;
        Self {
            deinterlace: if whole {
                Deinterlace::Weave
            } else {
                Deinterlace::Bob
            },
            deinterlace_chosen: false,
            telecine: !whole,
            matrix: Matrix::of_format(format),
            limited: !format.colour.full_range,
            transfer: Transfer::of_format(format),
            primaries: Primaries::of_format(format),
        }
    }
}

static ADAPT_PROGRESSIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// プログレッシブと示されたシーケンスとフレームはデインターレースしない。
/// 切ると、示されていても選んだ方式で全部デインターレースする。
pub fn set_adapt_progressive(on: bool) {
    ADAPT_PROGRESSIVE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn adapt_progressive() -> bool {
    ADAPT_PROGRESSIVE.load(std::sync::atomic::Ordering::Relaxed)
}

pub const FIELD_BUDGET_MS: f64 = 1000.0 / 59.94;

/// デインタレーサーだけで使ってよい割合。超える方式は外す。
pub const AFFORDABLE: f64 = 0.5;

/// `buffer_ms` でバッファリングしているときに映像フィルターが使ってよい割合。
pub const HEADROOM: f64 = 0.85;

/// 既にデインタレースされて出てきたフレームの設定。デインタレースを重ねないよう Weave に。
pub fn already_deinterlaced(format: mediadecoder::es::Format) -> Settings {
    Settings {
        deinterlace: Deinterlace::Weave,
        telecine: false,
        ..Settings::of_format(format)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// レンダリングできるデバイスが無い。
    NoDevice(String),
    Unsupported(&'static str),
    /// プールが埋まっている。何も捨てていないので、もう一度渡せる。
    Full,
    Surface(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDevice(why) => write!(out, "描画に使うデバイスを開けません: {why}"),
            Self::Unsupported(what) => write!(out, "この映像は表示できません: {what}"),
            Self::Full => write!(
                out,
                "フレームプールが埋まっている（できた物を先に描画する）"
            ),
            Self::Surface(why) => write!(out, "サーフェスに描画できません: {why}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_progressive_sequence_is_deinterlaced_only_when_not_adapting() {
        let progressive = mediadecoder::es::Format {
            width: 1920,
            height: 1080,
            progressive: true,
            ..mediadecoder::es::Format::default()
        };
        assert_eq!(
            Settings::adapting(progressive, true).deinterlace,
            Deinterlace::Weave
        );
        let forced = Settings::adapting(progressive, false);
        assert_eq!(forced.deinterlace, Deinterlace::Bob);
        assert!(forced.telecine);
        let lone_fields = mediadecoder::es::Format {
            field_pictures: true,
            ..progressive
        };
        assert_eq!(
            Settings::adapting(lone_fields, false).deinterlace,
            Deinterlace::Weave
        );
    }
}
