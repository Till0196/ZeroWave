// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フォント。fontconfig には頼らずファイルを渡す。

use std::sync::Arc;

use parlance::Script;
use parley::FontContext;
use parley::fontique::{
    Blob, Collection, CollectionOptions, FamilyId, FontInfoOverride, GenericFamily, SourceCache,
};

#[derive(Debug, Clone, Default)]
pub struct Faces {
    pub sans: Option<Vec<u8>>,
    pub mono: Option<Vec<u8>>,
}

pub const MONO_ALIAS: &str = "zerowave-bml";

impl Faces {
    pub fn find() -> Self {
        let lent = |name: &str| {
            if cfg!(target_arch = "wasm32") {
                crate::plug::host::font(name)
            } else {
                None
            }
        };
        Self {
            sans: lent("sans").or_else(find_japanese),
            mono: lent("mono").or_else(find_monospace_japanese),
        }
    }

    pub fn len(&self) -> usize {
        usize::from(self.sans.is_some()) + usize::from(self.mono.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 順序は固定する（登録順に依るとレンダリングのたびにグリフが変わる）。
pub fn context(faces: &Faces) -> FontContext {
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    let mut families = Vec::new();
    let register = |collection: &mut Collection, face: &Vec<u8>, alias: Option<&str>| {
        let blob = Blob::new(Arc::new(face.clone()) as Arc<dyn AsRef<[u8]> + Send + Sync>);
        let over = alias.map(|name| FontInfoOverride {
            family_name: Some(name),
            ..Default::default()
        });
        let mut found: Vec<(String, FamilyId)> = collection
            .register_fonts(blob, over)
            .into_iter()
            .map(|(id, _)| (collection.family_name(id).unwrap_or("").to_owned(), id))
            .collect();
        found.sort();
        found.into_iter().map(|(_, id)| id).collect::<Vec<_>>()
    };
    if let Some(sans) = &faces.sans {
        families.extend(register(&mut collection, sans, None));
        // Hybridcast の CSS は `font-family:"丸ゴシック"` と書く。
        for alias in ["丸ゴシック", "角ゴシック", "太丸ゴシック"] {
            let _ = register(&mut collection, sans, Some(alias));
        }
    }
    if let Some(mono) = &faces.mono {
        let named = register(&mut collection, mono, None);
        let _ = register(&mut collection, mono, Some(MONO_ALIAS));
        families.extend(named);
    }
    for generic in [
        GenericFamily::SansSerif,
        GenericFamily::Serif,
        GenericFamily::Monospace,
        GenericFamily::SystemUi,
        GenericFamily::UiSansSerif,
        GenericFamily::Cursive,
        GenericFamily::Fantasy,
    ] {
        collection.set_generic_families(generic, families.iter().copied());
    }
    for script in ["Hira", "Kana", "Hani", "Latn", "Zyyy", "Zinh", "Zzzz"] {
        collection.set_fallbacks(Script::from_str_unchecked(script), families.iter().copied());
    }
    FontContext {
        source_cache: SourceCache::new_shared(),
        collection,
    }
}

fn read_first(variable: &str, names: &[&str]) -> Option<Vec<u8>> {
    if let Some(path) = std::env::var_os(variable)
        && let Ok(bytes) = std::fs::read(path)
    {
        return Some(bytes);
    }
    names.iter().find_map(|name| crate::plug::host::font(name))
}

pub fn find_japanese() -> Option<Vec<u8>> {
    const NAMES: &[&str] = &[
        "Yu Gothic Medium",
        "Meiryo",
        "MS Gothic",
        "Hiragino Sans W3",
        "Hiragino Kaku Gothic ProN W3",
        "Noto Sans CJK JP",
        "Noto Sans JP",
        "IPAexGothic",
        "IPAGothic",
        "Droid Sans Japanese",
    ];
    read_first("ZW_FONT", NAMES)
}

/// BML の文字はグリッドに配置されるので、等幅でレンダリングすると受信機と同じに並ぶ。
pub fn find_monospace_japanese() -> Option<Vec<u8>> {
    const NAMES: &[&str] = &[
        "MS Gothic",
        "Osaka-Mono",
        "Osaka",
        "Noto Sans Mono CJK JP",
        "Noto Sans CJK JP",
        "IPAGothic",
    ];
    read_first("ZW_MONO_FONT", NAMES)
}

/// TTC の `index` 番目を一つのフォントファイルにする（fontique は番号を選んで登録できない）。
pub fn single_face(bytes: &[u8], index: u32) -> Option<Vec<u8>> {
    let be32 = |at: usize| -> Option<usize> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let be16 = |at: usize| -> Option<usize> {
        Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize)
    };
    if !bytes.starts_with(b"ttcf") || index as usize >= be32(8)? {
        return None;
    }
    let start = be32(12 + 4 * index as usize)?;
    let tables = be16(start + 4)?;
    let mut out = bytes.get(start..start + 12)?.to_vec();
    let mut body = Vec::new();
    let mut offset = 12 + 16 * tables;
    for n in 0..tables {
        let record = start + 12 + 16 * n;
        let (at, len) = (be32(record + 8)?, be32(record + 12)?);
        let table = bytes.get(at..at.checked_add(len)?)?;
        out.extend_from_slice(bytes.get(record..record + 8)?);
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(len as u32).to_be_bytes());
        body.extend_from_slice(table);
        let padded = len.next_multiple_of(4);
        body.resize(body.len() + padded - len, 0);
        offset += padded;
    }
    out.extend_from_slice(&body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection() -> Vec<u8> {
        let mut out = b"ttcf\0\x01\0\0\0\0\0\x02".to_vec();
        let header = 12 + 8;
        let face = 12 + 16;
        let tables = header + 2 * face;
        out.extend_from_slice(&(header as u32).to_be_bytes());
        out.extend_from_slice(&((header + face) as u32).to_be_bytes());
        for (n, tag) in [b"AAAA", b"BBBB"].into_iter().enumerate() {
            out.extend_from_slice(b"\0\x01\0\0\0\x01\0\x10\0\0\0\0");
            out.extend_from_slice(tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&((tables + 8 * n) as u32).to_be_bytes());
            out.extend_from_slice(&(5u32 - n as u32).to_be_bytes());
        }
        out.extend_from_slice(b"aaaaa\0\0\0bbbb");
        out
    }

    #[test]
    fn one_face_is_taken_out_of_a_collection() {
        let bytes = collection();
        let first = single_face(&bytes, 0).expect("一つ目");
        assert_eq!(&first[..4], b"\0\x01\0\0");
        assert_eq!(&first[12..16], b"AAAA");
        assert_eq!(
            u32::from_be_bytes(first[20..24].try_into().unwrap()),
            28,
            "テーブルはヘッダーのすぐ後"
        );
        assert_eq!(&first[28..], b"aaaaa\0\0\0", "4 の倍数にパディングする");
        let second = single_face(&bytes, 1).expect("二つ目");
        assert_eq!(&second[12..16], b"BBBB");
        assert_eq!(&second[28..], b"bbbb");
        assert!(single_face(&bytes, 2).is_none());
        assert!(
            single_face(b"\0\x01\0\0", 0).is_none(),
            "TTC でなければ抜き出さない"
        );
    }
}
