// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 局ロゴ一枚。PNG（パレット無し）のまま持ち、デコードで STD-B24 の既定の CLUT を差し込む。

use crate::image::Rgba;

/// 先に取る種類。大きいものから。
pub const PREFERRED: [u8; 8] = [7, 6, 5, 3, 4, 2, 0, 1];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logo {
    /// 0〜5 は TS（TR-B14）、5〜7 は MMT/TLV（TR-B39・STD-B63）。
    pub logo_type: u8,
    pub logo_id: u16,
    pub version: u16,
    pub png: Vec<u8>,
}

const LOGO_MOST: u32 = 512;

impl Logo {
    pub fn png_with_palette(&self) -> Vec<u8> {
        isdb::logo::with_palette(&self.png)
    }

    pub fn rgba(&self) -> Option<Rgba> {
        let png = self.png_with_palette();
        let mut decoder = png::Decoder::new(std::io::Cursor::new(png.as_slice()));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().ok()?;
        // ヘッダに書かれた大きさで確保するので先に見る（規格では大きくても 64x36）。
        let info = reader.info();
        if info.width > LOGO_MOST || info.height > LOGO_MOST {
            return None;
        }
        let mut buffer = vec![0u8; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buffer).ok()?;
        let pixels = &buffer[..info.buffer_size()];
        let rgba = crate::image::rgba_of_png(info.color_type, pixels)?;
        if rgba.len() != info.width as usize * info.height as usize * 4 {
            return None;
        }
        Some(Rgba::new(info.width, info.height, rgba))
    }

    /// 見える画素（アルファが 0 でない画素）があるか。デコードできないロゴも見えないものとする。
    pub fn visible(&self) -> bool {
        self.rgba().is_some_and(|picture| {
            picture
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 0)
        })
    }
}

/// 12 ビットで一周するので、前半（0〜2047）と後半に分かれていれば前半の方を新しいと見なす。
pub fn compare_versions(a: u16, b: u16) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a == b {
        return Ordering::Equal;
    }
    if (a <= 2047) == (b <= 2047) {
        return a.cmp(&b);
    }
    if a <= 2047 {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

/// フォルダの中は NID ごとのサブフォルダ（`7880/5_123_007.png` のように NID を 4 桁の 16 進で）。
fn network_folder(folder: &std::path::Path, network: u16) -> std::path::PathBuf {
    folder.join(format!("{network:04x}"))
}

fn file_name(logo: &Logo) -> String {
    format!(
        "{}_{:03x}_{:03x}.png",
        logo.logo_type, logo.logo_id, logo.version
    )
}

fn parse_name(name: &str) -> Option<(u8, u16, u16)> {
    let stem = name.strip_suffix(".png")?;
    let mut parts = stem.split('_');
    let logo_type = parts.next()?.parse().ok()?;
    let logo_id = u16::from_str_radix(parts.next()?, 16).ok()?;
    let version = u16::from_str_radix(parts.next()?, 16).ok()?;
    parts
        .next()
        .is_none()
        .then_some((logo_type, logo_id, version))
}

fn parse_network(name: &str) -> Option<u16> {
    (name.len() == 4)
        .then(|| u16::from_str_radix(name, 16).ok())
        .flatten()
}

/// フォルダの局ロゴ：（ネットワーク、種類、番号）→（一番新しいバージョン、ファイル）。
fn listed(
    folder: &std::path::Path,
) -> std::collections::BTreeMap<(u16, u8, u16), (u16, std::path::PathBuf)> {
    let mut newest = std::collections::BTreeMap::new();
    let Ok(networks) = std::fs::read_dir(folder) else {
        return newest;
    };
    for network in networks.flatten() {
        let Some(id) = parse_network(&network.file_name().to_string_lossy()) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(network.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((logo_type, logo_id, version)) = parse_name(&name) else {
                continue;
            };
            let path = entry.path();
            newest
                .entry((id, logo_type, logo_id))
                .and_modify(|(had, file): &mut (u16, std::path::PathBuf)| {
                    if compare_versions(version, *had).is_gt() {
                        *had = version;
                        file.clone_from(&path);
                    }
                })
                .or_insert((version, path));
        }
    }
    newest
}

pub fn load_folder(folder: &std::path::Path) -> Vec<(u16, Logo)> {
    listed(folder)
        .into_iter()
        .filter_map(|((network, logo_type, logo_id), (version, file))| {
            let png = std::fs::read(file).ok()?;
            Some((
                network,
                Logo {
                    logo_type,
                    logo_id,
                    version,
                    png,
                },
            ))
        })
        .collect()
}

/// 同じか新しいバージョンがあれば書かない（別の deck が先に書くことがある）。書いた枚数を返す。
pub fn save_folder(folder: &std::path::Path, logos: &[(u16, &Logo)]) -> usize {
    let had = listed(folder);
    let mut written = 0;
    for (network, logo) in logos {
        let at = (*network, logo.logo_type, logo.logo_id);
        if had
            .get(&at)
            .is_some_and(|(version, _)| compare_versions(*version, logo.version).is_ge())
        {
            continue;
        }
        let place = network_folder(folder, *network);
        let _ = std::fs::create_dir_all(&place);
        let path = place.join(file_name(logo));
        if let Err(error) = write_file(&path, &logo.png_with_palette()) {
            tracing::warn!("局ロゴを保存できない {}: {error}", path.display());
            continue;
        }
        if let Some((_, old)) = had.get(&at) {
            let _ = std::fs::remove_file(old);
        }
        written += 1;
    }
    written
}

/// 一時ファイルに書いてから置き換える。一時ファイルの名前にはプロセス ID を入れる（別の deck と書き込みが重ならない）。
fn write_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let fresh = path.with_extension(format!("png.{}.new", std::process::id()));
    std::fs::write(&fresh, bytes)?;
    std::fs::rename(&fresh, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&fresh);
    })
}

/// ロゴの種類の大きさ（TR-B14 第二分冊、TR-B39）。
pub fn name(logo_type: u8) -> &'static str {
    match logo_type {
        0 => "48×24",
        1 => "36×24",
        2 => "48×27",
        3 => "72×36",
        4 => "54×36",
        5 => "64×36",
        6 => "128×72",
        7 => "256×144",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed_png() -> Vec<u8> {
        indexed(&[0, 8])
    }

    /// 横に並べた画素の、パレットの無い PNG。
    fn indexed(pixels: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, pixels.len() as u32, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![9, 9, 9, 7, 7, 7]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        let mut stripped = out[..33].to_vec();
        let mut rest = &out[33..];
        while rest.len() >= 12 {
            let length = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
            let chunk = &rest[..12 + length];
            if &chunk[4..8] != b"PLTE" {
                stripped.extend_from_slice(chunk);
            }
            rest = &rest[12 + length..];
        }
        stripped
    }

    #[test]
    fn a_logo_without_a_palette_is_decoded_with_the_default_clut() {
        let logo = Logo {
            logo_type: 5,
            logo_id: 1,
            version: 0,
            png: indexed_png(),
        };
        let picture = logo.rgba().unwrap();
        assert_eq!((picture.width, picture.height), (2, 1));
        assert_eq!(&picture.rgba, &[0, 0, 0, 255, 0, 0, 0, 0]);
        assert_eq!(name(5), "64×36");
    }

    #[test]
    fn a_logo_with_nothing_visible_is_not_shown() {
        let key = super::super::Key {
            network: 4,
            service: 102,
        };
        let logo = |logo_type, pixels: &[u8]| Logo {
            logo_type,
            logo_id: 2,
            version: 0,
            png: indexed(pixels),
        };
        let mut guide = super::super::Guide::new();
        guide.tell_logo_id(key, 2);
        guide.tell_logo(4, logo(5, &[8, 8]));
        assert!(!guide.logos().next().unwrap().1.visible());
        assert!(guide.logo_of(key).is_none(), "透明だけのロゴは出さない");
        // 保存はする（バージョンを覚えておく）。
        assert_eq!(guide.logos().count(), 1);
        guide.tell_logo(4, logo(2, &[8, 1]));
        assert_eq!(guide.logo_of(key).unwrap().logo_type, 2);
        let mut lent = super::super::Guide::new();
        lent.tell_logo_id(key, 2);
        lent.lend_logos(&guide);
        assert_eq!(lent.logo_of(key).unwrap().logo_type, 2);
    }

    #[test]
    fn versions_wrap_around() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions(3, 2), Ordering::Greater);
        assert_eq!(compare_versions(2, 2), Ordering::Equal);
        assert_eq!(compare_versions(3000, 2500), Ordering::Greater);
        assert_eq!(compare_versions(0, 4095), Ordering::Greater);
        assert_eq!(compare_versions(4095, 0), Ordering::Less);
    }

    #[test]
    fn an_older_version_is_not_taken() {
        let mut guide = super::super::Guide::new();
        let logo = |version, png: u8| Logo {
            logo_type: 5,
            logo_id: 1,
            version,
            png: vec![png],
        };
        guide.tell_logo(4, logo(7, 1));
        guide.tell_logo(4, logo(6, 2));
        assert_eq!(guide.logos().next().unwrap().1.version, 7);
        guide.tell_logo(4, logo(7, 3));
        assert_eq!(guide.logos().next().unwrap().1.png, vec![3]);
        guide.tell_logo(4, logo(8, 4));
        assert_eq!(guide.logos().next().unwrap().1.version, 8);
    }

    #[test]
    fn logos_from_a_recording_are_learned_like_the_broadcast() {
        let logo = |id, version| Logo {
            logo_type: 5,
            logo_id: id,
            version,
            png: vec![1],
        };
        let mut kept = super::super::Guide::new();
        kept.tell_logo(4, logo(1, 7));
        let mut recorded = super::super::Guide::new();
        recorded.tell_logo(4, logo(1, 6));
        recorded.tell_logo(4, logo(2, 3));
        kept.learn_logos(&recorded);
        let versions: Vec<(u16, u16)> = kept
            .logos()
            .map(|(_, logo)| (logo.logo_id, logo.version))
            .collect();
        assert_eq!(versions, [(1, 7), (2, 3)]);
    }

    #[test]
    fn the_folder_keeps_the_newest() {
        let folder = std::env::temp_dir().join(format!("zw-logo-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let logo = |version| Logo {
            logo_type: 5,
            logo_id: 0x123,
            version,
            png: indexed_png(),
        };
        assert_eq!(save_folder(&folder, &[(0x7880, &logo(7))]), 1);
        assert!(folder.join("7880/5_123_007.png").exists());
        assert_eq!(save_folder(&folder, &[(0x7880, &logo(7))]), 0);
        assert_eq!(save_folder(&folder, &[(0x7880, &logo(8))]), 1);
        assert!(!folder.join("7880/5_123_007.png").exists());
        assert!(folder.join("7880/5_123_008.png").exists());
        assert_eq!(save_folder(&folder, &[(0x7880, &logo(7))]), 0);
        std::fs::write(folder.join("7880/5_123_006.png"), indexed_png()).unwrap();
        assert_eq!(save_folder(&folder, &[(0x0004, &logo(1))]), 1);
        assert!(folder.join("0004/5_123_001.png").exists());
        let read = load_folder(&folder);
        assert_eq!(read.len(), 2);
        assert_eq!((read[0].0, read[0].1.version), (0x0004, 1));
        assert_eq!((read[1].0, read[1].1.version), (0x7880, 8));
        assert!(
            read[1].1.rgba().is_some(),
            "パレットを差し込んだ形も画像にデコードできる"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }
}
