// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組の詳細（視聴年齢、契約、シリーズ等）を一行ずつの文字列に。

use super::Details;

pub fn details_lines(details: &Details) -> Vec<String> {
    let mut lines = Vec::new();
    for rating in &details.ratings {
        lines.push(match isdb::descriptors::rating_age(rating.rating) {
            Some(age) => format!(
                "視聴年齢 : {age} 歳以上（{}, 0x{:02x}）",
                rating.country, rating.rating
            ),
            None => format!(
                "視聴年齢 : 指定なし（{}, 0x{:02x}）",
                rating.country, rating.rating
            ),
        });
    }
    for contract in &details.contracts {
        lines.push(contract_line(contract));
    }
    if let Some(series) = &details.series {
        let mut said = format!(
            "シリーズ : ID 0x{:04x} ({}) / {}",
            series.id,
            series.id,
            series.pattern_name()
        );
        if series.episode != 0 || series.last_episode != 0 {
            said.push_str(&format!(" / 第{}話", series.episode));
            if series.last_episode != 0 {
                said.push_str(&format!("（全{}話）", series.last_episode));
            }
        }
        if series.repeat_label != 0 {
            said.push_str(&format!(" / 再放送ラベル {}", series.repeat_label));
        }
        if let Some(mjd) = series.expire_mjd {
            let (year, month, day) = civil_of_mjd(mjd);
            said.push_str(&format!(" / 期限 {year}/{month:02}/{day:02}"));
        }
        if !series.name.is_empty() {
            said.push_str(&format!(" / {}", series.name));
        }
        lines.push(said);
    }
    if let Some(region) = &details.region {
        lines.push(region_line(region));
    }
    for link in &details.links {
        lines.push(format!(
            "ハイパーリンク : 種別 {} → {:?}",
            link.kind, link.destination
        ));
    }
    for service in &details.multimedia {
        lines.push(format!(
            "マルチメディアサービス : 方式 0x{:04x}{}{}{}",
            service.data_component_id,
            service
                .component_tag
                .map(|tag| format!(" / component tag 0x{tag:04x}"))
                .unwrap_or_default(),
            slashed(&service.text),
            match service.associated_contents {
                Some(true) => " / 番組連動あり",
                Some(false) => " / 番組連動なし",
                None => "",
            }
        ));
    }
    if let Some(groups) = &details.groups {
        lines.push(format!(
            "コンポーネントグループ : 種別 {}（{}）",
            groups.group_type,
            if groups.group_type == 0 {
                "マルチビューTV"
            } else {
                "未定義"
            }
        ));
        for group in &groups.groups {
            let units: Vec<String> = group
                .units
                .iter()
                .map(|unit| format!("課金単位 {} [{}]", unit.ca_unit, tags(&unit.components)))
                .collect();
            lines.push(format!(
                "  グループ {} : {}{}{}",
                group.id,
                units.join(" / "),
                bitrate(group.total_bitrate),
                slashed(&group.name)
            ));
        }
    }
    lines
}

pub fn region_line(region: &isdb::descriptors::TargetRegion) -> String {
    if region.kind == 0x01 {
        format!(
            "対象地域 : {}",
            region
                .prefectures
                .iter()
                .map(|bit| isdb::descriptors::prefecture_name(*bit).unwrap_or("?"))
                .collect::<Vec<_>>()
                .join("・")
        )
    } else {
        format!(
            "対象地域 : 方式 0x{:02x} / {} バイト",
            region.kind,
            region.raw.len()
        )
    }
}

/// 1/4 Mbps 単位。
fn bitrate(rate: Option<u8>) -> String {
    match rate {
        Some(rate) => format!(" / 最大 {:.2} Mbps", f64::from(rate) / 4.0),
        None => String::new(),
    }
}

pub(crate) fn contract_line(contract: &isdb::descriptors::CaContract) -> String {
    format!(
        "CA 契約情報 : CA system ID 0x{:04x} / 課金単位 {} / component tag [{}]{}{}",
        contract.ca_system_id,
        contract.unit,
        tags(&contract.components),
        slashed(&contract.fee_name),
        if contract.verification.is_empty() {
            String::new()
        } else {
            format!(" / 契約確認情報 {} バイト", contract.verification.len())
        }
    )
}

fn tags(tags: &[u16]) -> String {
    tags.iter()
        .map(|tag| format!("0x{tag:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn slashed(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!(" / {text}")
    }
}

/// 修正ユリウス日を年月日に（STD-B10 付属 C）。
fn civil_of_mjd(mjd: u16) -> (i32, u32, u32) {
    let mjd = f64::from(mjd);
    let year = ((mjd - 15_078.2) / 365.25).floor();
    let month = ((mjd - 14_956.1 - (year * 365.25).floor()) / 30.6001).floor();
    let day = mjd - 14_956.0 - (year * 365.25).floor() - (month * 30.6001).floor();
    let k = if month == 14.0 || month == 15.0 {
        1.0
    } else {
        0.0
    };
    (
        (year + k) as i32 + 1900,
        (month - 1.0 - k * 12.0) as u32,
        day as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modified_julian_date_is_a_civil_date() {
        assert_eq!(civil_of_mjd(0xc079), (1993, 10, 13));
    }

    #[test]
    fn details_become_lines() {
        let details = Details {
            ratings: vec![isdb::descriptors::ParentalRating {
                country: "JPN".into(),
                rating: 0x04,
            }],
            series: Some(isdb::descriptors::Series {
                id: 1,
                pattern: 2,
                episode: 3,
                last_episode: 12,
                name: "連続".into(),
                ..Default::default()
            }),
            ..Details::default()
        };
        let lines = details_lines(&details);
        assert_eq!(lines[0], "視聴年齢 : 7 歳以上（JPN, 0x04）");
        assert_eq!(
            lines[1],
            "シリーズ : ID 0x0001 (1) / 週一回 / 第3話（全12話） / 連続"
        );
        assert_eq!(details.minimum_age(), Some(7));
    }
}
