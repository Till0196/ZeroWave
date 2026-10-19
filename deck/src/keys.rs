// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! キーバインド。`zerowave.toml` の `[keys]`（キー → コマンド）が既定の割り当てを置き換える。

use std::collections::BTreeMap;

use crate::window::menu::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    ChannelUp,
    ChannelDown,
    VolumeUp,
    VolumeDown,
    Mute,
    NextSound,
    NextPicture,
    Caption,
    Osd,
    Pause,
    SeekForward,
    SeekBack,
    /// リモコンの番号（1〜12）。いま見ている種別の中で。
    Remote(u8),
    Menu(Command),
}

impl Action {
    pub fn name(self) -> String {
        match self {
            Self::ChannelUp => "channel.up".into(),
            Self::ChannelDown => "channel.down".into(),
            Self::VolumeUp => "volume.up".into(),
            Self::VolumeDown => "volume.down".into(),
            Self::Mute => "mute".into(),
            Self::NextSound => "audio.next".into(),
            Self::NextPicture => "video.next".into(),
            Self::Caption => "caption".into(),
            Self::Osd => "osd".into(),
            Self::Pause => "pause".into(),
            Self::SeekForward => "seek.forward".into(),
            Self::SeekBack => "seek.back".into(),
            Self::Remote(n) => format!("channel.key {n}"),
            Self::Menu(command) => match command {
                Command::Open => "open".into(),
                Command::PlayerMode => "player_mode".into(),
                Command::FullScreen => "fullscreen".into(),
                Command::AlwaysOnTop => "always_on_top".into(),
                Command::StartStopRecording => "record".into(),
                Command::CopyImage => "capture.copy".into(),
                Command::SaveImage => "capture.save".into(),
                Command::PlaybackOff => "playback_off".into(),
                Command::Reset => "reset".into(),
                Command::ResetViewer => "reset_viewer".into(),
                Command::RebuildPlayer => "rebuild_player".into(),
                Command::ResetErrorCount => "reset_error_count".into(),
                Command::AddFavourite => "favourite.add".into(),
                Command::OrganizeFavourites => "favourite.organize".into(),
                Command::ClearRecent => "recent.clear".into(),
                Command::ChannelBack => "channel.back".into(),
                Command::ChannelForward => "channel.forward".into(),
                Command::ChannelPrevious => "channel.previous".into(),
                Command::PanelWindow => "panel".into(),
                Command::ProgrammeGuide => "guide".into(),
                Command::ProgrammeDetail => "programme_detail".into(),
                Command::CopyProgramme => "copy_programme".into(),
                Command::MenuBar => "menu_bar".into(),
                Command::StatusBar => "status_bar".into(),
                Command::Osd => "osd.toggle".into(),
                Command::OpenRecordFolder => "record.folder".into(),
                Command::OpenCaptureFolder => "capture.folder".into(),
                Command::RecordSettings => "record.settings".into(),
                Command::CaptureSettings => "capture.settings".into(),
                Command::PanScanSettings => "pan_scan.settings".into(),
                Command::Remote => "remote".into(),
                Command::VolumeUp => "volume.up".into(),
                Command::VolumeDown => "volume.down".into(),
                Command::Mute => "mute".into(),
                Command::AudioDelayEarlier => "audio_delay.earlier".into(),
                Command::AudioDelayLater => "audio_delay.later".into(),
                Command::AudioDelayReset => "audio_delay.reset".into(),
                Command::Settings => "settings".into(),
                Command::StreamInformation => "stream_info".into(),
                Command::DecoderInformation => "decoder_info".into(),
                Command::About => "about".into(),
                Command::Close => "close".into(),
            },
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::ChannelUp => "次のチャンネル".into(),
            Self::ChannelDown => "前のチャンネル".into(),
            Self::VolumeUp => "音量を上げる".into(),
            Self::VolumeDown => "音量を下げる".into(),
            Self::Mute => "消音".into(),
            Self::NextSound => "音声切替".into(),
            Self::NextPicture => "映像切替".into(),
            Self::Caption => "字幕".into(),
            Self::Osd => "チャンネル情報の表示".into(),
            Self::Pause => "一時停止".into(),
            Self::SeekForward => "早送り".into(),
            Self::SeekBack => "巻き戻し".into(),
            Self::Remote(n) => format!("チャンネル{n}"),
            Self::Menu(command) => match command {
                Command::Open => "開く".into(),
                Command::PlayerMode => "再生モード".into(),
                Command::FullScreen => "全画面表示".into(),
                Command::AlwaysOnTop => "常に前面に表示".into(),
                Command::StartStopRecording => "録画開始/停止".into(),
                Command::CopyImage => "画像のコピー".into(),
                Command::SaveImage => "画像の保存".into(),
                Command::PlaybackOff => "再生オフ".into(),
                Command::Reset => "リセット".into(),
                Command::ResetViewer => "ビューアリセット".into(),
                Command::RebuildPlayer => "映像と音声の再初期化".into(),
                Command::ResetErrorCount => "エラーカウントリセット".into(),
                Command::AddFavourite => "お気に入りに追加".into(),
                Command::OrganizeFavourites => "お気に入りの管理".into(),
                Command::ClearRecent => "最近見たチャンネルの履歴をクリア".into(),
                Command::ChannelBack => "前のチャンネルに戻る".into(),
                Command::ChannelForward => "次のチャンネルに進む".into(),
                Command::ChannelPrevious => "直前のチャンネル".into(),
                Command::PanelWindow => "パネルウィンドウ".into(),
                Command::ProgrammeGuide => "EPG番組表".into(),
                Command::ProgrammeDetail => "番組説明".into(),
                Command::CopyProgramme => "番組情報のコピー".into(),
                Command::MenuBar => "メニューバーの表示".into(),
                Command::StatusBar => "ステータスバーの表示".into(),
                Command::Osd => "OSD の表示".into(),
                Command::OpenRecordFolder => "録画フォルダを開く".into(),
                Command::OpenCaptureFolder => "キャプチャの保存先を開く".into(),
                Command::RecordSettings => "録画の設定".into(),
                Command::CaptureSettings => "キャプチャの設定".into(),
                Command::PanScanSettings => "パン&スキャンの設定".into(),
                Command::Remote => "リモコン".into(),
                Command::VolumeUp => "音量を上げる".into(),
                Command::VolumeDown => "音量を下げる".into(),
                Command::Mute => "消音".into(),
                Command::AudioDelayEarlier => "音ずれ補正 音声を早める".into(),
                Command::AudioDelayLater => "音ずれ補正 音声を遅らせる".into(),
                Command::AudioDelayReset => "音ずれ補正 リセット".into(),
                Command::Settings => "設定".into(),
                Command::StreamInformation => "ストリームの情報".into(),
                Command::DecoderInformation => "デコーダの情報".into(),
                Command::About => "バージョン情報".into(),
                Command::Close => "閉じる".into(),
            },
        }
    }

    pub fn all() -> Vec<Self> {
        let mut out = vec![
            Self::ChannelUp,
            Self::ChannelDown,
            Self::VolumeUp,
            Self::VolumeDown,
            Self::Mute,
            Self::Menu(Command::AudioDelayEarlier),
            Self::Menu(Command::AudioDelayLater),
            Self::Menu(Command::AudioDelayReset),
            Self::NextSound,
            Self::NextPicture,
            Self::Caption,
            Self::Osd,
            Self::Pause,
            Self::SeekBack,
            Self::SeekForward,
            Self::Menu(Command::FullScreen),
            Self::Menu(Command::AlwaysOnTop),
            Self::Menu(Command::ProgrammeGuide),
            Self::Menu(Command::ProgrammeDetail),
            Self::Menu(Command::CopyProgramme),
            Self::Menu(Command::MenuBar),
            Self::Menu(Command::StatusBar),
            Self::Menu(Command::Osd),
            Self::Menu(Command::PanelWindow),
            Self::Menu(Command::Remote),
            Self::Menu(Command::StreamInformation),
            Self::Menu(Command::DecoderInformation),
            Self::Menu(Command::About),
            Self::Menu(Command::Settings),
            Self::Menu(Command::Open),
            Self::Menu(Command::PlayerMode),
            Self::Menu(Command::PlaybackOff),
            Self::Menu(Command::Reset),
            Self::Menu(Command::ResetViewer),
            Self::Menu(Command::RebuildPlayer),
            Self::Menu(Command::ResetErrorCount),
            Self::Menu(Command::ChannelBack),
            Self::Menu(Command::ChannelForward),
            Self::Menu(Command::ChannelPrevious),
            Self::Menu(Command::AddFavourite),
            Self::Menu(Command::OrganizeFavourites),
            Self::Menu(Command::ClearRecent),
            Self::Menu(Command::StartStopRecording),
            Self::Menu(Command::CopyImage),
            Self::Menu(Command::SaveImage),
            Self::Menu(Command::Close),
        ];
        out.extend((1..=12).map(Self::Remote));
        if cfg!(target_os = "macos") {
            out.retain(|action| *action != Self::Menu(Command::MenuBar));
        }
        out
    }

    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        if let Some(rest) = name.strip_prefix("channel.key") {
            return rest
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|n| (1..=12).contains(n))
                .map(Self::Remote);
        }
        Self::all().into_iter().find(|action| action.name() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub cmd: bool,
    pub key: egui::Key,
}

impl Chord {
    pub fn parse(text: &str) -> Option<Self> {
        let mut chord = Self {
            ctrl: false,
            shift: false,
            alt: false,
            cmd: false,
            key: egui::Key::Space,
        };
        let mut key = None;
        for part in text.split('+') {
            let part = part.trim().to_ascii_lowercase();
            match part.as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" | "option" => chord.alt = true,
                "cmd" | "command" | "super" | "win" => chord.cmd = true,
                "" => return None,
                other => key = Some(key_of(other)?),
            }
        }
        chord.key = key?;
        Some(chord)
    }

    pub fn spelled(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("ctrl".to_owned());
        }
        if self.shift {
            parts.push("shift".to_owned());
        }
        if self.alt {
            parts.push("alt".to_owned());
        }
        if self.cmd {
            parts.push("cmd".to_owned());
        }
        parts.push(name_of(self.key));
        parts.join("+")
    }

    pub fn shown(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_owned());
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
        if self.alt {
            parts.push("Alt".to_owned());
        }
        if self.cmd {
            parts.push("Cmd".to_owned());
        }
        parts.push(match self.key {
            egui::Key::ArrowUp => "↑".to_owned(),
            egui::Key::ArrowDown => "↓".to_owned(),
            egui::Key::ArrowLeft => "←".to_owned(),
            egui::Key::ArrowRight => "→".to_owned(),
            egui::Key::Space => "Space".to_owned(),
            egui::Key::Minus => "-".to_owned(),
            key => key.symbol_or_name().to_uppercase(),
        });
        parts.join("+")
    }

    pub fn of(key: egui::Key, modifiers: egui::Modifiers) -> Self {
        Self {
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            alt: modifiers.alt,
            cmd: modifiers.mac_cmd,
            key,
        }
    }
}

fn key_of(name: &str) -> Option<egui::Key> {
    let egui_name = match name {
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "enter" | "return" => "Enter",
        "space" => "Space",
        "escape" | "esc" => "Escape",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "insert" => "Insert",
        other => {
            if other.len() == 1 {
                return egui::Key::from_name(&other.to_ascii_uppercase());
            }
            if let Some(n) = other.strip_prefix('f')
                && n.parse::<u8>().is_ok()
            {
                return egui::Key::from_name(&other.to_ascii_uppercase());
            }
            return None;
        }
    };
    egui::Key::from_name(egui_name)
}

fn name_of(key: egui::Key) -> String {
    match key {
        egui::Key::ArrowUp => "up".into(),
        egui::Key::ArrowDown => "down".into(),
        egui::Key::ArrowLeft => "left".into(),
        egui::Key::ArrowRight => "right".into(),
        egui::Key::Enter => "enter".into(),
        egui::Key::Space => "space".into(),
        egui::Key::Escape => "escape".into(),
        egui::Key::Tab => "tab".into(),
        egui::Key::Backspace => "backspace".into(),
        egui::Key::Delete => "delete".into(),
        egui::Key::Home => "home".into(),
        egui::Key::End => "end".into(),
        egui::Key::PageUp => "pageup".into(),
        egui::Key::PageDown => "pagedown".into(),
        egui::Key::Insert => "insert".into(),
        egui::Key::Minus => "-".into(),
        other => other.symbol_or_name().to_ascii_lowercase(),
    }
}

pub const DEFAULTS: &[(&str, Action)] = &[
    ("up", Action::VolumeUp),
    ("down", Action::VolumeDown),
    ("left", Action::ChannelDown),
    ("right", Action::ChannelUp),
    ("m", Action::Mute),
    ("a", Action::NextSound),
    ("v", Action::NextPicture),
    ("c", Action::Caption),
    ("i", Action::Osd),
    ("space", Action::Pause),
    ("shift+left", Action::SeekBack),
    ("shift+right", Action::SeekForward),
    ("alt+enter", Action::Menu(Command::FullScreen)),
    ("t", Action::Menu(Command::AlwaysOnTop)),
    ("e", Action::Menu(Command::ProgrammeGuide)),
    ("p", Action::Menu(Command::PanelWindow)),
    ("ctrl+o", Action::Menu(Command::Open)),
    ("r", Action::Menu(Command::StartStopRecording)),
    ("ctrl+w", Action::Menu(Command::Close)),
    ("ctrl+,", Action::Menu(Command::Settings)),
    ("1", Action::Remote(1)),
    ("2", Action::Remote(2)),
    ("3", Action::Remote(3)),
    ("4", Action::Remote(4)),
    ("5", Action::Remote(5)),
    ("6", Action::Remote(6)),
    ("7", Action::Remote(7)),
    ("8", Action::Remote(8)),
    ("9", Action::Remote(9)),
    ("0", Action::Remote(10)),
    ("-", Action::Remote(11)),
    ("=", Action::Remote(12)),
];

#[derive(Debug, Clone, Default)]
pub struct Bindings {
    bound: BTreeMap<Chord, Action>,
}

impl Bindings {
    pub fn of(keys: &BTreeMap<String, String>) -> Self {
        let mut bound: BTreeMap<Chord, Action> = DEFAULTS
            .iter()
            .filter_map(|(spelling, action)| Chord::parse(spelling).map(|chord| (chord, *action)))
            .collect();
        for (spelling, name) in keys {
            let Some(chord) = Chord::parse(spelling) else {
                tracing::warn!("[keys] のキー指定を解釈できない: {spelling}");
                continue;
            };
            bound.remove(&chord);
            if name.trim().is_empty() || name.trim() == "nothing" {
                continue;
            }
            let Some(action) = Action::parse(name) else {
                tracing::info!("[keys] {spelling}: 不明なコマンド {name}");
                continue;
            };
            bound.retain(|_, bound| *bound != action);
            bound.insert(chord, action);
        }
        Self { bound }
    }

    pub fn action(&self, chord: Chord) -> Option<Action> {
        self.bound.get(&chord).copied()
    }

    pub fn chord_of(&self, action: Action) -> Option<Chord> {
        self.bound
            .iter()
            .find(|(_, bound)| **bound == action)
            .map(|(chord, _)| *chord)
    }

    pub fn changes_from_default(&self) -> BTreeMap<String, String> {
        let defaults = Self::of(&BTreeMap::new());
        let mut out = BTreeMap::new();
        for (chord, action) in &defaults.bound {
            match self.bound.get(chord) {
                Some(now) if now == action => {}
                Some(now) => {
                    out.insert(chord.spelled(), now.name());
                }
                None => {
                    out.insert(chord.spelled(), String::from("nothing"));
                }
            }
        }
        for (chord, action) in &self.bound {
            if defaults.bound.get(chord) != Some(action) {
                out.insert(chord.spelled(), action.name());
            }
        }
        out
    }

    /// このコマンドをこのキーに割り当てる。前のキーは外れ、そのキーに割り当てていたコマンドも外れる。
    pub fn bind(&mut self, action: Action, chord: Chord) {
        self.bound.retain(|_, bound| *bound != action);
        self.bound.insert(chord, action);
    }

    pub fn unbind(&mut self, action: Action) {
        self.bound.retain(|_, bound| *bound != action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_go_both_ways() {
        for (spelling, _) in DEFAULTS {
            let chord = Chord::parse(spelling).unwrap_or_else(|| panic!("{spelling}"));
            assert_eq!(chord.spelled(), *spelling, "{spelling}");
        }
        let chord = Chord::parse("Ctrl+Shift+F5").unwrap();
        assert!(chord.ctrl && chord.shift);
        assert_eq!(chord.spelled(), "ctrl+shift+f5");
        assert_eq!(Chord::parse("ctrl+"), None);
        assert_eq!(Chord::parse("ctrl+zz"), None);
    }

    #[test]
    fn every_action_has_a_unique_name_that_reads_back() {
        let all = Action::all();
        for action in &all {
            assert_eq!(
                Action::parse(&action.name()),
                Some(*action),
                "{}",
                action.name()
            );
            assert!(!action.label().is_empty());
        }
        let names: std::collections::BTreeSet<String> = all.iter().map(|a| a.name()).collect();
        assert_eq!(names.len(), all.len(), "名前が重なっている");
    }

    #[test]
    fn a_file_line_replaces_the_default_and_frees_the_old_spelling() {
        let mut keys = BTreeMap::new();
        keys.insert(String::from("ctrl+up"), String::from("channel.up"));
        keys.insert(String::from("m"), String::from("nothing"));
        let bindings = Bindings::of(&keys);
        let up = Chord::parse("ctrl+up").unwrap();
        assert_eq!(bindings.action(up), Some(Action::ChannelUp));
        assert_eq!(bindings.action(Chord::parse("right").unwrap()), None);
        assert_eq!(bindings.action(Chord::parse("m").unwrap()), None);
        let written = bindings.changes_from_default();
        assert_eq!(written["ctrl+up"], "channel.up");
        assert_eq!(written["m"], "nothing");
        assert_eq!(written["right"], "nothing");
        assert_eq!(written.len(), 3, "{written:?}");
        assert!(
            Bindings::of(&BTreeMap::new())
                .changes_from_default()
                .is_empty()
        );
    }
}
