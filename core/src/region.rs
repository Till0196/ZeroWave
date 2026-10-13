// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 郵便番号から居住地の放送地域を決める（同じ地域の局を優先して並べる、データ放送の天気予報に使う）。
//! 地上波の `network_id` は放送地域から決まる（ARIB TR-B14 第五編）。

use std::sync::RwLock;

/// 居住地の地域識別（県域と広域圏）。空なら決めていない。
static HOME: RwLock<Vec<u8>> = RwLock::new(Vec::new());

/// 七桁、`-` は有っても無くても。読めなければ地域は決めない。
pub fn set_postal(postal: &str) {
    let areas = prefecture_of(postal).map(areas_of).unwrap_or_default();
    *HOME.write().unwrap_or_else(|e| e.into_inner()) = areas;
}

pub fn is_home(network: u16) -> bool {
    let Some(area) = area_of(network) else {
        return false;
    };
    HOME.read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&area)
}

pub fn digits(postal: &str) -> Option<String> {
    let digits: String = postal
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '-' | '－' | 'ー' | '−' | '‐' | '‑' | '―' | ' ' | '　' | '〒'
            )
        })
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            c => c,
        })
        .collect();
    (digits.len() == 7 && digits.chars().all(|c| c.is_ascii_digit())).then_some(digits)
}

pub fn area_of(network: u16) -> Option<u8> {
    let area = |network: u16| {
        let area = (0x7ff0 - (network & 0xfff0)) >> 4;
        (1..=62).contains(&area).then_some(area as u8)
    };
    match network {
        0x7c10..=0x7fef => area(network),
        // 県複フラグの立った物。
        0x7810..=0x7bef => area(network + 0x400),
        _ => None,
    }
}

fn prefecture_of(postal: &str) -> Option<u8> {
    let digits = digits(postal)?;
    let head: u16 = digits[..3].parse().ok()?;
    Some(match head {
        1..=9 | 40..=99 => HOKKAIDO,
        10..=19 => 18,   // 秋田
        20..=29 => 20,   // 岩手
        30..=39 => 22,   // 青森
        100..=209 => 23, // 東京
        210..=259 => 24, // 神奈川
        260..=299 => 27, // 千葉
        300..=319 => 26, // 茨城
        320..=329 => 28, // 栃木
        330..=369 => 29, // 埼玉
        370..=379 => 25, // 群馬
        380..=399 => 30, // 長野
        400..=409 => 32, // 山梨
        410..=439 => 35, // 静岡
        440..=499 => 33, // 愛知
        500..=509 => 39, // 岐阜
        510..=519 => 38, // 三重
        520..=529 => 45, // 滋賀
        530..=599 => 40, // 大阪
        600..=629 => 41, // 京都
        630..=639 => 44, // 奈良
        640..=649 => 43, // 和歌山
        650..=679 => 42, // 兵庫
        680..=689 => 49, // 鳥取
        690..=699 => 48, // 島根
        700..=719 => 47, // 岡山
        720..=739 => 46, // 広島
        740..=759 => 50, // 山口
        760..=769 => 52, // 香川
        770..=779 => 53, // 徳島
        780..=789 => 54, // 高知
        790..=799 => 51, // 愛媛
        800..=839 => 55, // 福岡
        840..=849 => 61, // 佐賀
        850..=859 => 57, // 長崎
        860..=869 => 56, // 熊本
        870..=879 => 60, // 大分
        880..=889 => 59, // 宮崎
        890..=899 => 58, // 鹿児島
        900..=909 => 62, // 沖縄
        910..=919 => 36, // 福井
        920..=929 => 34, // 石川
        930..=939 => 37, // 富山
        940..=959 => 31, // 新潟
        960..=979 => 21, // 福島
        980..=989 => 17, // 宮城
        990..=999 => 19, // 山形
        _ => return None,
    })
}

/// 道内の県域は郵便番号では分けないので、道内の地域識別を全部含める。
const HOKKAIDO: u8 = 4;

/// その県の局が出る地域識別：県域と、県が属する広域圏。
fn areas_of(prefecture: u8) -> Vec<u8> {
    let wide = match prefecture {
        23..=29 => Some(1), // 関東広域圏（東京・神奈川・群馬・茨城・千葉・栃木・埼玉）
        40..=45 => Some(2), // 近畿広域圏
        33 | 38 | 39 => Some(3), // 中京広域圏（愛知・三重・岐阜）
        47 | 52 => Some(5), // 岡山香川
        48 | 49 => Some(6), // 島根鳥取
        _ => None,
    };
    if prefecture == HOKKAIDO {
        return vec![HOKKAIDO, 10, 11, 12, 13, 14, 15, 16];
    }
    std::iter::once(prefecture).chain(wide).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrestrial_networks_name_their_area() {
        assert_eq!(area_of(0x7fe0), Some(1), "0x7fe0 は関東広域圏");
        assert_eq!(area_of(0x7e87), Some(23), "0x7e87 は東京");
        assert_eq!(area_of(0x7e77), Some(24), "0x7e77 は神奈川");
        assert_eq!(area_of(0x7e47), Some(27), "0x7e47 は千葉");
        assert_eq!(area_of(0x0004), None, "BS は地域が無い");
    }

    #[test]
    fn a_postal_code_names_the_prefecture() {
        assert_eq!(prefecture_of("100-0001"), Some(23));
        assert_eq!(prefecture_of("２６０－０００１"), Some(27));
        assert_eq!(prefecture_of("060-0001"), Some(HOKKAIDO));
        assert_eq!(prefecture_of("12345"), None);
        assert_eq!(areas_of(27), vec![27, 1]);
    }
}
