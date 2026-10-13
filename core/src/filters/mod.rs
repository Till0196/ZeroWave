// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 放送を読みデコードするフィルター群。同じテーブルや記述子を二箇所で読まない。TS と MMT/TLV の違いは
//! 一番下（[`packet`]）と記述子の解析に閉じ込める。繋ぐのは [`crate::engine::Engine`]。

pub mod analyzer;
pub mod captions;
pub mod counter;
/// 録画をデコーダーの単位に切るツール。テストと `--check` のためで、再生は通らない。
pub mod cut;
pub mod data;
pub mod feed;
pub mod logos;
pub mod packet;
pub mod selector;
pub mod tables;
pub mod units;

use mediadecoder::es::{Codec, Framing};

pub use feed::{Feed, Sink};
pub use isdb::descriptors::{
    ApplicationService, Assist, Component, Hierarchy, text as eight_unit_text,
};
pub use mpegts::demux::Counted;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    Video(Codec),
    Audio(Audio),
    /// Not decoded here, but told apart so a viewer can be told whether there are subtitles.
    Caption,
    Data,
    #[default]
    Other,
}

impl Kind {
    pub fn played(self) -> bool {
        matches!(self, Self::Video(_) | Self::Audio(_))
    }

    /// Type 0x06 is subtitles, superimposed text or data PES alike; descriptors tell them apart.
    pub fn of_stream_type(stream_type: u8, descriptors: &[isdb::tstables::Descriptor]) -> Self {
        use isdb::descriptors::{DATA_COMPONENT_CAPTION, DATA_COMPONENT_CAPTION_ONE_SEG, tag};
        if let Some(codec) = Codec::of_stream_type(stream_type) {
            return Self::Video(codec);
        }
        match stream_type {
            0x0f => Self::Audio(Audio::Adts),
            0x03 | 0x04 => Self::Audio(Audio::Mpeg),
            // AAC in LATM inside LOAS: a TS made out of an MMT/TLV broadcast. Without it there is no
            // sound, so no clock, and the picture never moves.
            0x11 => Self::Audio(Audio::Loas),
            0x06 => match isdb::tstables::find(descriptors, tag::DATA_COMPONENT)
                .and_then(|found| isdb::descriptors::data_component_id(&found.body))
            {
                Some(DATA_COMPONENT_CAPTION | DATA_COMPONENT_CAPTION_ONE_SEG) => Self::Caption,
                Some(_) => Self::Data,
                None => Self::Other,
            },
            0x0b | 0x0d => Self::Data,
            _ => Self::Other,
        }
    }

    pub fn of_asset(declared: &str) -> Self {
        if let Some(codec) = Codec::of_asset(declared) {
            return Self::Video(codec);
        }
        match declared {
            "mp4a" => Self::Audio(Audio::Latm),
            "stpp" => Self::Caption,
            "aapp" => Self::Data,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audio {
    Adts,
    Latm,
    Loas,
    Mpeg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub id: u16,
    pub kind: Kind,
    pub framing: Framing,
    pub declared: String,
    /// This, rather than the identifier, is what a stream is: an update may move a stream to
    /// another identifier.
    pub tag: Option<u16>,
    pub component: Option<Component>,
    pub name: Option<String>,
    pub hierarchy: Option<Hierarchy>,
    pub data: Option<isdb::bxml::DataComponent>,
    pub decode: Option<isdb::descriptors::VideoDecodeControl>,
}

#[derive(Debug, Clone, Copy)]
pub struct Unit<'a> {
    pub stream: u16,
    pub dts: Option<i64>,
    pub pts: Option<i64>,
    pub rap: bool,
    pub after_loss: bool,
    pub data: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub number: u16,
    /// ARIB pins a one-segment PMT to 0x1FC8..=0x1FCF, the only thing that tells it apart.
    pub map: u16,
    pub streams: Vec<Stream>,
    pub pcr: u16,
    pub ecm: Vec<(u16, u16)>,
    /// MMT のデータ放送のエントリー（ARIB STD-B60 7.4.3.39）。無ければ MH-AIT を見に行かない（TR-B39 11.1.1）。
    pub application: Option<ApplicationService>,
    pub region: Option<isdb::descriptors::TargetRegion>,
    pub emergency_news: Option<u64>,
    /// 緊急情報記述子（MH）（ARIB STD-B60 7.4.3.10）。TS では NIT に載るので空。
    pub emergency: Vec<isdb::nitdescriptors::EmergencyService>,
}

impl Program {
    pub fn one_seg(&self) -> bool {
        (0x1FC8..=0x1FCF).contains(&self.map)
    }

    pub fn carries(&self, id: u16, kind: Kind) -> bool {
        self.streams
            .iter()
            .any(|stream| stream.id == id && stream.kind == kind)
    }

    pub fn find(&self, wanted: impl Fn(Kind) -> bool) -> Option<&Stream> {
        self.streams.iter().find(|stream| wanted(stream.kind))
    }

    pub fn sounds(&self) -> impl Iterator<Item = &Stream> {
        self.streams
            .iter()
            .filter(|stream| matches!(stream.kind, Kind::Audio(_)))
    }

    pub fn sound(&self) -> Option<&Stream> {
        self.sounds().min_by_key(|stream| self.stand_in(stream))
    }

    pub fn sound_tagged(&self, tag: u16) -> Option<&Stream> {
        self.sounds().find(|stream| stream.tag == Some(tag))
    }

    pub fn partner(&self, stream: &Stream) -> Option<&Stream> {
        let pair = stream.hierarchy.as_ref()?;
        if let Some(reference) = pair.reference {
            return self.streams.iter().find(|other| other.id == reference);
        }
        let group = pair.group?;
        self.streams.iter().find(|other| {
            other.id != stream.id
                && other
                    .hierarchy
                    .as_ref()
                    .is_some_and(|one| one.group == Some(group))
        })
    }

    /// The level alone does not say it: BS marks its only sound low quality with reference 0x1FFF.
    pub fn stand_in(&self, stream: &Stream) -> bool {
        let Some(pair) = &stream.hierarchy else {
            return false;
        };
        pair.level > 0 && self.partner(stream).is_some()
    }

    pub fn pictures(&self) -> impl Iterator<Item = &Stream> {
        self.streams
            .iter()
            .filter(|stream| matches!(stream.kind, Kind::Video(_)))
    }

    pub fn picture(&self) -> Option<&Stream> {
        self.pictures().min_by_key(|stream| self.stand_in(stream))
    }

    pub fn picture_tagged(&self, tag: u16) -> Option<&Stream> {
        self.pictures().find(|stream| stream.tag == Some(tag))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Container {
    #[default]
    Ts,
    Tlv,
}

impl Container {
    pub fn framing(self) -> Framing {
        match self {
            Self::Ts => Framing::StartCodes,
            Self::Tlv => Framing::Lengths,
        }
    }

    pub fn of_path(path: &str) -> Option<Self> {
        match path.rsplit('.').next()? {
            "ts" | "m2ts" | "mts" => Some(Self::Ts),
            "tlv" | "mmts" => Some(Self::Tlv),
            _ => None,
        }
    }

    /// TS sync every 188 (or 192) bytes, TLV `0x7F` chained by length. The head may be cut.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        const SEARCH: usize = 64 * 1024;
        const RUN: usize = 4;
        let ts = |at: usize, stride: usize| {
            (0..RUN).all(|nth| bytes.get(at + nth * stride) == Some(&0x47))
        };
        let tlv = |mut at: usize| {
            for _ in 0..RUN {
                if bytes.get(at) != Some(&0x7f) {
                    return false;
                }
                let (Some(high), Some(low)) = (bytes.get(at + 2), bytes.get(at + 3)) else {
                    return false;
                };
                at += 4 + usize::from(u16::from_be_bytes([*high, *low]));
            }
            true
        };
        for at in 0..SEARCH.min(bytes.len()) {
            if ts(at, 188) || ts(at, 192) {
                return Some(Self::Ts);
            }
            if tlv(at) {
                return Some(Self::Tlv);
            }
        }
        None
    }

    pub fn of_file(path: &str) -> Option<Self> {
        use std::io::Read;
        let mut head = Vec::with_capacity(1 << 18);
        if let Ok(file) = std::fs::File::open(path) {
            let _ = file.take(1 << 18).read_to_end(&mut head);
        }
        Self::sniff(&head).or_else(|| Self::of_path(&path.to_ascii_lowercase()))
    }
}

/// ストリーム全体の受信の質。D（連続性指標の飛び）、E（エラーインジケーター）、S（読めないことを示すフラグ）。三つは別のことを
/// 表すので分けておく。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub packets: u64,
    pub dropped: u64,
    pub errors: u64,
    pub scrambled: u64,
}

impl Counts {
    pub fn clean(&self) -> bool {
        self.dropped == 0 && self.errors == 0 && self.scrambled == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sound_of_a_four_k_service_in_a_transport_stream_is_found() {
        assert_eq!(Kind::of_stream_type(0x11, &[]), Kind::Audio(Audio::Loas));
        assert_eq!(Kind::of_stream_type(0x0f, &[]), Kind::Audio(Audio::Adts));
        assert_eq!(Kind::of_stream_type(0x03, &[]), Kind::Audio(Audio::Mpeg));
        assert_eq!(Kind::of_stream_type(0x24, &[]), Kind::Video(Codec::Hevc));
        assert_eq!(Kind::of_stream_type(0x1b, &[]), Kind::Video(Codec::H264));
        assert_eq!(Kind::of_asset("hev1"), Kind::Video(Codec::Hevc));
        assert_eq!(Kind::of_asset("mp4a"), Kind::Audio(Audio::Latm));
        assert_eq!(Kind::of_asset("stpp"), Kind::Caption);
    }

    #[test]
    fn a_private_stream_is_named_by_its_data_component_descriptor() {
        let caption = vec![isdb::tstables::Descriptor {
            tag: isdb::descriptors::tag::DATA_COMPONENT,
            body: vec![0x00, 0x08, 0x3d, 0x00],
        }];
        assert_eq!(Kind::of_stream_type(0x06, &caption), Kind::Caption);
        let other = vec![isdb::tstables::Descriptor {
            tag: isdb::descriptors::tag::DATA_COMPONENT,
            body: vec![0x00, 0x0c],
        }];
        assert_eq!(Kind::of_stream_type(0x06, &other), Kind::Data);
        assert_eq!(Kind::of_stream_type(0x06, &[]), Kind::Other);
        assert_eq!(Kind::of_stream_type(0x0d, &[]), Kind::Data);
        assert!(!Kind::Caption.played());
        assert!(!Kind::Data.played());
        assert!(Kind::Audio(Audio::Adts).played());
    }

    #[test]
    fn a_name_says_which_transmission() {
        assert_eq!(Container::of_path("a/b.ts"), Some(Container::Ts));
        assert_eq!(Container::of_path("recording.mmts"), Some(Container::Tlv));
    }

    #[test]
    fn containers_are_told_by_their_bytes() {
        let mut ts = vec![0u8; 188 * 5 + 7];
        for nth in 0..5 {
            ts[7 + nth * 188] = 0x47;
        }
        assert_eq!(Container::sniff(&ts), Some(Container::Ts));
        let mut m2ts = vec![0u8; 192 * 5];
        for nth in 0..5 {
            m2ts[4 + nth * 192] = 0x47;
        }
        assert_eq!(Container::sniff(&m2ts), Some(Container::Ts));
        let mut tlv = vec![0x33u8; 3];
        for length in [10u16, 300, 0, 5] {
            tlv.extend_from_slice(&[0x7f, 0x01]);
            tlv.extend_from_slice(&length.to_be_bytes());
            tlv.extend(std::iter::repeat_n(0x11u8, usize::from(length)));
        }
        assert_eq!(Container::sniff(&tlv), Some(Container::Tlv));
        assert_eq!(Container::sniff(&[0u8; 4096]), None);
        assert_eq!(Container::of_path("x.mp4"), None);
    }
}
