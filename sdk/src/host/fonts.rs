// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! OS のフォントの一覧と、名前からファイル（と TTC の何番目か）を検索する仕組み。グリフは読まない。
//! フォント名・ファミリー名・ファミリー名＋ウェイトのどれでも、日本語でも英語でも検索できる。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use skrifa::raw::TableProvider;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

const JAPANESE: [char; 3] = ['あ', 'ア', '漢'];

#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub path: PathBuf,
    pub index: u32,
    /// 英語。name の 16 番、無ければ 1 番。
    pub family: String,
    pub family_ja: Option<String>,
    /// 英語。17 番、無ければ 2 番。
    pub style: String,
    pub style_ja: Option<String>,
    /// name の 4 番。
    pub full: String,
    pub full_ja: Option<String>,
    pub postscript: String,
    /// OS/2 の usWeightClass（可変フォントは既定の位置）。
    pub weight: u16,
    pub italic: bool,
    pub monospaced: bool,
    pub japanese: bool,
    pub axes: Vec<Axis>,
    pub instances: Vec<Instance>,
    /// 正規化した形：フォント名、ファミリー名＋ウェイト、PostScript 名、名前付きインスタンス。
    names: Vec<String>,
    /// 正規化した形：1 番と 16 番、全部の言語。
    families: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub tag: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub name: String,
    pub name_ja: Option<String>,
    pub coords: Vec<f32>,
}

impl Face {
    /// 日本語のフォント名があればそれ。設定にはこれを書く。
    pub fn name(&self) -> &str {
        self.full_ja.as_deref().unwrap_or(&self.full)
    }

    pub fn family_name(&self) -> &str {
        self.family_ja.as_deref().unwrap_or(&self.family)
    }

    pub fn style_name(&self) -> &str {
        self.style_ja.as_deref().unwrap_or(&self.style)
    }

    pub fn is_bold(&self) -> bool {
        self.weight >= 600
    }

    pub fn variable(&self) -> bool {
        self.axes.iter().any(|axis| axis.tag == "wght")
    }

    fn called(&self, wanted: &str) -> bool {
        self.names.iter().any(|name| name == wanted)
    }

    fn in_family(&self, wanted: &str) -> bool {
        self.families.iter().any(|name| name == wanted)
    }

    pub fn bytes(&self) -> Option<Vec<u8>> {
        let bytes = std::fs::read(&self.path).ok()?;
        if bytes.starts_with(b"ttcf") {
            crate::fonts::single_face(&bytes, self.index)
        } else {
            Some(bytes)
        }
    }

    pub fn count_covered(&self, letters: &str) -> usize {
        with_file(&self.path, |bytes| {
            FontRef::from_index(bytes, self.index).map_or(0, |font| {
                let charmap = font.charmap();
                letters
                    .chars()
                    .filter(|letter| charmap.map(*letter).is_some())
                    .count()
            })
        })
        .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    faces: Vec<Face>,
    pub took: std::time::Duration,
}

impl Catalog {
    pub fn system() -> Self {
        Self::of_files(font_files(&folders()))
    }

    pub fn of_files(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let started = std::time::Instant::now();
        let mut paths: Vec<PathBuf> = paths.into_iter().collect();
        paths.sort();
        paths.dedup();
        let faces = paths
            .into_iter()
            .flat_map(|path| with_file(&path, |bytes| faces_of(&path, bytes)).unwrap_or_default())
            .collect();
        Self {
            faces,
            took: started.elapsed(),
        }
    }

    pub fn faces(&self) -> &[Face] {
        &self.faces
    }

    /// フォント名・PostScript 名、ファミリー名（標準のウェイト）、ファミリー名＋ウェイトの順。
    pub fn find(&self, name: &str) -> Option<&Face> {
        let wanted = key(name);
        if wanted.is_empty() {
            return None;
        }
        if let Some(face) = self
            .faces
            .iter()
            .filter(|face| face.called(&wanted))
            .min_by_key(|face| face.italic)
        {
            return Some(face);
        }
        if let Some(face) = self.in_family(&wanted, 400, false) {
            return Some(face);
        }
        let (rest, last) = name.trim().rsplit_once(char::is_whitespace)?;
        let weight = weight_of(last)?;
        self.in_family(&key(rest), weight, false)
    }

    pub fn family(&self, name: &str, weight: u16, italic: bool) -> Option<&Face> {
        self.in_family(&key(name), weight, italic)
    }

    fn in_family(&self, wanted: &str, weight: u16, italic: bool) -> Option<&Face> {
        self.faces
            .iter()
            .filter(|face| face.in_family(wanted))
            .min_by_key(|face| (face.italic != italic, face.weight.abs_diff(weight)))
    }

    pub fn families(&self) -> Vec<(String, Vec<&Face>)> {
        let mut families: Vec<(String, Vec<&Face>)> = Vec::new();
        for face in &self.faces {
            let name = face.family_name();
            match families.iter_mut().find(|(had, _)| had == name) {
                Some((_, faces)) => faces.push(face),
                None => families.push((name.to_owned(), vec![face])),
            }
        }
        for (_, faces) in &mut families {
            faces.sort_by_key(|face| (face.italic, face.weight, face.name().to_owned()));
            faces.dedup_by(|a, b| key(a.name()) == key(b.name()));
        }
        families.sort_by_key(|family| family.0.to_lowercase());
        families
    }
}

static SYSTEM: OnceLock<Catalog> = OnceLock::new();

/// 作っている途中なら待つ。一度作ったら作り直さない。
pub fn system() -> &'static Catalog {
    SYSTEM.get_or_init(|| {
        let catalog = Catalog::system();
        tracing::info!(
            "フォント一覧: {} 個（{:?}）",
            catalog.faces.len(),
            catalog.took
        );
        catalog
    })
}

/// 起動を待たせないよう別のスレッドで作り始める。
pub fn warm() {
    if SYSTEM.get().is_some() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name(String::from("フォントの一覧"))
        .spawn(|| {
            system();
        });
    if let Err(error) = spawned {
        tracing::warn!("フォント一覧のスレッドを起動できません: {error}");
    }
}

pub fn ready() -> Option<&'static Catalog> {
    SYSTEM.get()
}

/// OS のフォントフォルダー以下のパスかフォント名。名前で検索したものはそのフォントだけのファイルにする。
pub fn lend(name: &str) -> Option<Vec<u8>> {
    let path = Path::new(name);
    if path.is_absolute() {
        return (is_font_file(path) && under_any(path, &folders()))
            .then(|| std::fs::read(path).ok())
            .flatten();
    }
    system().find(name)?.bytes()
}

/// `kind = "font"` の値。ファイルのパスならそのファイル、そうでなければ名前で検索する。
pub fn lend_chosen(value: &str) -> Option<Vec<u8>> {
    let value = value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value)
        .trim();
    if value.is_empty() {
        return None;
    }
    let path = Path::new(value);
    if path.is_file() {
        return std::fs::read(path).ok();
    }
    system().find(value)?.bytes()
}

/// 全角の英数字と空白を半角に、小文字に、空白・`-`・`_` を除く。
pub fn key(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn weight_of(word: &str) -> Option<u16> {
    let word = key(word);
    let named = match word.as_str() {
        "thin" | "hairline" => 100,
        "extralight" | "ultralight" => 200,
        "light" | "細字" => 300,
        "semilight" => 350,
        "regular" | "normal" | "book" | "roman" | "標準" => 400,
        "medium" => 500,
        "semibold" | "demibold" | "demi" => 600,
        "bold" | "太字" => 700,
        "extrabold" | "ultrabold" => 800,
        "black" | "heavy" => 900,
        "extrablack" | "ultrablack" => 950,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(step) = word.strip_prefix('w')
        && let Ok(step @ 0..=9) = step.parse::<u16>()
    {
        return Some(step * 100);
    }
    word.parse::<u16>().ok().filter(|n| (1..=1000).contains(n))
}

fn faces_of(path: &Path, bytes: &[u8]) -> Vec<Face> {
    let count = if bytes.starts_with(b"ttcf") {
        skrifa::raw::CollectionRef::new(bytes).map_or(0, |collection| collection.len())
    } else {
        1
    };
    (0..count)
        .filter_map(|index| {
            let font = FontRef::from_index(bytes, index).ok()?;
            face_of(path, index, &font)
        })
        .collect()
}

fn strings(font: &FontRef<'_>, id: StringId) -> Vec<(String, String)> {
    font.localized_strings(id)
        .filter_map(|string| {
            let value = string.to_string();
            let value = value.trim();
            if value.is_empty() || value.contains('\u{FFFD}') || value.contains('\0') {
                return None;
            }
            Some((string.language().unwrap_or("").to_owned(), value.to_owned()))
        })
        .collect()
}

fn english_and_japanese(strings: &[(String, String)]) -> (Option<String>, Option<String>) {
    let english = strings
        .iter()
        .find(|(language, _)| language.starts_with("en"))
        .or_else(|| strings.first())
        .map(|(_, value)| value.clone());
    let japanese = strings
        .iter()
        .find(|(language, _)| language.starts_with("ja"))
        .map(|(_, value)| value.clone())
        .filter(|value| Some(value) != english.as_ref());
    (english, japanese)
}

fn face_of(path: &Path, index: u32, font: &FontRef<'_>) -> Option<Face> {
    let typographic = strings(font, StringId::TYPOGRAPHIC_FAMILY_NAME);
    let legacy = strings(font, StringId::FAMILY_NAME);
    let typographic_style = strings(font, StringId::TYPOGRAPHIC_SUBFAMILY_NAME);
    let legacy_style = strings(font, StringId::SUBFAMILY_NAME);
    let full_names = strings(font, StringId::FULL_NAME);
    let postscript = strings(font, StringId::POSTSCRIPT_NAME)
        .into_iter()
        .next()
        .map(|(_, value)| value)
        .unwrap_or_default();
    let (family, family_ja) = english_and_japanese(if typographic.is_empty() {
        &legacy
    } else {
        &typographic
    });
    let family = family?;
    let (style, style_ja) = english_and_japanese(if typographic_style.is_empty() {
        &legacy_style
    } else {
        &typographic_style
    });
    let style = style.unwrap_or_default();
    let (full, full_ja) = english_and_japanese(&full_names);
    let full = full.unwrap_or_else(|| {
        if style.is_empty() {
            family.clone()
        } else {
            format!("{family} {style}")
        }
    });
    let attributes = font.attributes();
    let monospaced = font.post().is_ok_and(|post| post.is_fixed_pitch() != 0);
    let charmap = font.charmap();
    let japanese = JAPANESE.iter().all(|letter| charmap.map(*letter).is_some());
    let axes: Vec<Axis> = font
        .axes()
        .iter()
        .map(|axis| Axis {
            tag: axis.tag().to_string(),
            min: axis.min_value(),
            default: axis.default_value(),
            max: axis.max_value(),
        })
        .collect();
    let mut instances = Vec::new();
    for instance in font.named_instances().iter() {
        let names = strings(font, instance.subfamily_name_id());
        let (Some(name), name_ja) = english_and_japanese(&names) else {
            continue;
        };
        instances.push((
            Instance {
                name,
                name_ja,
                coords: instance.user_coords().collect(),
            },
            names,
        ));
    }
    // ファミリー名とウェイト名は言語を跨いで組み合わせる（「游ゴシック Bold」）。
    let mut families: Vec<String> = typographic
        .iter()
        .chain(&legacy)
        .map(|(_, value)| key(value))
        .collect();
    families.sort();
    families.dedup();
    let mut names: Vec<String> = full_names.iter().map(|(_, value)| key(value)).collect();
    names.push(key(&postscript));
    let styles: Vec<String> = typographic_style
        .iter()
        .chain(&legacy_style)
        .map(|(_, value)| key(value))
        .collect();
    for family in &families {
        for style in &styles {
            names.push(format!("{family}{style}"));
        }
        for (_, instance_names) in &instances {
            for (_, name) in instance_names {
                names.push(format!("{family}{}", key(name)));
            }
        }
    }
    names.retain(|name| !name.is_empty());
    names.sort();
    names.dedup();
    Some(Face {
        path: path.to_owned(),
        index,
        family,
        family_ja,
        style,
        style_ja,
        full,
        full_ja,
        postscript,
        weight: attributes.weight.value().round().clamp(1.0, 1000.0) as u16,
        italic: !matches!(attributes.style, skrifa::attribute::Style::Normal),
        monospaced,
        japanese,
        axes,
        instances: instances
            .into_iter()
            .map(|(instance, _)| instance)
            .collect(),
        names,
        families,
    })
}

fn with_file<T>(path: &Path, read: impl FnOnce(&[u8]) -> T) -> Option<T> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: 読むだけ。OS のフォントは使っている間は書き換えられない。
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(read(&map))
}

pub fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// `..` や別名で外へ出られないよう、両方を解いて比べる。
pub fn under_any(path: &Path, folders: &[PathBuf]) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    folders
        .iter()
        .filter_map(|folder| folder.canonicalize().ok())
        .any(|folder| path.starts_with(folder))
}

pub fn folders() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let home = var("HOME");
    let mut folders = Vec::new();
    if cfg!(target_os = "windows") {
        let windows = var("WINDIR")
            .or_else(|| var("SystemRoot"))
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        folders.push(windows.join("Fonts"));
        if let Some(local) = var("LOCALAPPDATA") {
            folders.push(local.join(r"Microsoft\Windows\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        folders.extend(
            [
                "/System/Library/Fonts",
                "/Library/Fonts",
                "/Network/Library/Fonts",
            ]
            .map(PathBuf::from),
        );
        // 後からダウンロードしたフォント（ヒラギノ丸ゴなど）。他のアセットは見ない。
        for assets in ["/System/Library/AssetsV2", "/System/Library/Assets"] {
            if let Ok(entries) = std::fs::read_dir(assets) {
                folders.extend(
                    entries
                        .flatten()
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with("com_apple_MobileAsset_Font")
                        })
                        .map(|entry| entry.path()),
                );
            }
        }
        if let Some(home) = &home {
            folders.push(home.join("Library/Fonts"));
        }
    } else {
        folders.extend(
            [
                "/usr/share/fonts",
                "/usr/local/share/fonts",
                "/system/fonts",
            ]
            .map(PathBuf::from),
        );
        let data_home =
            var("XDG_DATA_HOME").or_else(|| home.as_ref().map(|home| home.join(".local/share")));
        if let Some(data_home) = &data_home {
            folders.push(data_home.join("fonts"));
        }
        if let Some(home) = &home {
            folders.push(home.join(".fonts"));
        }
        if let Some(dirs) = std::env::var_os("XDG_DATA_DIRS") {
            folders.extend(std::env::split_paths(&dirs).map(|dir| dir.join("fonts")));
        }
        folders.extend(fontconfig_dirs(home.as_deref(), data_home.as_deref()));
    }
    let mut seen = Vec::new();
    folders.retain(|folder| {
        let new = !seen.contains(folder);
        seen.push(folder.clone());
        new
    });
    folders
}

fn fontconfig_dirs(home: Option<&Path>, data_home: Option<&Path>) -> Vec<PathBuf> {
    let mut files = vec![PathBuf::from("/etc/fonts/fonts.conf")];
    if let Ok(entries) = std::fs::read_dir("/etc/fonts/conf.d") {
        let mut more: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        more.sort();
        files.extend(more);
    }
    let mut dirs = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut rest = text.as_str();
        while let Some(start) = rest.find("<dir") {
            rest = &rest[start + 4..];
            let Some(close) = rest.find('>') else {
                break;
            };
            let attributes = &rest[..close];
            if attributes.ends_with('/') || !(attributes.is_empty() || attributes.starts_with(' '))
            {
                continue;
            }
            rest = &rest[close + 1..];
            let Some(end) = rest.find("</dir>") else {
                break;
            };
            let dir = rest[..end].trim();
            rest = &rest[end..];
            let path = if attributes.contains("prefix=\"xdg\"") {
                data_home.map(|base| base.join(dir))
            } else if let Some(under) = dir.strip_prefix("~/") {
                home.map(|home| home.join(under))
            } else {
                Some(PathBuf::from(dir))
            };
            dirs.extend(path.filter(|path| path.is_absolute()));
        }
    }
    dirs
}

pub fn font_files(folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<(PathBuf, u32)> = folders.iter().map(|folder| (folder.clone(), 0)).collect();
    while let Some((folder, depth)) = stack.pop() {
        let Ok(real) = folder.canonicalize() else {
            continue;
        };
        if depth > 8 || !seen.insert(real) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = std::fs::metadata(&path) else {
                continue;
            };
            if kind.is_dir() {
                stack.push((path, depth + 1));
            } else if is_font_file(&path) {
                files.push(path);
            }
        }
    }
    files
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = tables.to_vec();
        tables.sort_by_key(|(tag, _)| **tag);
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let mut offset = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in &tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            body.extend_from_slice(data);
            let padded = data.len().next_multiple_of(4);
            body.resize(body.len() + padded - data.len(), 0);
            offset += padded;
        }
        out.extend_from_slice(&body);
        out
    }

    pub fn ttc(faces: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"ttcf\0\x01\0\0".to_vec();
        out.extend_from_slice(&(faces.len() as u32).to_be_bytes());
        let mut at = 12 + 4 * faces.len();
        for face in faces {
            out.extend_from_slice(&(at as u32).to_be_bytes());
            at += face.len();
        }
        let mut base = 12 + 4 * faces.len();
        for face in faces {
            // テーブルのオフセットはファイルの先頭から数えるので、フォントの位置の分だけずらす。
            let mut face = face.clone();
            let tables = u16::from_be_bytes([face[4], face[5]]) as usize;
            for n in 0..tables {
                let record = 12 + 16 * n + 8;
                let offset = u32::from_be_bytes(face[record..record + 4].try_into().unwrap());
                face[record..record + 4].copy_from_slice(&(offset + base as u32).to_be_bytes());
            }
            base += face.len();
            out.extend_from_slice(&face);
        }
        out
    }

    pub fn name_table(records: &[(u16, u16, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(records.len() as u16).to_be_bytes());
        out.extend_from_slice(&((6 + 12 * records.len()) as u16).to_be_bytes());
        let mut strings = Vec::new();
        for (language, id, text) in records {
            let encoded: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            for value in [
                3u16,
                1,
                *language,
                *id,
                encoded.len() as u16,
                strings.len() as u16,
            ] {
                out.extend_from_slice(&value.to_be_bytes());
            }
            strings.extend_from_slice(&encoded);
        }
        out.extend_from_slice(&strings);
        out
    }

    pub fn os2(weight: u16, italic: bool) -> Vec<u8> {
        let mut out = vec![0u8; 96];
        out[0..2].copy_from_slice(&4u16.to_be_bytes());
        out[4..6].copy_from_slice(&weight.to_be_bytes());
        out[6..8].copy_from_slice(&5u16.to_be_bytes());
        let selection: u16 = if italic { 1 } else { 0x40 };
        out[62..64].copy_from_slice(&selection.to_be_bytes());
        out
    }

    pub fn cmap(letters: &[char]) -> Vec<u8> {
        let mut letters = letters.to_vec();
        letters.sort();
        let mut sub = Vec::new();
        sub.extend_from_slice(&12u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&((16 + 12 * letters.len()) as u32).to_be_bytes());
        sub.extend_from_slice(&0u32.to_be_bytes());
        sub.extend_from_slice(&(letters.len() as u32).to_be_bytes());
        for (n, letter) in letters.iter().enumerate() {
            sub.extend_from_slice(&(*letter as u32).to_be_bytes());
            sub.extend_from_slice(&(*letter as u32).to_be_bytes());
            sub.extend_from_slice(&(n as u32 + 1).to_be_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&10u16.to_be_bytes());
        out.extend_from_slice(&12u32.to_be_bytes());
        out.extend_from_slice(&sub);
        out
    }

    pub fn fvar(min: f32, default: f32, max: f32, instances: &[(u16, f32)]) -> Vec<u8> {
        let fixed = |value: f32| ((value * 65536.0) as i32).to_be_bytes();
        let mut out = Vec::new();
        for value in [1u16, 0, 16, 2, 1, 20, instances.len() as u16, 4 + 4] {
            out.extend_from_slice(&value.to_be_bytes());
        }
        out.extend_from_slice(b"wght");
        out.extend_from_slice(&fixed(min));
        out.extend_from_slice(&fixed(default));
        out.extend_from_slice(&fixed(max));
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&256u16.to_be_bytes());
        for (name, weight) in instances {
            out.extend_from_slice(&name.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&fixed(*weight));
        }
        out
    }

    const EN: u16 = 0x0409;
    const JA: u16 = 0x0411;

    fn yu(weight: u16, style: &str, style_ja: &str) -> Vec<u8> {
        let full = format!("Yu Gothic {style}");
        let full_ja = format!("游ゴシック {style_ja}");
        sfnt(&[
            (
                b"name",
                name_table(&[
                    (EN, 1, "Yu Gothic"),
                    (EN, 2, style),
                    (EN, 4, &full),
                    (EN, 6, &format!("YuGothic-{style}")),
                    (JA, 1, "游ゴシック"),
                    (JA, 2, style_ja),
                    (JA, 4, &full_ja),
                ]),
            ),
            (b"OS/2", os2(weight, false)),
            (b"cmap", cmap(&['A', 'あ', 'ア', '漢'])),
        ])
    }

    fn catalog_of(files: &[(&str, Vec<u8>)]) -> (Catalog, PathBuf) {
        let folder = std::env::temp_dir().join(format!(
            "zw-fonts-{}-{}",
            std::process::id(),
            files.first().map_or("", |(name, _)| name)
        ));
        std::fs::create_dir_all(&folder).unwrap();
        for (name, bytes) in files {
            std::fs::write(folder.join(name), bytes).unwrap();
        }
        (
            Catalog::of_files(font_files(std::slice::from_ref(&folder))),
            folder,
        )
    }

    #[test]
    fn names_are_compared_loosely() {
        assert_eq!(key("ＭＳ ゴシック"), key("MS ゴシック"));
        assert_eq!(key("Yu Gothic UI"), "yugothicui");
        assert_eq!(key("BIZ UDPゴシック"), "bizudpゴシック");
        assert_eq!(weight_of("Bold"), Some(700));
        assert_eq!(weight_of("W6"), Some(600));
        assert_eq!(weight_of("太字"), Some(700));
        assert_eq!(weight_of("350"), Some(350));
        assert_eq!(weight_of("Gothic"), None);
    }

    #[test]
    fn japanese_and_english_names_both_find_the_face() {
        let regular = yu(400, "Regular", "標準");
        let bold = yu(700, "Bold", "太字");
        let (catalog, folder) = catalog_of(&[("yu.ttc", ttc(&[regular, bold]))]);
        assert_eq!(catalog.faces().len(), 2, "TTC の中の二つ");
        let face = &catalog.faces()[1];
        assert_eq!(face.index, 1);
        assert_eq!(face.family, "Yu Gothic");
        assert_eq!(face.family_ja.as_deref(), Some("游ゴシック"));
        assert_eq!(face.full, "Yu Gothic Bold");
        assert_eq!(face.name(), "游ゴシック 太字", "表示名は日本語のフォント名");
        assert_eq!(face.weight, 700);
        assert!(face.is_bold() && !face.italic && face.japanese);
        let index = |name: &str| catalog.find(name).map(|face| face.index);
        assert_eq!(
            index("游ゴシック Bold"),
            Some(1),
            "日本語のファミリー名＋英語のウェイト"
        );
        assert_eq!(index("游ゴシック 太字"), Some(1));
        assert_eq!(index("Yu Gothic Bold"), Some(1));
        assert_eq!(
            index("Yu Gothic 太字"),
            Some(1),
            "英語のファミリー名＋日本語のウェイト"
        );
        assert_eq!(index("YuGothic-Bold"), Some(1), "PostScript 名");
        assert_eq!(index("游ゴシック"), Some(0), "ファミリー名は標準のウェイト");
        assert_eq!(index("yu gothic"), Some(0));
        assert_eq!(
            index("Yu Gothic 800"),
            Some(1),
            "ファミリー名＋ウェイトは近いもの"
        );
        assert_eq!(index("游明朝"), None);
        let lent = catalog.find("游ゴシック Bold").unwrap().bytes().unwrap();
        let single = FontRef::new(&lent).expect("一つのフォント");
        assert_eq!(
            single
                .localized_strings(StringId::FULL_NAME)
                .english_or_first()
                .map(|name| name.to_string())
                .as_deref(),
            Some("Yu Gothic Bold")
        );
        let families = catalog.families();
        assert_eq!(families.len(), 1);
        assert_eq!(families[0].0, "游ゴシック");
        assert_eq!(families[0].1.len(), 2);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn variable_axes_and_named_instances_are_listed() {
        let face = sfnt(&[
            (
                b"name",
                name_table(&[
                    (EN, 1, "Flex Sans"),
                    (EN, 2, "Regular"),
                    (EN, 4, "Flex Sans Regular"),
                    (EN, 256, "Light"),
                    (EN, 257, "Bold"),
                ]),
            ),
            (b"OS/2", os2(400, false)),
            (
                b"fvar",
                fvar(100.0, 400.0, 900.0, &[(256, 300.0), (257, 700.0)]),
            ),
        ]);
        let (catalog, folder) = catalog_of(&[("flex.ttf", face)]);
        let face = catalog.find("Flex Sans").expect("ファミリー名");
        assert!(face.variable());
        assert_eq!(
            face.axes,
            vec![Axis {
                tag: String::from("wght"),
                min: 100.0,
                default: 400.0,
                max: 900.0
            }]
        );
        assert_eq!(face.instances.len(), 2);
        assert_eq!(face.instances[1].name, "Bold");
        assert_eq!(face.instances[1].coords, vec![700.0]);
        assert!(
            catalog.find("Flex Sans Bold").is_some(),
            "名前付きインスタンスでも検索できる"
        );
        assert!(!face.japanese);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_font_is_lent_only_from_the_font_folders_by_path() {
        let folders = [std::env::temp_dir()];
        assert!(!under_any(Path::new("/etc/passwd"), &folders));
        assert!(is_font_file(Path::new("C:\\Windows\\Fonts\\meiryo.TTC")));
        assert!(!is_font_file(Path::new("/etc/passwd")));
        assert_eq!(lend("/etc/passwd"), None);
    }

    #[test]
    #[ignore = "このマシンに入っているフォントに依る（Windows・mac で手動で実行する）"]
    fn the_system_fonts_are_found_by_name() {
        let catalog = Catalog::system();
        eprintln!("{} 個、{:?}", catalog.faces().len(), catalog.took);
        let names: &[&str] = if cfg!(windows) {
            &[
                "游ゴシック Bold",
                "Yu Gothic Bold",
                "游ゴシック",
                "BIZ UDゴシック",
                "BIZ UDPゴシック Bold",
                "メイリオ",
                "Meiryo Bold",
                "MS ゴシック",
                "ＭＳ Ｐゴシック",
                "MS UI Gothic",
                "Yu Gothic UI",
            ]
        } else if cfg!(target_os = "macos") {
            &[
                "ヒラギノ角ゴシック W3",
                "ヒラギノ角ゴシック W6",
                "Hiragino Sans W6",
                "ヒラギノ丸ゴ ProN W4",
                "ヒラギノ明朝 ProN W3",
                "Osaka",
            ]
        } else {
            &["Noto Sans CJK JP", "Noto Sans CJK JP Bold", "DejaVu Sans"]
        };
        let mut missing = Vec::new();
        for name in names {
            match catalog.find(name) {
                Some(face) => eprintln!(
                    "{name} → {}（{}）{}#{} 太さ {}",
                    face.name(),
                    face.full,
                    face.path.display(),
                    face.index,
                    face.weight
                ),
                None => missing.push(*name),
            }
        }
        assert!(missing.is_empty(), "見つからない: {missing:?}");
    }
}
