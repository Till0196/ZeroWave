// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 日本語のフォントをこの PC から探して egui に持たせる（egui のフォントに日本語は無く、同梱もしない）。
//! TTC のように複数のフェイスがあるときは name テーブルを読んで UI 向けを選ぶ。

use std::sync::Arc;

use zwsdk::host::fonts::{Catalog, Face as CatalogFace};

/// 記号のフォントを選ぶときに調べる文字。ARIB の追加記号は日本語のフォントに無いものが多い。
const SYMBOLS: &str = "⚟⚞♬☎⛅⛄☂⛔⚡";

mod system;

/// 無い名前で文字をレイアウトすると egui が止まるので、どのコンテキストにも必ず作る。
pub const OSD: &str = "osd";
pub const CAPTION: &str = "caption";
pub const SUPERIMPOSE: &str = "superimpose";
pub const GUIDE: &str = "guide";
pub const STATUS: &str = "status";

const REFERENCE_PT: f32 = 9.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choice {
    pub ui: String,
    pub ui_size: u32,
    pub osd: String,
    pub caption: String,
    pub superimpose: String,
    pub fallback: bool,
    pub guide: String,
    pub guide_size: u32,
    pub status: String,
}

impl Choice {
    pub fn of(file: &crate::config::File) -> Self {
        Self {
            ui: file.ui.font.clone(),
            ui_size: file.ui.font_size,
            osd: file.osd.font.clone(),
            caption: file.caption.font.clone(),
            superimpose: file.caption.superimpose_font.clone(),
            fallback: file.caption.fallback,
            guide: file.guide.font.clone(),
            guide_size: file.guide.font_size,
            status: file.ui.status_font.clone(),
        }
    }
}

static CHOSEN: std::sync::RwLock<Option<Choice>> = std::sync::RwLock::new(None);

/// メインウィンドウと別ウィンドウは別のコンテキストなので、コンテキストごとに入れたバージョンを記録して比べる。
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn choose(choice: Choice) {
    let mut chosen = CHOSEN.write().unwrap_or_else(|e| e.into_inner());
    if chosen.as_ref() == Some(&choice) {
        return;
    }
    *chosen = Some(choice);
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn refresh(context: &egui::Context, seen: &mut u64) {
    let now = generation();
    if *seen != now {
        *seen = now;
        install(context);
    }
}

pub fn chosen() -> Choice {
    CHOSEN
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| Choice::of(&crate::config::load_plain()))
}

/// 一度読んだら手放さず、どのコンテキストもこの一つを指す（10 MB を超えるため）。
#[derive(Debug, Clone, Copy)]
struct Loaded {
    bytes: &'static [u8],
    index: u32,
}

struct Base {
    body: Option<(&'static str, String, Loaded)>,
    symbols: Option<(&'static str, Loaded)>,
}

static BASE: std::sync::OnceLock<Base> = std::sync::OnceLock::new();

fn base() -> &'static Base {
    BASE.get_or_init(|| {
        let started = std::time::Instant::now();
        let ui = system::ui_family();
        let fallbacks = system::japanese_fallbacks(ui.as_deref());
        // 前に決めた物を、OS の UI フォントと代替の並びが同じでファイルが残っていれば使う（一覧の作成は遅い）。
        if let Some(remembered) = Remembered::read()
            && remembered.ui == ui.clone().unwrap_or_default()
            && remembered.fallbacks == fallbacks
            && let Some(base) = remembered.load()
        {
            tracing::info!("フォントの選択 {:?}（前回の結果を使用）", started.elapsed());
            return base;
        }
        let catalog = zwsdk::host::fonts::system();
        let body = japanese_body(catalog, ui.as_deref(), &fallbacks);
        let symbols = symbol_face(catalog);
        let remembered = Remembered {
            ui: ui.unwrap_or_default(),
            fallbacks,
            body: body
                .as_ref()
                .map(|(how, name, face)| Face::of(face, how, name)),
            symbols: symbols
                .as_ref()
                .map(|(name, face)| Face::of(face, "", name)),
        };
        remembered.write();
        let base = Base {
            body: body.and_then(|(how, name, face)| Some((how, name, load_face(face)?))),
            symbols: symbols.and_then(|(name, face)| {
                let name: &'static str = Box::leak(name.into_boxed_str());
                Some((name, load_face(face)?))
            }),
        };
        tracing::info!("フォントの選択 {:?}", started.elapsed());
        base
    })
}

/// OS のテキストコントロールに渡す UI フォント。UI フォントを選んでいなければ `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFace {
    pub family: String,
    pub weight: u16,
    pub postscript: String,
}

pub fn native_face() -> Option<NativeFace> {
    // 毎フレーム問い合わせが来るので、フォントの選択が変わったとき（世代）だけ取得し直す。
    static MADE: std::sync::RwLock<Option<(u64, Option<NativeFace>)>> =
        std::sync::RwLock::new(None);
    let now = generation();
    if let Some((seen, face)) = MADE.read().unwrap_or_else(|e| e.into_inner()).as_ref()
        && *seen == now
    {
        return face.clone();
    }
    let face = look_up_native();
    *MADE.write().unwrap_or_else(|e| e.into_inner()) = Some((now, face.clone()));
    face
}

fn look_up_native() -> Option<NativeFace> {
    let name = chosen().ui;
    if name.trim().is_empty() {
        return None;
    }
    let face = zwsdk::host::fonts::system().find(&name)?;
    Some(NativeFace {
        family: face.family.clone(),
        weight: face.weight,
        postscript: face.postscript.clone(),
    })
}

pub fn body_name() -> Option<String> {
    BASE.get()?.body.as_ref().map(|(_, name, _)| name.clone())
}

/// 決めたフォントの記録（`state.toml` の `[fonts]`）。
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
struct Remembered {
    ui: String,
    fallbacks: Vec<String>,
    body: Option<Face>,
    symbols: Option<Face>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Face {
    how: String,
    family: String,
    path: std::path::PathBuf,
    index: u32,
}

impl Face {
    fn of(face: &zwsdk::host::fonts::Face, how: &str, family: &str) -> Self {
        Self {
            how: how.to_owned(),
            family: family.to_owned(),
            path: face.path.clone(),
            index: face.index,
        }
    }

    fn load(&self) -> Option<Loaded> {
        let mut bytes = std::fs::read(&self.path).ok()?;
        close_line_gap(&mut bytes);
        Some(Loaded {
            bytes: Box::leak(bytes.into_boxed_slice()),
            index: self.index,
        })
    }
}

impl Remembered {
    fn read() -> Option<Self> {
        crate::config::load_state().fonts?.try_into().ok()
    }

    fn write(&self) {
        let table = toml::Table::try_from(self).ok();
        crate::config::update_state(move |state| state.fonts = table);
    }

    fn load(&self) -> Option<Base> {
        let body = self.body.as_ref()?;
        let how: &'static str = Box::leak(body.how.clone().into_boxed_str());
        let symbols = self.symbols.as_ref().and_then(|symbols| {
            let name: &'static str = Box::leak(symbols.family.clone().into_boxed_str());
            Some((name, symbols.load()?))
        });
        Some(Base {
            body: Some((how, body.family.clone(), body.load()?)),
            symbols,
        })
    }
}

/// OS の UI フォント、OS の日本語の代替、一覧の順に探し、仮名と漢字を表示できるか確かめて使う。
fn japanese_body<'c>(
    catalog: &'c Catalog,
    ui: Option<&str>,
    fallbacks: &[String],
) -> Option<(&'static str, String, &'c CatalogFace)> {
    let asked = ui
        .map(|name| ("OS の UI フォント", name.to_owned()))
        .into_iter()
        .chain(
            fallbacks
                .iter()
                .map(|name| ("OS の代替フォント", name.clone())),
        );
    for (how, name) in asked {
        if let Some(face) = catalog.find(&name)
            && face.japanese
        {
            return Some((how, name, face));
        }
    }
    let (name, face) = scan_japanese(catalog)?;
    Some(("フォントの一覧", name, face))
}

/// 中国語のフォントより日本語、明朝よりゴシック、仮名を詰めた「UI」、ふつうの太さに近い物。
fn scan_japanese(catalog: &Catalog) -> Option<(String, &CatalogFace)> {
    catalog
        .faces()
        .iter()
        .filter(|face| {
            !face.italic && !face.monospaced && face.family_ja.is_some() && face.japanese
        })
        .min_by_key(|face| {
            let names = format!(
                "{} {}",
                face.family,
                face.family_ja.as_deref().unwrap_or_default()
            );
            let serif =
                names.contains("Mincho") || names.contains("明朝") || names.contains("Serif");
            let not_ui = !names.split(' ').any(|word| word == "UI");
            (chinese(&names), serif, not_ui, face.weight.abs_diff(400))
        })
        .map(|face| (face.family.clone(), face))
}

/// 中国語のフォントか（日本語の名前も持つが字形が違う）。
fn chinese(names: &str) -> bool {
    names
        .split(' ')
        .any(|word| matches!(word, "TC" | "SC" | "HK" | "CN" | "TW"))
        || [
            "Heiti", "Songti", "Kaiti", "PingFang", "STSong", "SimSun", "SimHei", "YaHei",
            "JhengHei",
        ]
        .iter()
        .any(|name| names.contains(name))
}

/// 記号のフォント：一覧の中で [`SYMBOLS`] を一番多く表示できるフォント（三文字以上）。
fn symbol_face(catalog: &Catalog) -> Option<(String, &CatalogFace)> {
    // 同じ数なら日本語の名前を持たないフォントを先に（字形が本文のフォントと競合する）。
    let (count, _, face) = catalog
        .faces()
        .iter()
        .map(|face| (face.count_covered(SYMBOLS), face.family_ja.is_none(), face))
        .max_by_key(|(count, not_japanese, _)| (*count, *not_japanese))?;
    if count < 3 {
        return None;
    }
    Some((face.family.clone(), face))
}

/// lineGap を零にして保持し続ける。ファイルごとに一つで、選び直しても読み直さない（TTC は 10〜30 MB）。
fn load_face(face: &CatalogFace) -> Option<Loaded> {
    static FILES: std::sync::Mutex<std::collections::BTreeMap<std::path::PathBuf, &'static [u8]>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = match files.get(&face.path) {
        Some(bytes) => *bytes,
        None => {
            let mut bytes = std::fs::read(&face.path).ok()?;
            close_line_gap(&mut bytes);
            let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            files.insert(face.path.clone(), leaked);
            leaked
        }
    };
    Some(Loaded {
        bytes,
        index: face.index,
    })
}

/// 日本語・英語のフォント名、ファミリー名で読む。一度読んだ物はキャッシュする。
fn load_family(name: &str) -> Option<Loaded> {
    static LOADED: std::sync::Mutex<std::collections::BTreeMap<String, Option<Loaded>>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = loaded.get(name) {
        return *known;
    }
    let found = zwsdk::host::fonts::system().find(name).and_then(load_face);
    if found.is_none() {
        tracing::warn!("フォント「{name}」が見つからない（既定のフォントを使う）");
    }
    loaded.insert(name.to_owned(), found);
    found
}

fn data_of(loaded: Loaded, scale: f32) -> Arc<egui::FontData> {
    let mut data = egui::FontData::from_static(loaded.bytes);
    data.index = loaded.index;
    data.tweak.scale = scale;
    Arc::new(data)
}

fn scale_of(size: u32) -> f32 {
    if size == 0 {
        1.0
    } else {
        size as f32 / REFERENCE_PT
    }
}

/// 見つからなければ `None` で、日本語は出ない。
pub fn install(context: &egui::Context) -> Option<(&'static str, String)> {
    install_with(context, &chosen())
}

pub fn install_with(context: &egui::Context, choice: &Choice) -> Option<(&'static str, String)> {
    let base = base();
    let mut fonts = egui::FontDefinitions::default();
    let egui_proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    // 日本語のほうを先に。ラテン文字を egui のフォントから取ると、同じ行で太さと基線が揃わない。
    let mut fallback: Vec<String> = Vec::new();
    if let Some((_, _, body)) = base.body {
        fonts
            .font_data
            .insert("japanese".to_owned(), data_of(body, 1.0));
        fallback.push("japanese".to_owned());
        // 等幅は egui のフォントを先に（時刻と PID の数字で、日本語は混じらない）。
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(1, "japanese".to_owned());
    }
    fallback.extend(egui_proportional);
    if let Some((_, symbols)) = base.symbols {
        fonts
            .font_data
            .insert("symbols".to_owned(), data_of(symbols, 1.0));
        fallback.push("symbols".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("symbols".to_owned());
    }
    // 大きさだけを変えるときは、本文のフォントを倍率付きで別に置く。
    let scaled_body = |fonts: &mut egui::FontDefinitions, tag: &str, size: u32| -> Vec<String> {
        let mut list = fallback.clone();
        if size != 0
            && let Some((_, _, body)) = base.body
        {
            fonts
                .font_data
                .insert(tag.to_owned(), data_of(body, scale_of(size)));
            list[0] = tag.to_owned();
        }
        list
    };
    let with_chosen =
        |fonts: &mut egui::FontDefinitions, tag: &str, name: &str, size: u32, rest: Vec<String>| {
            let mut list = Vec::new();
            if !name.is_empty()
                && let Some(loaded) = load_family(name)
            {
                fonts
                    .font_data
                    .insert(tag.to_owned(), data_of(loaded, scale_of(size)));
                list.push(tag.to_owned());
            }
            list.extend(rest);
            list
        };
    // 用途ごとに選んでいなければ、UI に選んだフォントを先に。
    let ui_font = (!choice.ui.is_empty())
        .then(|| load_family(&choice.ui))
        .flatten();
    let based = |fonts: &mut egui::FontDefinitions, tag: &str, size: u32| -> Vec<String> {
        let mut list = scaled_body(fonts, &format!("{tag}-body"), size);
        if let Some(loaded) = ui_font {
            fonts
                .font_data
                .insert(tag.to_owned(), data_of(loaded, scale_of(size)));
            list.insert(0, tag.to_owned());
        }
        list
    };
    let ui_rest = scaled_body(&mut fonts, "japanese-ui", choice.ui_size);
    let ui = with_chosen(&mut fonts, "chosen-ui", &choice.ui, choice.ui_size, ui_rest);
    let status = with_chosen(&mut fonts, "chosen-status", &choice.status, 0, ui.clone());
    fonts
        .families
        .insert(egui::FontFamily::Name(STATUS.into()), status);
    fonts.families.insert(egui::FontFamily::Proportional, ui);
    let osd_rest = based(&mut fonts, "ui-osd", 0);
    let osd = with_chosen(&mut fonts, "chosen-osd", &choice.osd, 0, osd_rest);
    fonts
        .families
        .insert(egui::FontFamily::Name(OSD.into()), osd);
    let caption_default = based(&mut fonts, "ui-caption", 0);
    let caption_rest = if choice.fallback {
        caption_default.clone()
    } else {
        Vec::new()
    };
    let mut caption = with_chosen(
        &mut fonts,
        "chosen-caption",
        &choice.caption,
        0,
        caption_rest.clone(),
    );
    if caption.is_empty() {
        caption = caption_default.clone();
    }
    let superimpose_name = if choice.superimpose.is_empty() {
        &choice.caption
    } else {
        &choice.superimpose
    };
    let mut superimpose = with_chosen(
        &mut fonts,
        "chosen-superimpose",
        superimpose_name,
        0,
        caption_rest,
    );
    if superimpose.is_empty() {
        superimpose = caption_default;
    }
    fonts
        .families
        .insert(egui::FontFamily::Name(CAPTION.into()), caption);
    fonts
        .families
        .insert(egui::FontFamily::Name(SUPERIMPOSE.into()), superimpose);
    let guide_rest = based(&mut fonts, "ui-guide", choice.guide_size);
    let guide = with_chosen(
        &mut fonts,
        "chosen-guide",
        &choice.guide,
        choice.guide_size,
        guide_rest,
    );
    fonts
        .families
        .insert(egui::FontFamily::Name(GUIDE.into()), guide);
    context.set_fonts(fonts);
    base.body
        .as_ref()
        .map(|(path, family, _)| (*path, family.clone()))
}

pub fn osd(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(OSD.into()))
}

pub fn caption(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(CAPTION.into()))
}

pub fn superimpose(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(SUPERIMPOSE.into()))
}

pub fn status(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(STATUS.into()))
}

pub fn guide(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(GUIDE.into()))
}

pub fn install_or_say(context: &egui::Context) {
    match install(context) {
        Some((path, family)) if family.is_empty() => tracing::info!("フォント {path}"),
        Some((path, family)) => tracing::info!("フォント {path}（{family}）"),
        None => {
            tracing::warn!("日本語のフォントが見つからない（番組名やメニューの文字が表示されない）")
        }
    }
}

fn face_offsets(bytes: &[u8]) -> Vec<usize> {
    if bytes.starts_with(b"ttcf") {
        let count = read_u32(bytes, 8).unwrap_or(0) as usize;
        (0..count)
            .filter_map(|index| read_u32(bytes, 12 + index * 4).map(|at| at as usize))
            .collect()
    } else {
        vec![0]
    }
}

fn table(bytes: &[u8], face: usize, tag: &[u8; 4]) -> Option<usize> {
    let tables = read_u16(bytes, face + 4)? as usize;
    let record = (0..tables)
        .map(|table| face + 12 + table * 16)
        .find(|at| bytes.get(*at..*at + 4) == Some(tag))?;
    Some(read_u32(bytes, record + 8)? as usize)
}

/// lineGap を零にする。egui のレイアウトの箱は lineGap を含み文字が上に寄るため。チェックサムは直さない。
fn close_line_gap(bytes: &mut [u8]) -> usize {
    let mut closed = 0;
    for face in face_offsets(bytes) {
        // hhea: ascender +4、descender +6、lineGap +8。
        if let Some(hhea) = table(bytes, face, b"hhea")
            && let Some(gap) = bytes.get_mut(hhea + 8..hhea + 10)
        {
            gap.copy_from_slice(&[0, 0]);
            closed += 1;
        }
        // OS/2: sTypoAscender +68、sTypoDescender +70、sTypoLineGap +72。
        if let Some(os2) = table(bytes, face, b"OS/2")
            && let Some(gap) = bytes.get_mut(os2 + 72..os2 + 74)
        {
            gap.copy_from_slice(&[0, 0]);
            closed += 1;
        }
    }
    closed
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *bytes.get(at)?,
        *bytes.get(at + 1)?,
        *bytes.get(at + 2)?,
        *bytes.get(at + 3)?,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_faces_are_told_from_japanese_ones() {
        for name in [
            "Heiti TC",
            "PingFang SC",
            "Songti TC",
            "Microsoft YaHei",
            "Noto Sans CJK SC",
        ] {
            assert!(chinese(name), "{name}");
        }
        for name in [
            "Hiragino Sans",
            "Yu Gothic UI",
            "Noto Sans CJK JP",
            "Meiryo UI",
            "IPAexGothic",
        ] {
            assert!(!chinese(name), "{name}");
        }
    }

    #[test]
    fn the_line_gap_is_closed_in_both_tables() {
        let mut out = Vec::new();
        out.extend_from_slice(&[0, 1, 0, 0]);
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let hhea_at = 12 + 16 * 2;
        let os2_at = hhea_at + 36;
        for (tag, at, len) in [(b"hhea", hhea_at, 36u32), (b"OS/2", os2_at, 78)] {
            out.extend_from_slice(tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&(at as u32).to_be_bytes());
            out.extend_from_slice(&len.to_be_bytes());
        }
        let mut hhea = vec![0u8; 36];
        hhea[4..6].copy_from_slice(&880i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-120i16).to_be_bytes());
        hhea[8..10].copy_from_slice(&500i16.to_be_bytes());
        let mut os2 = vec![0u8; 78];
        os2[68..70].copy_from_slice(&880i16.to_be_bytes());
        os2[72..74].copy_from_slice(&500i16.to_be_bytes());
        out.extend_from_slice(&hhea);
        out.extend_from_slice(&os2);
        assert_eq!(close_line_gap(&mut out), 2);
        assert_eq!(read_u16(&out, hhea_at + 8), Some(0));
        assert_eq!(read_u16(&out, os2_at + 72), Some(0));
        assert_eq!(read_u16(&out, hhea_at + 4), Some(880));
        assert_eq!(read_u16(&out, os2_at + 68), Some(880));
    }
}
