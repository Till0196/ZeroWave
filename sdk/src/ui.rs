// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! メニュー・ページ・通知（`ui` hook）のデータ型。プラグインはウィジェットの並びを返し、レンダリングは本体が行う。
//! 呼ぶスレッドは本体が決めるので、共有するデータは設定に置く。

use serde::{Deserialize, Serialize};

use crate::video::{At, Format, Program};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub settings: Option<String>,
    /// 下のパネルのタブ。`page` と `event` がパネルの識別子で呼ばれる（ヘッダーとフッターは使わない）。
    #[serde(default)]
    pub panels: Vec<Panel>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Panel {
    pub id: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ctx {
    #[serde(default)]
    pub program: Option<Program>,
    #[serde(default)]
    pub format: Option<Format>,
    #[serde(default)]
    pub video: Option<VideoStatus>,
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// [`Program::band`] の取りうる値。ページのときだけ。
    #[serde(default)]
    pub networks: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub network: u16,
    pub service: u16,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoStatus {
    #[serde(default)]
    pub building: bool,
    #[serde(default)]
    pub stages: Vec<StageStatus>,
    #[serde(default)]
    pub paused: Vec<PausedStatus>,
    #[serde(default)]
    pub failed: Vec<String>,
    #[serde(default)]
    pub budget: f64,
    #[serde(default)]
    pub rate: f64,
    #[serde(default)]
    pub device: String,
    /// 選局からの、レンダリングしたフレームと間に合わず出さなかったフレームの数。
    #[serde(default)]
    pub drawn: u64,
    #[serde(default)]
    pub late: u64,
    /// 直近数秒。0 でなければいま間に合っていない。
    #[serde(default)]
    pub late_recent: u64,
    #[serde(default)]
    pub host: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageStatus {
    pub at: At,
    pub input: [u32; 2],
    pub output: [u32; 2],
    #[serde(default)]
    pub ms: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub passes: Vec<(String, f64)>,
    /// 重さが上限を超えているのに [`crate::video::Graph::keep`] で適用している。
    #[serde(default)]
    pub over: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PausedStatus {
    pub at: At,
    pub ms: f64,
    pub budget: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MenuItem {
    Item {
        id: String,
        label: String,
        #[serde(default)]
        checked: Option<bool>,
        #[serde(default = "yes")]
        enabled: bool,
    },
    Open {
        label: String,
        open: Open,
    },
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Open {
    Settings,
    Page(String),
    /// [`Manifest::panels`] の識別子のタブで下のパネルを開く。
    Panel(String),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub size: Option<[f32; 2]>,
    #[serde(default)]
    pub tabs: Vec<(String, String)>,
    #[serde(default)]
    pub tab: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub footer: Vec<Button>,
    /// キーに結び付けた項目の変更を下書きにし、フッターに「OK」「キャンセル」「適用」を出す。偽なら即座に書き込む。
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node {
    Section {
        label: String,
    },
    Note {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Toggle {
        id: String,
        label: String,
        on: bool,
        #[serde(default)]
        indent: u8,
        #[serde(default = "yes")]
        enabled: bool,
    },
    Line {
        #[serde(default)]
        label: String,
        items: Vec<Inline>,
    },
    Buttons {
        buttons: Vec<Button>,
        #[serde(default)]
        right: bool,
    },
    /// 行を押すと `id` のイベントが `value` に行の識別子で届く。
    Table {
        id: String,
        columns: Vec<Column>,
        rows: Vec<Row>,
        #[serde(default)]
        empty: String,
    },
    Bar {
        label: String,
        parts: Vec<(f32, Tone)>,
        #[serde(default)]
        left: String,
        #[serde(default)]
        right: String,
    },
    Columns {
        width: f32,
        left: Vec<Node>,
        right: Vec<Node>,
    },
    Space {
        height: f32,
    },
    /// 選ぶと `id` のイベントが `value` に選んだ項目の識別子で届く。
    Segments {
        id: String,
        options: Vec<(String, String)>,
        value: String,
    },
    /// `kind = "program"` のキーのカード。値は本体が読み書きする（下のパネルでは見せるだけ）。
    Program {
        key: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        badge: Option<Badge>,
        #[serde(default)]
        hint: String,
    },
    /// `kind = "list"` のキーをタグで並べる項目。値は本体が書く。
    List {
        key: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        hint: String,
    },
    /// `kind = "map"` のキーのマップ。一時的に外せる（[`crate::config::OFF`]）。値は本体が書く。
    Map {
        key: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        empty: String,
    },
    /// `program`・`args`・`env` のキーから本体が組み立てるコマンドライン。下書きがあればそれで組み立てる。
    Command {
        program: String,
        #[serde(default)]
        args: String,
        #[serde(default)]
        env: String,
        #[serde(default)]
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge {
    pub text: String,
    #[serde(default)]
    pub tone: Tone,
}

impl Badge {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Inline {
    Text {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Choice {
        id: String,
        options: Vec<(String, String)>,
        value: String,
        #[serde(default)]
        width: f32,
    },
    Input {
        id: String,
        value: String,
        #[serde(default)]
        width: f32,
        #[serde(default)]
        hint: String,
    },
    /// 押すと `value` にチップの識別子、`removable` の × で `remove:<識別子>`。
    Chips {
        id: String,
        chips: Vec<Chip>,
        #[serde(default)]
        removable: bool,
    },
    Button(Button),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chip {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub on: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    Plain,
    Muted,
    Accent,
    Good,
    Warn,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub id: String,
    pub label: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub icon: String,
}

impl Button {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: true,
            primary: false,
            icon: String::new(),
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = icon.to_owned();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub label: String,
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub right: bool,
}

impl Column {
    pub fn new(label: impl Into<String>, width: f32) -> Self {
        Self {
            label: label.into(),
            width,
            right: false,
        }
    }

    pub fn right(mut self) -> Self {
        self.right = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub cells: Vec<Cell>,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub dim: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Cell {
    Text {
        text: String,
        #[serde(default)]
        tone: Tone,
        #[serde(default)]
        sub: String,
    },
    Tag {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Chips {
        chips: Vec<String>,
        #[serde(default)]
        joint: String,
    },
    Toggle {
        id: String,
        on: bool,
    },
    Choice {
        id: String,
        options: Vec<(String, String)>,
        value: String,
    },
    Buttons {
        buttons: Vec<Button>,
    },
}

impl Cell {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            tone: Tone::Plain,
            sub: String::new(),
        }
    }

    pub fn muted(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            tone: Tone::Muted,
            sub: String::new(),
        }
    }

    pub fn with_sub(text: impl Into<String>, sub: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            tone: Tone::Plain,
            sub: sub.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub open: Option<String>,
    #[serde(default)]
    pub close: bool,
    /// OS のファイル選択ダイアログ。選ばれたら `id` のイベントが `value` にパスで届く。
    #[serde(default)]
    pub pick_file: Option<PickFile>,
    /// 結果は `files` のイベント（`value` が空なら完了）。
    #[serde(default)]
    pub files: Vec<FileOp>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickFile {
    pub id: String,
    #[serde(default)]
    pub filters: Vec<(String, Vec<String>)>,
}

/// `folder` は `kind = "folder"` のキー、`path` はその下の相対パス（`..` は不可）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum FileOp {
    Copy {
        from: String,
        folder: String,
        path: String,
    },
    Remove {
        folder: String,
        path: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    /// 同じキーの通知は表示中のものを差し替える。空なら毎回新しい。
    #[serde(default)]
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub details: Vec<String>,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default)]
    pub progress: Option<f32>,
    #[serde(default)]
    pub seconds: Option<f32>,
}

impl Notice {
    pub fn of_text(text: &str) -> Self {
        let mut lines = text.lines();
        Self {
            title: lines.next().unwrap_or_default().to_owned(),
            details: lines.map(str::to_owned).collect(),
            ..Self::default()
        }
    }
}

fn yes() -> bool {
    true
}

pub fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

pub fn from_json<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[doc(hidden)]
pub trait Word: serde::de::DeserializeOwned + Default {}
impl<T: serde::de::DeserializeOwned + Default> Word for T {}

#[doc(hidden)]
pub fn decode<T: Word>(bytes: &[u8]) -> T {
    serde_json::from_slice(bytes).unwrap_or_default()
}

pub fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains(':')
        && path
            .split(['/', '\\'])
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_round_trip() {
        let items = vec![
            MenuItem::Item {
                id: String::from("auto"),
                label: String::from("自動選択"),
                checked: Some(true),
                enabled: true,
            },
            MenuItem::Separator,
            MenuItem::Open {
                label: String::from("状態…"),
                open: Open::Page(String::from("status")),
            },
        ];
        let json = String::from_utf8(to_json(&items)).unwrap();
        assert!(json.contains(r#""open":{"page":"status"}"#), "{json}");
        assert_eq!(from_json::<Vec<MenuItem>>(json.as_bytes()).unwrap(), items);
        let item: MenuItem = from_json(br#"{"kind":"item","id":"x","label":"X"}"#).unwrap();
        assert!(matches!(
            item,
            MenuItem::Item {
                enabled: true,
                checked: None,
                ..
            }
        ));
        let reply: Reply = from_json(
            br#"{"files":[{"op":"copy","from":"C:/a.glsl","folder":"folder","path":"a.glsl"}]}"#,
        )
        .unwrap();
        assert_eq!(reply.files.len(), 1);
        assert_eq!(Notice::of_text("題\n一\n二").details, ["一", "二"]);
    }

    #[test]
    fn panels_are_declared_and_opened() {
        let manifest: Manifest = from_json(
            r#"{"title":"映像フィルター","panels":[{"id":"filter","title":"映像フィルター"}]}"#
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            manifest.panels,
            [Panel {
                id: String::from("filter"),
                title: String::from("映像フィルター"),
            }]
        );
        let bare: Manifest = from_json(br#"{"title":"x"}"#).unwrap();
        assert!(bare.panels.is_empty());
        let item = MenuItem::Open {
            label: String::from("パネル"),
            open: Open::Panel(String::from("filter")),
        };
        let json = String::from_utf8(to_json(&item)).unwrap();
        assert!(json.contains(r#""open":{"panel":"filter"}"#), "{json}");
        assert_eq!(from_json::<MenuItem>(json.as_bytes()).unwrap(), item);
    }

    #[test]
    fn bound_nodes_round_trip() {
        let page: Page = from_json(
            br#"{"confirm":true,"nodes":[
                {"kind":"segments","id":"kind","options":[["a","A"],["b","B"]],"value":"a"},
                {"kind":"program","key":"p","badge":{"text":"x","tone":"good"}},
                {"kind":"list","key":"l"},
                {"kind":"map","key":"m","label":"M"},
                {"kind":"command","program":"p","args":"l","env":"m"}]}"#,
        )
        .unwrap();
        assert!(page.confirm);
        assert_eq!(page.nodes.len(), 5);
        assert!(matches!(
            &page.nodes[1],
            Node::Program { badge: Some(Badge { tone: Tone::Good, .. }), hint, .. } if hint.is_empty()
        ));
        let json = String::from_utf8(to_json(&page)).unwrap();
        assert_eq!(from_json::<Page>(json.as_bytes()).unwrap(), page);
        assert!(!from_json::<Page>(b"{}").unwrap().confirm);
    }

    #[test]
    fn only_relative_paths_inside_the_folder() {
        assert!(safe_relative("a.glsl"));
        assert!(safe_relative("deinterlace/tgmc.wgsl"));
        for bad in [
            "",
            "../a",
            "a/../b",
            "/etc/x",
            "C:\\x",
            "\\\\host\\x",
            "a//b",
            "./a",
        ] {
            assert!(!safe_relative(bad), "{bad}");
        }
    }
}
