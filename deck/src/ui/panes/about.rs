// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「ZeroWave について」のウィンドウ。表示する文字列はログに書く文字列と同じ（[`About::written`]）。

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use zwcore::output::video::gpu::Gpu;

use crate::machine::{self, Unit};
use crate::ui::theme::settings as skin;
use crate::ui::theme::settings::Skin as Look;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Copy,
    Close,
}

/// ウィンドウの幅。開くときは見積もった高さで開き、集め終えてから合わせ直す（`threads/panes/paint.rs`）。
pub const WIDTH: f32 = 800.0;
pub const HEIGHT: f32 = 520.0;

const FOOT: f32 = 46.0;
const SIDE: f32 = 220.0;
const LOGO: f32 = 112.0;
const LABEL: f32 = 124.0;
const LINE: f32 = 17.0;
const ROW_GAP: f32 = 6.0;
const PAD: f32 = 24.0;
const SIDE_HEIGHT: f32 = 350.0;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct About {
    pub version: String,
    /// コミット（`git describe`）。分からないビルドでは空。
    pub build: String,
    pub built: String,
    pub arch: String,
    pub os: String,
    pub cpus: Vec<(String, String)>,
    pub gpus: Vec<Unit>,
    /// ウィンドウが開いてから足す。
    pub displays: Vec<Unit>,
    pub rendering: String,
    pub decoders: String,
    pub audio: String,
    pub runtime: String,
    pub plugins: Vec<Plugin>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub file: String,
}

struct Row {
    label: String,
    lines: Vec<Line>,
}

struct Line {
    strong: String,
    dim: String,
}

fn line(strong: impl Into<String>, dim: impl Into<String>) -> Line {
    Line {
        strong: strong.into(),
        dim: dim.into(),
    }
}

fn numbered(label: &str, count: usize, index: usize) -> String {
    if count > 1 {
        format!("{label} {}", index + 1)
    } else {
        label.to_owned()
    }
}

impl About {
    pub fn gather(gpu: Option<&Gpu>) -> Self {
        let plugins = crate::plugins::loaded()
            .all()
            .iter()
            .map(|plugin| Plugin {
                name: plugin.manifest().name.clone(),
                version: plugin.manifest().version.clone(),
                file: plugin
                    .path()
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            })
            .collect();
        Self {
            version: String::from(env!("CARGO_PKG_VERSION")),
            build: String::from(env!("ZW_BUILD")),
            built: machine::local_time(env!("ZW_BUILT_AT").parse().unwrap_or(0)),
            arch: String::from(std::env::consts::ARCH),
            os: machine::os(),
            cpus: machine::cpus(),
            gpus: machine::gpus(gpu),
            displays: Vec::new(),
            rendering: machine::rendering(gpu),
            decoders: decoders(),
            audio: audio(),
            runtime: format!("wasmtime {}", env!("ZW_WASMTIME")),
            plugins,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row {
            label: String::from("OS"),
            lines: vec![line(&self.os, "")],
        }];
        for (index, (model, kind)) in self.cpus.iter().enumerate() {
            let lines = if model.is_empty() {
                vec![line(kind, "")]
            } else {
                vec![line(model, format!(" · {kind}"))]
            };
            rows.push(Row {
                label: numbered("CPU", self.cpus.len(), index),
                lines,
            });
        }
        for (label, units) in [("GPU", &self.gpus), ("画面", &self.displays)] {
            for (index, unit) in units.iter().enumerate() {
                let mark = unit
                    .mark
                    .map(|mark| format!(" · {mark}"))
                    .unwrap_or_default();
                let lines = if unit.name.is_empty() {
                    vec![line(&unit.detail, mark)]
                } else {
                    vec![line(&unit.name, mark), line("", &unit.detail)]
                };
                rows.push(Row {
                    label: numbered(label, units.len(), index),
                    lines,
                });
            }
        }
        rows.push(Row {
            label: String::from("レンダリング"),
            lines: vec![line(&self.rendering, "")],
        });
        rows.push(Row {
            label: String::from("デコーダ"),
            lines: vec![line(&self.decoders, "")],
        });
        rows.push(Row {
            label: String::from("サウンド"),
            lines: vec![line(&self.audio, "")],
        });
        rows.push(Row {
            label: String::from("プラグインランタイム"),
            lines: vec![line(&self.runtime, format!(" · ABI {}", zwsdk::host::ABI))],
        });
        let plugins = if self.plugins.is_empty() {
            vec![line("なし", "")]
        } else {
            self.plugins
                .iter()
                .map(|plugin| {
                    let mut dim = String::new();
                    for part in [&plugin.version, &plugin.file] {
                        if !part.is_empty() {
                            dim.push(' ');
                            dim.push_str(part);
                        }
                    }
                    line(&plugin.name, dim)
                })
                .collect()
        };
        rows.push(Row {
            label: String::from("プラグイン"),
            lines: plugins,
        });
        rows
    }

    pub fn height(&self) -> f32 {
        let rows = self.rows();
        let lines: usize = rows.iter().map(|row| row.lines.len()).sum();
        let body = PAD * 2.0 + lines as f32 * LINE + rows.len().saturating_sub(1) as f32 * ROW_GAP;
        body.max(SIDE_HEIGHT) + FOOT
    }

    pub fn written(&self) -> String {
        let mut head = format!("ZeroWave {}", self.version);
        if !self.build.is_empty() {
            head.push_str(" · ");
            head.push_str(&self.build);
        }
        let mut out = format!("{head} · {}\nビルド日 {}\n", self.arch, self.built);
        for row in self.rows() {
            for (index, line) in row.lines.iter().enumerate() {
                let head = if index == 0 { row.label.as_str() } else { "" };
                let text = format!("{}{}", line.strong, line.dim);
                if head.is_empty() {
                    out.push_str(&format!("  {}\n", text.trim_start()));
                } else {
                    out.push_str(&format!("{head}: {}\n", text.trim_start()));
                }
            }
        }
        out.push_str(COPYRIGHT);
        out.push('\n');
        out
    }
}

const COPYRIGHT: &str = "Copyright 2026 Till0196 · Apache-2.0";

/// 組み込んだデコーダ。この PC で開けるかは選局するまで分からない。
fn decoders() -> String {
    let mut names: Vec<&str> = Vec::new();
    if cfg!(target_os = "windows") {
        names.push("D3D12");
    }
    if cfg!(target_vendor = "apple") {
        names.push("VideoToolbox");
    }
    if cfg!(any(feature = "vaapi", target_os = "linux")) {
        names.push("VA-API");
    }
    if cfg!(any(feature = "v4l2", target_os = "linux")) {
        names.push("V4L2");
    }
    if cfg!(feature = "nvdec") {
        names.push("NVDEC");
    }
    names.push("ソフトウェア");
    names.join(" / ")
}

/// 音声の出力方式。出力の名前（Linux は ALSA、ほかは cpal の host）と、組み込んであればパススルー。
fn audio() -> String {
    let host = crate::audio::device::host_name();
    if cfg!(any(feature = "passthrough", target_os = "linux")) {
        format!("{host}（パススルー可）")
    } else {
        host
    }
}

static LOGO_PNG: &[u8] = include_bytes!("../../../icon/about.png");

fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("ZeroWave のアイコン");
    if let Some(held) = ctx.data(|data| data.get_temp::<Option<egui::TextureHandle>>(id)) {
        return held;
    }
    let made = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png)
        .ok()
        .map(|picture| {
            let picture = picture.to_rgba8();
            let size = [picture.width() as usize, picture.height() as usize];
            ctx.load_texture(
                "ZeroWave のアイコン",
                egui::ColorImage::from_rgba_unmultiplied(size, picture.as_raw()),
                egui::TextureOptions {
                    mipmap_mode: Some(egui::TextureFilter::Linear),
                    ..egui::TextureOptions::LINEAR
                },
            )
        });
    ctx.data_mut(|data| data.insert_temp(id, made.clone()));
    made
}

pub fn show(root: &mut Ui, about: &About, look: &Look) -> Option<Pressed> {
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOT), full.max);
    let side = Rect::from_min_max(full.min, pos2(full.left() + SIDE, foot.top()));
    let texture = logo(root.ctx());
    let painter = root.painter();
    painter.line_segment(
        [side.right_top(), side.right_bottom()],
        Stroke::new(1.0, look.edge),
    );

    let icon = Rect::from_center_size(
        pos2(side.center().x, side.top() + 32.0 + LOGO / 2.0),
        vec2(LOGO, LOGO),
    );
    if let Some(texture) = &texture {
        painter.image(
            texture.id(),
            icon,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let mut at = icon.bottom() + 14.0;
    for (text, font, colour, gap) in [
        (
            String::from("ZeroWave"),
            FontId::proportional(skin::TEXT_SIZE + 8.0),
            look.text,
            4.0,
        ),
        (
            format!("バージョン {}", about.version),
            FontId::proportional(skin::TEXT_SIZE - 0.5),
            look.label,
            2.0,
        ),
        (
            about.build.clone(),
            FontId::monospace(skin::TEXT_SIZE - 0.5),
            look.label,
            2.0,
        ),
        (
            about.arch.clone(),
            FontId::monospace(skin::TEXT_SIZE - 0.5),
            look.label,
            4.0,
        ),
        (
            format!("ビルド日 {}", about.built),
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.group,
            0.0,
        ),
    ] {
        if text.is_empty() {
            continue;
        }
        let galley = painter.layout(text, font, colour, side.width() - 32.0);
        let height = galley.size().y;
        painter.galley(
            pos2(side.center().x - galley.size().x / 2.0, at),
            galley,
            colour,
        );
        at += height + gap;
    }
    painter.text(
        pos2(side.center().x, side.bottom() - 20.0),
        Align2::CENTER_BOTTOM,
        "Copyright 2026 Till0196\nApache-2.0",
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        look.group,
    );

    let label_right = side.right() + PAD + LABEL;
    let value_left = label_right + 12.0;
    let clip = Rect::from_min_max(
        pos2(side.right(), full.top()),
        pos2(full.right(), foot.top()),
    );
    let painter = root.painter().with_clip_rect(clip);
    let font = FontId::proportional(skin::TEXT_SIZE);
    let mut y = full.top() + PAD;
    for row in about.rows() {
        for (index, line) in row.lines.iter().enumerate() {
            if index == 0 {
                painter.text(
                    pos2(label_right, y),
                    Align2::RIGHT_TOP,
                    &row.label,
                    font.clone(),
                    look.group,
                );
            }
            let mut job = LayoutJob::default();
            job.append(
                &line.strong,
                0.0,
                TextFormat::simple(font.clone(), look.text),
            );
            job.append(&line.dim, 0.0, TextFormat::simple(font.clone(), look.label));
            let galley = painter.layout_job(job);
            painter.galley(pos2(value_left, y), galley, look.text);
            y += LINE;
        }
        y += ROW_GAP;
    }

    let painter = root.painter();
    painter.rect_filled(foot, 0.0, look.dark);
    painter.line_segment(
        [foot.left_top(), foot.right_top()],
        Stroke::new(1.0, look.edge),
    );
    let mut pressed = None;
    let mut right = foot.right() - 18.0;
    for (label, which, primary, width) in [
        ("OK", Pressed::Close, true, 72.0),
        ("情報をコピー", Pressed::Copy, false, 96.0),
    ] {
        let button = Rect::from_min_size(
            pos2(right - width, foot.center().y - 12.0),
            vec2(width, 24.0),
        );
        right = button.left() - 8.0;
        let response = root.interact(
            button,
            root.scope_id().with(("について", label)),
            Sense::click(),
        );
        let painter = root.painter();
        if primary {
            painter.rect_filled(button, skin::BUTTON_ROUNDING, look.primary);
        } else {
            let fill = if response.hovered() {
                look.button_edge
            } else {
                look.button_fill
            };
            painter.rect_filled(button, skin::BUTTON_ROUNDING, fill);
            painter.rect_stroke(
                button,
                skin::BUTTON_ROUNDING,
                Stroke::new(1.0, look.button_edge),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            button.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            if primary { Color32::WHITE } else { look.text },
        );
        if response.clicked() {
            pressed = Some(which);
        }
    }
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> About {
        About {
            version: String::from("0.1.0"),
            build: String::from("875327d-dirty"),
            built: String::from("2026-10-11 20:32"),
            arch: String::from("x86_64"),
            os: String::from("Windows 11 Pro 24H2 ビルド 26100.4061"),
            cpus: vec![(
                String::from("Intel Core i7-12700K"),
                String::from("x64 · 64 ビット"),
            )],
            gpus: vec![
                Unit {
                    name: String::from("NVIDIA GeForce RTX 4070"),
                    mark: Some("レンダリング利用中"),
                    detail: String::from("Direct3D 12 · ドライバー 32.0.15.7680"),
                },
                Unit {
                    name: String::from("Intel Arc A380 Graphics"),
                    mark: None,
                    detail: String::from("Direct3D 12 · ドライバー 32.0.101.6913"),
                },
            ],
            displays: vec![Unit {
                name: String::from("DELL U2720Q"),
                mark: Some("表示中"),
                detail: String::from("3840×2160 · 60 Hz · 拡大率 150%（144 dpi）"),
            }],
            rendering: String::from("Direct3D 12"),
            decoders: String::from("D3D12 / ソフトウェア"),
            audio: String::from("WASAPI（パススルー可）"),
            runtime: String::from("wasmtime 48.0.2"),
            plugins: vec![
                Plugin {
                    name: String::from("データ放送"),
                    version: String::from("0.1.0"),
                    file: String::from("databroadcast.wasm"),
                },
                Plugin {
                    name: String::from("多重フレーム（TSMF）"),
                    version: String::from("0.1.0"),
                    file: String::from("tsmf.wasm"),
                },
            ],
        }
    }

    #[test]
    fn what_is_copied_follows_the_window() {
        let written = sample().written();
        let mut lines = written.lines();
        assert_eq!(
            lines.next(),
            Some("ZeroWave 0.1.0 · 875327d-dirty · x86_64")
        );
        assert_eq!(lines.next(), Some("ビルド日 2026-10-11 20:32"));
        assert!(written.contains("GPU 1: NVIDIA GeForce RTX 4070 · レンダリング利用中\n"));
        assert!(written.contains("GPU 2: Intel Arc A380 Graphics\n"));
        assert!(written.contains("画面: DELL U2720Q · 表示中\n"));
        assert!(written.contains("プラグイン: データ放送 0.1.0 databroadcast.wasm\n"));
        assert!(written.contains("  多重フレーム（TSMF） 0.1.0 tsmf.wasm\n"));
        assert!(written.trim_end().ends_with("Apache-2.0"));
    }

    #[test]
    fn an_unknown_commit_says_nothing() {
        let mut about = sample();
        about.build.clear();
        assert!(about.written().starts_with("ZeroWave 0.1.0 · x86_64\n"));
    }

    #[test]
    fn a_single_one_has_no_number() {
        let written = sample().written();
        assert!(written.contains("CPU: Intel Core i7-12700K · x64 · 64 ビット\n"));
        assert!(written.contains("画面: "));
        assert!(!written.contains("画面 1"));
    }

    #[test]
    fn no_plugins_is_said_plainly() {
        let about = About::default();
        assert!(about.written().contains("プラグイン: なし"));
    }

    #[test]
    fn more_units_need_a_taller_window() {
        let one = sample();
        let mut two = sample();
        two.displays.push(two.displays[0].clone());
        assert!(two.height() > one.height());
    }

    #[test]
    fn software_decoding_is_always_there() {
        assert!(decoders().ends_with("ソフトウェア"));
    }
}
