// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! コマンドラインオプションで選べること。既定はどれも `auto`。無いものを指定されたらエラーで終了し、理由を出す。

use zwcore::decode::{Api, DeinterlaceBy, Prefer};
use zwcore::engine::DualMono;
use zwcore::output::video::Deinterlace;
pub use zwcore::player::Options;
use zwcore::sound::mix::{FiveOne, TwentyTwo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Render {
    #[default]
    Auto,
    Wgpu,
    Gles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Present {
    #[default]
    Auto,
    Vsync,
    /// 待たずに出す。画面のリフレッシュが極端に遅い環境のため（通常のディスプレイではティアリングが出る）。
    Immediate,
}

impl Present {
    pub fn mode(self) -> wgpu::PresentMode {
        match self {
            Self::Auto | Self::Vsync => wgpu::PresentMode::AutoVsync,
            Self::Immediate => wgpu::PresentMode::AutoNoVsync,
        }
    }

    fn parse(named: &str) -> Result<Self, String> {
        match named {
            "auto" => Ok(Self::Auto),
            "vsync" => Ok(Self::Vsync),
            "immediate" | "now" => Ok(Self::Immediate),
            other => Err(format!("--present に {other}: auto|vsync|immediate")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Hdr {
    #[default]
    Auto,
    Off,
    On,
}

impl Hdr {
    fn parse(named: &str) -> Result<Self, String> {
        match named {
            "auto" => Ok(Self::Auto),
            "off" | "sdr" => Ok(Self::Off),
            "on" | "hdr" => Ok(Self::On),
            other => Err(format!("--hdr に {other}: auto|off|on")),
        }
    }
}

fn dual_mono(named: &str) -> Result<DualMono, String> {
    match named {
        "main" | "primary" | "主" => Ok(DualMono::Main),
        "sub" | "secondary" | "副" => Ok(DualMono::Sub),
        "both" | "stereo" => Ok(DualMono::Both),
        other => Err(format!("--dual-mono に {other}: main|sub|both")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WheelDoes {
    #[default]
    Volume,
    Channel,
    Nothing,
}

impl WheelDoes {
    pub fn parse(key: &str, value: &str) -> Result<Self, String> {
        match value {
            "" | "volume" => Ok(Self::Volume),
            "channel" => Ok(Self::Channel),
            "nothing" | "none" => Ok(Self::Nothing),
            other => Err(format!("{key} に {other}: volume|channel|nothing")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Knobs {
    pub volume_step: u32,
    pub wheel: WheelDoes,
    pub wheel_shift: WheelDoes,
    pub wheel_ctrl: WheelDoes,
    pub oneseg: OneSeg,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            volume_step: 5,
            wheel: WheelDoes::Volume,
            wheel_shift: WheelDoes::Channel,
            wheel_ctrl: WheelDoes::Nothing,
            oneseg: OneSeg::Fallback,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OneSeg {
    #[default]
    Fallback,
    Never,
    Always,
}

impl OneSeg {
    pub fn parse(named: &str) -> Result<Self, String> {
        match named {
            "" | "fallback" => Ok(Self::Fallback),
            "never" => Ok(Self::Never),
            "always" => Ok(Self::Always),
            other => Err(format!("playback.oneseg に {other}: fallback|never|always")),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Choices {
    pub options: Options,
    pub render: Render,
    pub present: Present,
    pub backend: Option<wgpu::Backends>,
    pub gpu: Option<String>,
    pub fit_window: bool,
    pub hdr: Hdr,
    pub knobs: Knobs,
    /// `--render auto` で、この機械では GLES 2.0 で描くと決めた（wgpu にハードウェアの GPU が無い）。
    /// 設定の `render` は auto のまま（設定を保存し直しても再起動にならないように）。
    pub auto_gles: bool,
}

impl Choices {
    /// GLES 2.0 で描くか（`--render gles`、または auto でこの機械が GLES を選んだ）。
    pub fn draws_gles(&self) -> bool {
        self.render == Render::Gles || (self.render == Render::Auto && self.auto_gles)
    }

    pub fn line(&self) -> String {
        let decoder = match (self.options.decoder, self.options.api) {
            (None, Api::Auto) => "auto",
            (None, Api::D3d12) => "auto（D3D12）",
            (None, Api::D3d11) => "auto（D3D11VA）",
            (Some(Prefer::Platform), Api::Auto) => "hardware",
            (Some(Prefer::Platform), Api::D3d12) => "hardware（D3D12）",
            (Some(Prefer::Platform), Api::D3d11) => "hardware（D3D11VA）",
            (Some(Prefer::Software), _) => "software",
            (Some(Prefer::System), _) => "system（OS のソフトウェア）",
        };
        let deinterlace = match self.options.deinterlace {
            None => String::from("auto"),
            Some(Deinterlace::Weave) => String::from("off（weave）"),
            Some(step) => String::from(step.name()),
        };
        let by = match self.options.deinterlace_by {
            DeinterlaceBy::Auto => "auto",
            DeinterlaceBy::Software => "software",
            DeinterlaceBy::Hardware => "hardware",
        };
        let render = match self.render {
            Render::Auto if self.auto_gles => String::from("auto（gles）"),
            Render::Auto => String::from("auto"),
            Render::Wgpu => match self.backend {
                Some(wgpu::Backends::VULKAN) => String::from("wgpu/vulkan"),
                Some(wgpu::Backends::METAL) => String::from("wgpu/metal"),
                Some(wgpu::Backends::DX12) => String::from("wgpu/dx12"),
                _ => String::from("wgpu"),
            },
            Render::Gles => String::from("gles"),
        };
        let gpu = match &self.gpu {
            Some(name) => format!("  GPU {name}"),
            None => String::new(),
        };
        let present = match self.present {
            Present::Auto => String::new(),
            Present::Vsync => String::from("  present vsync"),
            Present::Immediate => String::from("  present immediate"),
        };
        let latency = match self.options.audio_latency {
            0 => String::new(),
            ms if ms > 0 => format!("  音声の遅延 {ms} ms"),
            ms => format!("  映像の遅延 {} ms", -ms),
        };
        let hdr = match self.hdr {
            Hdr::Auto => "",
            Hdr::Off => "  HDR off（サーフェスは SDR）",
            Hdr::On => "  HDR on（サーフェスは HDR）",
        };
        format!(
            "選択  デコーダー {decoder}  デインタレース {deinterlace}（{by}）  レンダリング {render}{gpu}{present}{latency}{hdr}"
        )
    }

    pub fn parse(flag: &str, value: Option<&str>, into: &mut Self) -> Result<(), String> {
        if flag == "--fit-window" {
            into.fit_window = true;
            return Ok(());
        }
        let named = value.unwrap_or("");
        match flag {
            "--decoder" => {
                into.options.decoder = match named {
                    "auto" => None,
                    "hardware" | "hw" | "platform" => Some(Prefer::Platform),
                    "software" | "sw" => Some(Prefer::Software),
                    "system" | "os" => Some(Prefer::System),
                    other => {
                        return Err(format!(
                            "--decoder に {other}: auto|hardware|software|system"
                        ));
                    }
                };
            }
            "--decoder-api" => {
                into.options.api = match named {
                    "auto" => Api::Auto,
                    "d3d12" => Api::D3d12,
                    "d3d11" | "d3d11va" | "dxva" => Api::D3d11,
                    other => return Err(format!("--decoder-api に {other}: auto|d3d12|d3d11")),
                };
            }
            "--deinterlace" => {
                into.options.deinterlace = match named {
                    "auto" => None,
                    // `off` は「何もしない」ではなく、フィールドを合わせたまま出す（`Weave`）。
                    "off" | "none" | "weave" => Some(Deinterlace::Weave),
                    "bob" => Some(Deinterlace::Bob),
                    "yadif" => Some(Deinterlace::Yadif),
                    "bwdif" => Some(Deinterlace::Bwdif),
                    other => {
                        return Err(format!(
                            "--deinterlace に {other}: auto|off|weave|bob|yadif|bwdif"
                        ));
                    }
                };
            }
            "--audio" => {
                into.options.audio = match named.parse::<usize>() {
                    Ok(at) if at >= 1 => Some(at),
                    _ => return Err(format!("--audio に {named}: 1 以上の番号")),
                };
            }
            "--audio-latency" => {
                into.options.audio_latency = match named.parse::<i32>() {
                    Ok(ms) if (-1000..=1000).contains(&ms) => ms,
                    _ => {
                        return Err(format!(
                            "--audio-latency に {named}: -1000〜1000 のミリ秒（負は映像のほうが遅いとき）"
                        ));
                    }
                };
            }
            "--dual-mono" => {
                into.options.dual_mono = dual_mono(named)?;
            }
            "--video" => {
                into.options.video = match named.parse::<usize>() {
                    Ok(at) if at >= 1 => Some(at),
                    _ => return Err(format!("--video に {named}: 1 以上の番号")),
                };
            }
            "--upscale" | "--downscale" => {
                let scaler = zwcore::output::video::Scaler::parse(named).ok_or_else(|| {
                    format!(
                        "{flag} に {named}: nearest|bilinear|mitchell|catmull-rom|spline36|lanczos3|ewa-lanczos"
                    )
                })?;
                if flag == "--upscale" {
                    into.options.scaling.upscale = scaler;
                } else {
                    into.options.scaling.downscale = scaler;
                }
            }
            "--antiring" => {
                into.options.scaling.antiring = match named {
                    "on" | "true" => true,
                    "off" | "false" => false,
                    other => return Err(format!("--antiring に {other}: on|off")),
                };
            }
            "--telecine" => {
                into.options.telecine = match named {
                    "auto" => None,
                    "on" | "true" => Some(true),
                    "off" | "false" => Some(false),
                    other => return Err(format!("--telecine に {other}: auto|on|off")),
                };
            }
            "--deinterlace-by" => {
                into.options.deinterlace_by = match named {
                    "auto" => DeinterlaceBy::Auto,
                    "software" | "sw" => DeinterlaceBy::Software,
                    "hardware" | "hw" => DeinterlaceBy::Hardware,
                    other => {
                        return Err(format!(
                            "--deinterlace-by に {other}: auto|software|hardware"
                        ));
                    }
                };
            }
            "--render" => {
                into.render = match named {
                    "auto" => Render::Auto,
                    "wgpu" => Render::Wgpu,
                    "gles" | "gl" | "es2" => Render::Gles,
                    other => return Err(format!("--render に {other}: auto|wgpu|gles")),
                };
            }
            "--gpu" => {
                let name = named.trim();
                if name.is_empty() {
                    return Err(String::from("--gpu に名前が無い"));
                }
                into.gpu = (name != "auto").then(|| String::from(name));
            }
            "--present" => into.present = Present::parse(named)?,
            "--hdr" => into.hdr = Hdr::parse(named)?,
            "--backend" => {
                into.backend = match named {
                    "dx12" | "d3d12" => Some(wgpu::Backends::DX12),
                    "vulkan" | "vk" => Some(wgpu::Backends::VULKAN),
                    "metal" => Some(wgpu::Backends::METAL),
                    "gles" | "gl" | "es2" => {
                        into.render = Render::Gles;
                        None
                    }
                    other => {
                        return Err(format!(
                            "--backend に {other}: vulkan|metal|dx12（レンダリングの方式は --render）"
                        ));
                    }
                };
                if into.backend.is_some() && into.render == Render::Auto {
                    into.render = Render::Wgpu;
                }
            }
            "--mono-to-stereo" => {
                into.options.mix.mono_to_stereo = match named {
                    "true" | "on" | "yes" => true,
                    "false" | "off" | "no" => false,
                    other => return Err(format!("--mono-to-stereo に {other}: true|false")),
                };
            }
            "--downmix" => {
                into.options.mix.five_one = match named {
                    "auto" => FiveOne::Auto,
                    "stereo" => FiveOne::Stereo,
                    "off" => FiveOne::Off,
                    other => return Err(format!("--downmix に {other}: auto|stereo|off")),
                };
            }
            "--downmix-22" => {
                into.options.mix.twenty_two = match named {
                    "auto" => TwentyTwo::Auto,
                    "stereo" => TwentyTwo::Stereo,
                    "51" | "5.1" => TwentyTwo::FiveOne,
                    "71" | "7.1" => TwentyTwo::SevenOne,
                    "off" => TwentyTwo::Off,
                    other => {
                        return Err(format!("--downmix-22 に {other}: auto|stereo|51|71|off"));
                    }
                };
            }
            other => return Err(format!("不明なオプション: {other}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oneseg_spellings() {
        assert_eq!(OneSeg::parse(""), Ok(OneSeg::Fallback));
        assert_eq!(OneSeg::parse("fallback"), Ok(OneSeg::Fallback));
        assert_eq!(OneSeg::parse("never"), Ok(OneSeg::Never));
        assert_eq!(OneSeg::parse("always"), Ok(OneSeg::Always));
        assert!(OneSeg::parse("sometimes").is_err());
    }

    fn read(pairs: &[(&str, &str)]) -> Choices {
        let mut choices = Choices::default();
        for (flag, value) in pairs {
            Choices::parse(flag, Some(value), &mut choices).expect("読める");
        }
        choices
    }

    #[test]
    fn nothing_named_leaves_everything_automatic() {
        let choices = Choices::default();
        assert_eq!(choices.options.decoder, None);
        assert_eq!(choices.options.deinterlace, None);
        assert_eq!(choices.options.deinterlace_by, DeinterlaceBy::Auto);
        assert_eq!(choices.render, Render::Auto);
        assert_eq!(choices.options.prefer(), Prefer::Platform);
        assert_eq!(choices.hdr, Hdr::Auto);
    }

    #[test]
    fn hdr_is_auto_off_or_on() {
        assert_eq!(read(&[("--hdr", "off")]).hdr, Hdr::Off);
        assert_eq!(read(&[("--hdr", "on")]).hdr, Hdr::On);
        assert_eq!(read(&[("--hdr", "auto")]).hdr, Hdr::Auto);
        let mut choices = Choices::default();
        assert!(Choices::parse("--hdr", Some("bright"), &mut choices).is_err());
    }

    #[test]
    fn off_means_weave_not_nothing() {
        assert_eq!(
            read(&[("--deinterlace", "off")]).options.deinterlace,
            Some(Deinterlace::Weave)
        );
        assert_eq!(
            read(&[("--deinterlace", "weave")]).options.deinterlace,
            Some(Deinterlace::Weave)
        );
    }

    #[test]
    fn the_step_and_the_place_are_separate_axes() {
        let choices = read(&[("--deinterlace", "bwdif")]);
        assert_eq!(choices.options.deinterlace, Some(Deinterlace::Bwdif));
        assert_eq!(
            choices.options.deinterlace_by,
            DeinterlaceBy::Auto,
            "場所は指定されていない"
        );
        let choices = read(&[("--deinterlace-by", "hardware")]);
        assert_eq!(choices.options.deinterlace, None, "方式は指定されていない");
        assert_eq!(choices.options.deinterlace_by, DeinterlaceBy::Hardware);
    }

    #[test]
    fn when_to_show_a_picture_can_be_named() {
        assert_eq!(
            read(&[("--present", "immediate")]).present,
            Present::Immediate
        );
        assert_eq!(read(&[("--present", "now")]).present, Present::Immediate);
        assert_eq!(read(&[("--present", "vsync")]).present, Present::Vsync);
        assert_eq!(Choices::default().present, Present::Auto);
        assert_eq!(
            Choices::default().present.mode(),
            wgpu::PresentMode::AutoVsync
        );
    }

    #[test]
    fn the_older_spelling_for_the_hand_written_route_still_works() {
        assert_eq!(read(&[("--backend", "gles")]).render, Render::Gles);
        assert_eq!(read(&[("--render", "gles")]).render, Render::Gles);
    }

    #[test]
    fn a_gpu_is_named_and_auto_is_none() {
        assert_eq!(
            read(&[("--gpu", "AMD Radeon(TM) Graphics")]).gpu.as_deref(),
            Some("AMD Radeon(TM) Graphics")
        );
        assert_eq!(read(&[("--gpu", "auto")]).gpu, None);
        assert!(Choices::parse("--gpu", Some("  "), &mut Choices::default()).is_err());
    }

    #[test]
    fn naming_a_backend_means_wgpu() {
        let choices = read(&[("--backend", "vulkan")]);
        assert_eq!(choices.render, Render::Wgpu);
        assert_eq!(choices.backend, Some(wgpu::Backends::VULKAN));
    }

    #[test]
    fn an_unknown_name_says_what_the_names_are() {
        let mut choices = Choices::default();
        let told = Choices::parse("--deinterlace", Some("bwdif2"), &mut choices)
            .expect_err("エラーになるはず");
        assert!(told.contains("bwdif"), "使える名前が出ていない: {told}");
        let told = Choices::parse("--render", Some("opengl4"), &mut choices)
            .expect_err("エラーになるはず");
        assert!(told.contains("gles"), "使える名前が出ていない: {told}");
    }

    #[test]
    fn system_is_only_what_is_named() {
        for name in ["system", "os"] {
            let choices = read(&[("--decoder", name)]);
            assert_eq!(choices.options.prefer(), Prefer::System);
            assert!(choices.line().contains("system"), "{}", choices.line());
        }
        assert_eq!(
            read(&[("--decoder", "auto")]).options.prefer(),
            Prefer::Platform
        );
        let told = Choices::parse("--decoder", Some("mft"), &mut Choices::default())
            .expect_err("エラーになるはず");
        assert!(told.contains("system"), "使える名前が出ていない: {told}");
    }

    #[test]
    fn the_line_says_what_is_in_effect() {
        let line = read(&[
            ("--decoder", "software"),
            ("--deinterlace", "bwdif"),
            ("--deinterlace-by", "hardware"),
            ("--render", "gles"),
        ])
        .line();
        assert!(line.contains("software"), "{line}");
        assert!(line.contains("bwdif"), "{line}");
        assert!(line.contains("hardware"), "{line}");
        assert!(line.contains("gles"), "{line}");
        assert!(Choices::default().line().contains("auto"));
    }
}
