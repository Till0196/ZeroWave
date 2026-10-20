// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! コード値や時刻を画面に出す文字列に。

use super::*;

/// 放送の時刻（日本標準時）を 1970 年からの秒に。タイムゾーンは付けない。
pub fn epoch(time: BroadcastTime) -> Moment {
    let year = i64::from(time.year);
    let month = i64::from(time.month);
    let day = i64::from(time.day);
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    days * 86_400
        + i64::from(time.hour) * 3600
        + i64::from(time.minute) * 60
        + i64::from(time.second)
}

pub fn video_name(component_type: u8) -> Option<String> {
    // ARIB STD-B10 第2部 表 6-5。4320p は 0x83 だけ、2160p は 0x91〜0x94。
    if component_type == 0x83 {
        return Some(String::from("4320p、アスペクト比16:9"));
    }
    let lines = match component_type & 0xf0 {
        0x00 => "480i（525i）",
        0x90 => "2160p",
        0xa0 => "480p（525p）",
        0xb0 => "1080i（1125i）",
        0xc0 => "720p（750p）",
        0xd0 => "240p",
        0xe0 => "1080p（1125p）",
        0xf0 => "180p",
        _ => return None,
    };
    let shape = match component_type & 0x0f {
        0x01 => "アスペクト比4:3",
        0x02 => "アスペクト比16:9 パンベクトルあり",
        0x03 => "アスペクト比16:9 パンベクトルなし",
        0x04 => "アスペクト比16:9より大きい",
        _ => return None,
    };
    Some(format!("{lines}、{shape}"))
}

impl Picture {
    pub fn name(&self) -> Option<String> {
        // DMirakurun は MMT/TLV の番組にもこの項目を埋めるので、テーブルに無い番号は指定なしとして扱う。
        if let Some(named) = self.component_type.and_then(video_name) {
            return Some(named);
        }
        let mut parts = Vec::new();
        let lines = match self.resolution {
            1 => Some("180"),
            2 => Some("240"),
            3 => Some("480"),
            4 => Some("720"),
            5 => Some("1080"),
            6 => Some("2160"),
            7 => Some("4320"),
            _ => None,
        };
        let fps = match self.frame_rate {
            1 => Some("15"),
            2 => Some("23.976"),
            3 => Some("24"),
            4 => Some("25"),
            5 => Some("29.97"),
            6 => Some("30"),
            7 => Some("50"),
            8 => Some("59.94"),
            9 => Some("60"),
            10 => Some("100"),
            11 => Some("119.88"),
            12 => Some("120"),
            _ => None,
        };
        let scan = if self.progressive { 'p' } else { 'i' };
        match (lines, fps) {
            (Some(lines), Some(fps)) => parts.push(format!("{lines}{scan} {fps}fps")),
            (Some(lines), None) => parts.push(format!("{lines}{scan}")),
            (None, Some(fps)) => parts.push(format!("{fps}fps")),
            (None, None) => {}
        }
        if let Some(shape) = aspect_name(self.aspect) {
            parts.push(shape.to_owned());
        }
        if let Some(transfer) = transfer_name(self.transfer) {
            parts.push(transfer.to_owned());
        }
        (!parts.is_empty()).then(|| parts.join("、"))
    }
}

fn aspect_name(aspect: u8) -> Option<&'static str> {
    Some(match aspect {
        1 => "アスペクト比4:3",
        2 => "アスペクト比16:9 パンベクトルあり",
        3 => "アスペクト比16:9",
        4 => "アスペクト比16:9より大きい",
        _ => return None,
    })
}

/// 伝送特性（ARIB STD-B60 表 7-51）。BT.709 と BT.2020 は SDR。
fn transfer_name(transfer: u8) -> Option<&'static str> {
    Some(match transfer {
        1 => "SDR（BT.709）",
        2 => "SDR（IEC 61966-2-4）",
        3 => "SDR（BT.2020）",
        4 => "HDR（PQ）",
        5 => "HDR（HLG）",
        _ => return None,
    })
}

pub trait Described {
    /// デュアルモノの 2 つ目の言語は「／」で繋ぐ。
    fn who(&self) -> String;
    fn with_assist(&self, called: String) -> String;
    fn arrangement(&self) -> String;
    fn describe(&self) -> String;
}

impl Described for Component {
    fn who(&self) -> String {
        if let Some(name) = self.called() {
            return name.to_owned();
        }
        let mut who = language_name(&self.language).to_owned();
        if let Some(second) = &self.second {
            let second = language_name(second);
            if !second.is_empty() {
                if !who.is_empty() {
                    who.push('／');
                }
                who.push_str(second);
            }
        }
        who
    }

    /// 解説放送などを添える（STD-B10 表 6-43 の b6-b5）。名前に既に含まれていれば重ねない。
    fn with_assist(&self, called: String) -> String {
        let Some(word) = self.assist().name() else {
            return called;
        };
        if called.contains(word) {
            return called;
        }
        if called.is_empty() {
            return word.to_owned();
        }
        format!("{called}（{word}）")
    }

    /// テーブルはここ一つ（音声一覧とステータスバーで表示が一致するように）。
    fn arrangement(&self) -> String {
        if self.dual_mono() {
            return String::from("デュアルモノ");
        }
        match self.channels() {
            0 => String::new(),
            1 => String::from("モノラル"),
            2 => String::from("ステレオ"),
            6 => String::from("5.1ch"),
            24 => String::from("22.2ch"),
            many => format!("{many}ch"),
        }
    }

    /// 未知の音声モードは番号のまま書く。
    fn describe(&self) -> String {
        let mut who = self.who();
        if who.is_empty()
            && let Some(word) = self.assist().name()
        {
            who = word.to_owned();
        }
        let mode = match audio_name(self.kind) {
            Some(mode) => mode.to_owned(),
            None => format!("モード 0x{:02x}", self.kind & 0x1f),
        };
        let mode = match self.assist().name() {
            Some(word) if !who.contains(word) => format!("{mode}（{word}）"),
            _ => mode,
        };
        let mut parts = vec![mode];
        if let Some(hz) = self.sampling_hz() {
            parts.push(if hz % 1000 == 0 {
                format!("{}kHz", hz / 1000)
            } else {
                format!("{:.2}kHz", f64::from(hz) / 1000.0)
            });
        }
        if self.main {
            parts.push("主音声".to_owned());
        }
        if who.is_empty() {
            parts.join("、")
        } else {
            format!("{who}　{}", parts.join("、"))
        }
    }
}

pub fn language_name(iso: &str) -> &'static str {
    match iso {
        "jpn" => "日本語",
        "eng" => "英語",
        "kor" => "韓国語",
        "zho" | "chi" => "中国語",
        "por" => "ポルトガル語",
        "spa" => "スペイン語",
        "fra" | "fre" => "フランス語",
        "deu" | "ger" => "ドイツ語",
        "ita" => "イタリア語",
        "rus" => "ロシア語",
        "vie" => "ベトナム語",
        "tha" => "タイ語",
        "ind" => "インドネシア語",
        "tgl" => "タガログ語",
        // 解説放送が `etc` で来ることがある。
        _ => "",
    }
}

/// 音声モードは下位 5 ビットだけ見る（STD-B10 表 6-43）。全体で比べると 0x23 に名前が付かない。
pub fn audio_name(component_type: u8) -> Option<&'static str> {
    Some(match component_type & 0x1f {
        0x01 => "1/0モード（シングルモノ）",
        0x02 => "1/0＋1/0モード（デュアルモノ）",
        0x03 => "2/0モード（ステレオ）",
        0x04 => "2/1モード",
        0x05 => "3/0モード",
        0x06 => "2/2モード",
        0x07 => "3/1モード",
        0x08 => "3/2モード",
        0x09 => "3/2＋LFEモード（5.1ch）",
        0x0a => "3/3.1モード",
        0x0b => "2/0/0-2/0/2-0.1モード",
        0x0c => "5/2.1モード",
        0x0d => "3/2/2.1モード",
        0x0e => "2/0/0-3/0/2-0.1モード",
        0x0f => "0/2/0-3/0/2-0.1モード",
        // 12 チャンネルで 10.2ch。
        0x10 => "2/0/0-3/2/3-0.2モード（10.2ch）",
        // 24 チャンネル（STD-B32 第二部の構成 13）。
        0x11 => "3/3/3-5/2/3-3/0/0.2モード（22.2ch）",
        _ => return None,
    })
}

static SHOWN_OFFSET: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(super::JST_OFFSET);

/// 時刻を表示する時差（UTC から、秒）。設定の `guide.offset`。
pub fn shown_offset() -> i32 {
    SHOWN_OFFSET.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_shown_offset(seconds: i32) {
    SHOWN_OFFSET.store(seconds, std::sync::atomic::Ordering::Relaxed);
}

/// 放送の時刻（日本標準時）を、表示する時差の時刻に。
pub fn shown(moment: Moment) -> Moment {
    shifted(moment, shown_offset())
}

/// [`shown`] の逆。表示する時差で決めた時刻（日の始まりなど）を放送の時刻に。
pub fn unshown(moment: Moment) -> Moment {
    moment - shifted(0, shown_offset())
}

fn shifted(moment: Moment, offset: i32) -> Moment {
    moment - i64::from(super::JST_OFFSET) + i64::from(offset)
}

/// 以下の表示は放送の時刻を受け取り、表示する時差で書く。
pub fn hhmm(moment: Moment) -> String {
    let (_, _, _, hour, minute, _) = civil(shown(moment));
    format!("{hour:02}:{minute:02}")
}

pub fn date_label(moment: Moment) -> String {
    let (_, month, day, _, _, _) = civil(shown(moment));
    format!("{month}/{day}（{}）", weekday(moment))
}

pub fn airtime(start: Moment, ends: Option<Moment>) -> String {
    match ends {
        Some(ends) => format!("{}〜{}", hhmm(start), hhmm(ends)),
        None => format!("{}〜", hhmm(start)),
    }
}

pub fn weekday(moment: Moment) -> &'static str {
    const WEEKDAYS: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];
    WEEKDAYS[(shown(moment).div_euclid(86_400) + 4).rem_euclid(7) as usize]
}

pub fn civil(moment: Moment) -> (i64, u32, u32, u32, u32, u32) {
    let days = moment.div_euclid(86_400) + 719_468;
    let seconds = moment.rem_euclid(86_400);
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month = (5 * day_of_year + 2) / 153;
    (
        year_of_era + era * 400 + i64::from(month >= 10),
        (if month < 10 { month + 3 } else { month - 9 }) as u32,
        (day_of_year - (153 * month + 2) / 5 + 1) as u32,
        (seconds / 3600) as u32,
        ((seconds / 60) % 60) as u32,
        (seconds % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use isdb::descriptors::Assist;

    fn sound(kind: u8) -> Component {
        Component {
            kind,
            tag: None,
            language: String::from("jpn"),
            second: None,
            main: true,
            name: None,
            quality: 0,
            sampling: 0,
            simulcast: None,
        }
    }

    #[test]
    fn a_time_is_shown_in_the_offset_of_the_settings() {
        // 2026-10-16 12:40:04 JST。
        let moment = epoch(BroadcastTime {
            year: 2026,
            month: 10,
            day: 16,
            hour: 12,
            minute: 40,
            second: 4,
        });
        assert_eq!(civil(shifted(moment, 0)).3, 3);
        let (_, _, day, hour, minute, _) = civil(shifted(moment, -(5 * 3600 + 30 * 60)));
        assert_eq!((day, hour, minute), (15, 22, 10));
        assert_eq!(shifted(moment, super::super::JST_OFFSET), moment);
    }

    #[test]
    fn a_sound_says_who_it_is_for_when_the_transmission_did() {
        assert_eq!(
            sound(0x23).with_assist(String::from("日本語")),
            "日本語（解説）"
        );
        assert_eq!(
            sound(0x43).with_assist(String::from("日本語")),
            "日本語（聴覚障害者用音声）"
        );
        assert_eq!(sound(0x03).with_assist(String::from("日本語")), "日本語");
        assert_eq!(
            sound(0x23).with_assist(String::from("日本語（解説）")),
            "日本語（解説）"
        );
        assert_eq!(sound(0x23).with_assist(String::new()), "解説");
    }

    #[test]
    fn a_sound_is_named_by_its_words_and_its_arrangement() {
        assert_eq!(sound(0x03).who(), "日本語");
        assert_eq!(sound(0x03).arrangement(), "ステレオ");
        let mut dual = sound(0x02);
        dual.second = Some(String::from("eng"));
        assert_eq!(dual.who(), "日本語／英語");
        assert_eq!(dual.arrangement(), "デュアルモノ");
        let mut named = sound(0x11);
        named.name = Some(String::from("球場音声"));
        assert_eq!(named.who(), "球場音声");
        assert_eq!(named.arrangement(), "22.2ch");
        let mut unknown = sound(0x00);
        unknown.language = String::from("etc");
        assert_eq!(unknown.who(), "");
        assert_eq!(unknown.arrangement(), "");
    }

    #[test]
    fn it_counts_the_channels_of_the_arrangements() {
        assert_eq!(sound(0x11).channels(), 24);
        assert_eq!(sound(0x10).channels(), 12);
        assert_eq!(sound(0x09).channels(), 6);
        assert_eq!(sound(0x03).channels(), 2);
        assert_eq!(sound(0x01).channels(), 1);
        assert_eq!(sound(0x00).channels(), 0);
        assert_eq!(sound(0x12).channels(), 0);
    }

    #[test]
    fn a_mark_is_not_a_name_and_the_table_fills_the_map_in() {
        let mut marked = sound(0x03);
        marked.name = Some("[S]".to_owned());
        assert_eq!(marked.called(), None);
        marked.name = Some("［Ｓ］".to_owned());
        assert_eq!(marked.called(), None);
        let mut named = sound(0x03);
        named.name = Some("球場音声".to_owned());
        assert_eq!(named.called(), Some("球場音声"));
        let mut described = sound(0x23);
        described.name = Some("日本語（解説）".to_owned());
        marked.enrich(&described);
        assert_eq!(marked.called(), Some("日本語（解説）"));
        assert_eq!(marked.assist(), Assist::Visual);
        let mut other = sound(0x09);
        other.enrich(&described);
        assert_eq!(other.assist(), Assist::None);
    }

    #[test]
    fn dual_mono_is_two_channels_and_says_what_it_is() {
        assert!(sound(0x02).dual_mono());
        assert_eq!(sound(0x02).channels(), 2);
        assert!(!sound(0x03).dual_mono());
    }

    #[test]
    fn the_upper_three_bits_are_not_the_arrangement() {
        assert_eq!(sound(0x80 | 0x11).channels(), 24);
        assert_eq!(sound(0x20 | 0x03).channels(), 2);
        assert_eq!(
            crate::guide::audio_name(0x20 | 0x03),
            Some("2/0モード（ステレオ）")
        );
        // 取り違えると二つの言語が左右に分かれて再生される。
        assert!(sound(0x40 | 0x02).dual_mono());
        assert_eq!(sound(0x40 | 0x02).channels(), 2);
    }

    #[test]
    fn the_name_and_the_count_agree() {
        for kind in 0x01..=0x11u8 {
            let name = crate::guide::audio_name(kind).expect("テーブルにある");
            let said = name.contains("22.2ch");
            assert_eq!(
                said,
                sound(kind).channels() == 24,
                "0x{kind:02x} {name} と {} チャンネル",
                sound(kind).channels()
            );
        }
    }
}
