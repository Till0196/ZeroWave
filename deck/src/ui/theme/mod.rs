// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画面の色と寸法。色はこのディレクトリにしか書かない（下のテストが見張る）。
//! 明暗で変わるものは `Skin` に `DARK`・`LIGHT`・`skin(mode)` で置き、[`Theme`] に載せる。
//! `ZW_THEME=light|dark` で両方を確かめられる。

use egui::Color32;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Light,
    Dark,
}

/// 設定で選んだ明暗（`[ui] theme`）。0 は OS に従う、1 は明るい、2 は暗い。
static CHOSEN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn choose(named: &str) {
    let chosen = match named {
        "light" => 1,
        "dark" => 2,
        _ => 0,
    };
    CHOSEN.store(chosen, std::sync::atomic::Ordering::Relaxed);
}

impl Mode {
    /// 指定が無ければ明るいほう。設定、`ZW_THEME` の順に強い。
    pub fn of(theme: Option<winit::window::Theme>) -> Self {
        match std::env::var("ZW_THEME").as_deref() {
            Ok("light") => return Self::Light,
            Ok("dark") => return Self::Dark,
            _ => {}
        }
        match CHOSEN.load(std::sync::atomic::Ordering::Relaxed) {
            1 => return Self::Light,
            2 => return Self::Dark,
            _ => {}
        }
        match theme {
            Some(winit::window::Theme::Dark) => Self::Dark,
            _ => Self::Light,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub mode: Mode,
    pub status: status::Skin,
    pub stream: stream::Skin,
    pub decoder: decoder::Skin,
    pub epg: epg::Skin,
    pub mini: mini::Skin,
    pub settings: settings::Skin,
    pub detail: detail::Skin,
    pub chips: chips::Skin,
    pub remote: remote::Skin,
}

impl Theme {
    pub const fn of(mode: Mode) -> Self {
        Self {
            mode,
            status: status::skin(mode),
            stream: stream::skin(mode),
            decoder: decoder::skin(mode),
            epg: epg::skin(mode),
            mini: mini::skin(mode),
            settings: settings::skin(mode),
            detail: detail::skin(mode),
            chips: chips::skin(mode),
            remote: remote::skin(mode),
        }
    }
}

/// 白へ寄せる。`lift` は八分の幾つ白に寄せるか。
pub const fn lightened(colour: Color32, lift: u8) -> Color32 {
    const fn raise(channel: u8, lift: u8) -> u8 {
        (channel as u16 + (255 - channel as u16) * lift as u16 / 8) as u8
    }
    Color32::from_rgb(
        raise(colour.r(), lift),
        raise(colour.g(), lift),
        raise(colour.b(), lift),
    )
}

pub mod status;

pub mod table;

pub mod stream;

pub mod decoder;

pub mod epg;

pub mod mini;

pub mod settings;

pub mod remote;

pub mod detail;

pub mod chips;

pub mod osd;

pub mod transport;

pub mod overlay;

pub mod scene;

#[cfg(test)]
mod tests {
    use super::*;

    fn skins() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/theme");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("読める").flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let name = path
                .file_stem()
                .expect("名前")
                .to_string_lossy()
                .into_owned();
            if name == "mod" {
                continue;
            }
            out.push((name, std::fs::read_to_string(&path).expect("読める")));
        }
        out.sort();
        out
    }

    fn theme_struct() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/theme/mod.rs");
        let source = std::fs::read_to_string(path).expect("読める");
        let start = source.find("pub struct Theme {").expect("Theme");
        let end = start + source[start..].find("\n}").expect("閉じ");
        source[start..end].to_owned()
    }

    /// `Skin` のある mod は `DARK`・`LIGHT`・`skin()` を三つとも持つ。
    #[test]
    fn every_skin_has_both_sides() {
        let mut seen = 0;
        for (name, body) in skins() {
            if !body.contains("pub struct Skin") {
                continue;
            }
            seen += 1;
            for needed in [
                "pub const DARK: Skin",
                "pub const LIGHT: Skin",
                "pub const fn skin(",
            ] {
                assert!(body.contains(needed), "theme::{name} に {needed} が無い");
            }
        }
        assert!(seen >= 7, "Skin のある mod が {seen} しか見つからない");
    }

    /// `Skin` のある mod は、どれも [`Theme`] に一欄ある。
    #[test]
    fn the_theme_carries_every_skin() {
        let theme = theme_struct();
        for (name, body) in skins() {
            if body.contains("pub struct Skin") {
                let field = format!("pub {name}: {name}::Skin,");
                assert!(theme.contains(&field), "Theme に `{field}` が無い");
            }
        }
    }

    /// 色の数は theme にしか書かない。放送の色と映像の上のものだけ例外。
    #[test]
    fn colours_live_only_here() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed = ["caption.rs", "databroadcast.rs", "badges.rs", "osd.rs"];
        let is_allowed = |relative: &str| {
            relative.starts_with("ui/theme/")
                || (relative.starts_with("ui/")
                    && allowed
                        .iter()
                        .any(|name| relative.rsplit('/').next() == Some(name)))
        };
        let mut files = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("読める").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        walk(&root, &mut files);
        let mut offenders = Vec::new();
        for path in files {
            let relative = path
                .strip_prefix(&root)
                .expect("src の下")
                .to_string_lossy()
                .replace('\\', "/");
            if is_allowed(&relative) {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("読める");
            for (n, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                for call in [
                    "from_rgb(",
                    "from_rgba_premultiplied(",
                    "from_rgba_unmultiplied(",
                    "from_gray(",
                    "from_black_alpha(",
                    "from_white_alpha(",
                ] {
                    if let Some(at) = line.find(call) {
                        let after = line[at + call.len()..].trim_start();
                        if after.starts_with(|c: char| c.is_ascii_digit()) {
                            offenders.push(format!("{relative}:{}: {}", n + 1, line.trim()));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "色の数は ui/theme/ に置く（明暗で忘れる）:\n{}",
            offenders.join("\n")
        );
    }

    /// 薄めても色味の向きが変わらないこと。差が 6 以内の組は灰の青みほどなので数えない。
    #[test]
    fn a_lightened_colour_keeps_which_way_it_leans() {
        for (dark, light) in epg::DARK.genres.iter().zip(epg::LIGHT.genres) {
            let leans = |colour: Color32| {
                let (red, green, blue) = (colour.r(), colour.g(), colour.b());
                let way = |a: u8, b: u8| (a.abs_diff(b) > 6).then_some(a > b);
                [way(red, green), way(green, blue), way(red, blue)]
            };
            for (one, other) in leans(*dark).iter().zip(leans(light)) {
                if let (Some(one), Some(other)) = (one, other) {
                    assert_eq!(*one, other, "{dark:?} → {light:?}");
                }
            }
        }
    }

    #[test]
    fn a_lightened_colour_is_actually_light() {
        for light in epg::LIGHT.genres.iter().chain([&epg::LIGHT.genre_end]) {
            let darkest = light.r().min(light.g()).min(light.b());
            assert!(darkest > 0xb0, "{light:?} は白い背景の上で暗すぎる");
            assert!(
                light.r().max(light.g()).max(light.b()) < 0xff,
                "{light:?} が背景に溶ける"
            );
        }
    }

    #[test]
    fn the_dark_genres_sit_on_the_dark_ground() {
        for dark in epg::DARK.genres.iter().chain([&epg::DARK.genre_end]) {
            let brightest = dark.r().max(dark.g()).max(dark.b());
            assert!(brightest < 0x80, "{dark:?} は暗い背景の上で明るすぎる");
            assert!(
                brightest > epg::DARK.background.r(),
                "{dark:?} が背景に沈む"
            );
        }
    }

    #[test]
    fn the_four_genres_are_told_apart() {
        let told: std::collections::BTreeSet<[u8; 3]> = [1, 3, 4, 6, 15]
            .map(|at| epg::DARK.genres[at])
            .iter()
            .map(|colour| [colour.r(), colour.g(), colour.b()])
            .collect();
        assert_eq!(told.len(), 5);
    }

    #[test]
    fn the_chosen_theme_wins_over_the_system() {
        if std::env::var_os("ZW_THEME").is_some() {
            return;
        }
        choose("dark");
        assert_eq!(Mode::of(Some(winit::window::Theme::Light)), Mode::Dark);
        choose("light");
        assert_eq!(Mode::of(Some(winit::window::Theme::Dark)), Mode::Light);
        choose("auto");
        assert_eq!(Mode::of(Some(winit::window::Theme::Dark)), Mode::Dark);
    }

    #[test]
    fn no_theme_from_the_system_means_the_light_one() {
        assert_eq!(Mode::of(None), Mode::Light);
        assert_eq!(Mode::of(Some(winit::window::Theme::Dark)), Mode::Dark);
        assert_eq!(Mode::of(Some(winit::window::Theme::Light)), Mode::Light);
    }

    #[test]
    fn each_skin_says_which_one_it_is() {
        assert_eq!(epg::LIGHT.mode, Mode::Light);
        assert_eq!(epg::DARK.mode, Mode::Dark);
        assert_eq!(epg::skin(Mode::Dark).mode, Mode::Dark);
    }
}
