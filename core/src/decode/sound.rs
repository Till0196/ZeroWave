// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 音声デコーダーの優先順。映像と同じく、上から試して最後は ZeroWave。
//! 22.2ch（チャンネル構成 0 で PCE を使う物）は OS のデコーダーに任せない。拒否されたら ZeroWave で
//! デコードし直す。OS の出力は ZeroWave のチャンネル順に並べ替える。
//! 二か国語の AAC（チャンネル構成 0 で PCE なしの SCE 二つ、ARIB STD-B32）は、前に SCE を二つ置く PCE を
//! 設定に入れて渡す。

use mediadecoder::audio::{self, Format, Sound, Speaker};

#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
static SAY: crate::trace::Trace = crate::trace::Trace::of("sound");

#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
mod audiotoolbox;
#[cfg(all(feature = "android", target_os = "android"))]
mod mediacodec;
#[cfg(all(feature = "mediafoundation", windows))]
mod mediafoundation;

#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
use audiotoolbox::{AtSound as Os, usable};
#[cfg(all(feature = "android", target_os = "android"))]
use mediacodec::{McSound as Os, usable};
#[cfg(all(feature = "mediafoundation", windows))]
use mediafoundation::{MfSound as Os, usable};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundCodec {
    Aac,
    /// MPEG-1 Audio（Layer II、まれに Layer I）。
    Mpeg,
}

impl SoundCodec {
    pub fn name(self) -> &'static str {
        match self {
            Self::Aac => "AAC",
            Self::Mpeg => "MPEG-1 Audio",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundPick {
    /// OS のデコーダー。名前があればその一つ（Windows の MFT の名前など）。
    System(Option<String>),
    ZeroWave,
}

impl SoundPick {
    pub fn parse(spelled: &str) -> Option<Self> {
        let spelled = spelled.trim();
        let (way, name) = match spelled.split_once(':') {
            Some((way, name)) => (way, Some(name.trim())),
            None => (spelled, None),
        };
        match way.trim().to_ascii_lowercase().as_str() {
            "system" => Some(Self::System(
                name.filter(|name| !name.is_empty()).map(str::to_owned),
            )),
            "zerowave" => Some(Self::ZeroWave),
            _ => None,
        }
    }

    pub fn spelling(&self) -> String {
        match self {
            Self::System(Some(name)) => format!("system:{name}"),
            Self::System(None) => String::from("system"),
            Self::ZeroWave => String::from("zerowave"),
        }
    }
}

/// コーデックごとの優先順（設定の `audio.decoder.*`）。空は ZeroWave だけ。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundPlan {
    pub aac: Vec<SoundPick>,
    pub mpeg: Vec<SoundPick>,
}

impl SoundPlan {
    pub fn of(&self, codec: SoundCodec) -> &[SoundPick] {
        match codec {
            SoundCodec::Aac => &self.aac,
            SoundCodec::Mpeg => &self.mpeg,
        }
    }
}

static PLAN: std::sync::Mutex<SoundPlan> = std::sync::Mutex::new(SoundPlan {
    aac: Vec::new(),
    mpeg: Vec::new(),
});

/// 優先順を差し替える。次に開く音声デコーダーから有効になる。
pub fn set_sound_plan(plan: SoundPlan) {
    if let Ok(mut slot) = PLAN.lock() {
        *slot = plan;
    }
}

pub fn sound_plan() -> SoundPlan {
    PLAN.lock().map(|slot| slot.clone()).unwrap_or_default()
}

/// OS の音声デコーダーのインターフェース（[`audio::Decoder`] に理由・名前・任せてよいかを足す）。
trait System: Send {
    fn name(&self) -> &str;
    fn accepts(&self, format: Format, data: &[u8]) -> bool;
    fn decode(&mut self, format: Format, data: &[u8]) -> Result<(), String>;
    fn take(&mut self) -> Option<Sound>;
    fn flush(&mut self);
    fn reset(&mut self);
}

fn zerowave(codec: SoundCodec) -> Box<dyn audio::Decoder + Send> {
    match codec {
        SoundCodec::Aac => Box::new(audio::aac::Aac::new()),
        SoundCodec::Mpeg => Box::new(audio::mpeg::Mpeg::new()),
    }
}

pub fn open(codec: SoundCodec) -> Box<dyn audio::Decoder + Send> {
    let own = zerowave(codec);
    let mut refused = None;
    for pick in sound_plan().of(codec) {
        match pick {
            SoundPick::ZeroWave => break,
            SoundPick::System(name) => match open_system(codec, name.as_deref()) {
                Ok(system) => {
                    tracing::info!("音声デコーダー {}: {}", codec.name(), system.name());
                    tell(codec, system.name(), None);
                    return Box::new(Chosen {
                        codec,
                        system: Some(system),
                        own,
                        paired: false,
                    });
                }
                Err(why) => {
                    tracing::warn!("音声 {}: OS のデコーダーを開けない（{why}）", codec.name());
                    refused = Some("システムのデコーダーは利用できません");
                }
            },
        }
    }
    tell(codec, zerowave_name(codec), refused);
    own
}

/// 内蔵のデコーダー（ZeroWave）の名前。
pub fn zerowave_name(codec: SoundCodec) -> &'static str {
    match codec {
        SoundCodec::Aac => "ZeroWave AAC Audio Decoder",
        SoundCodec::Mpeg => "ZeroWave MPEG Audio Decoder",
    }
}

/// いま音声をデコードしているデコーダー。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Using {
    pub codec: SoundCodec,
    pub name: String,
    /// OS のデコーダーを選んでいたのに ZeroWave でデコードしているなら、その理由。
    pub why: Option<String>,
}

static USING: std::sync::Mutex<Option<Using>> = std::sync::Mutex::new(None);

pub fn using() -> Option<Using> {
    USING.lock().ok().and_then(|slot| slot.clone())
}

fn tell(codec: SoundCodec, name: &str, why: Option<&str>) {
    if let Ok(mut slot) = USING.lock() {
        *slot = Some(Using {
            codec,
            name: name.to_owned(),
            why: why.map(str::to_owned),
        });
    }
}

/// このコーデックに使える OS のデコーダーの名前。
pub fn offers(codec: SoundCodec) -> Vec<String> {
    #[cfg(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android"),
        all(feature = "videotoolbox", target_vendor = "apple")
    ))]
    {
        usable(codec)
    }
    #[cfg(not(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android"),
        all(feature = "videotoolbox", target_vendor = "apple")
    )))]
    {
        let _ = codec;
        Vec::new()
    }
}

fn open_system(codec: SoundCodec, name: Option<&str>) -> Result<Box<dyn System>, String> {
    #[cfg(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android"),
        all(feature = "videotoolbox", target_vendor = "apple")
    ))]
    {
        Os::open(codec, name).map(|opened| Box::new(opened) as Box<dyn System>)
    }
    #[cfg(not(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android"),
        all(feature = "videotoolbox", target_vendor = "apple")
    )))]
    {
        let _ = (codec, name);
        Err(String::from("このビルドに OS の音声デコーダーはありません"))
    }
}

/// OS のデコーダーでデコードし、任せられないフレームや拒否されたフレームは ZeroWave で。
struct Chosen {
    codec: SoundCodec,
    /// 拒否されたら `None` になり、以後はずっと ZeroWave。
    system: Option<Box<dyn System>>,
    own: Box<dyn audio::Decoder + Send>,
    /// 二か国語の AAC を渡した。出てくる二つのチャンネルは左右ではない。
    paired: bool,
}

impl Chosen {
    fn give_up(&mut self, why: &str) {
        if let Some(system) = self.system.take() {
            tracing::warn!(
                "音声 {}: {} から ZeroWave に切り替える（{why}）",
                self.codec.name(),
                system.name()
            );
            tell(
                self.codec,
                zerowave_name(self.codec),
                Some(&format!("{}：{why}", system.name())),
            );
        }
    }
}

impl audio::Decoder for Chosen {
    fn decode(&mut self, format: Format, data: &[u8]) -> Result<(), audio::Error> {
        if let Some(system) = self.system.as_mut() {
            self.paired = self.codec == SoundCodec::Aac && paired(format, data);
            if !system.accepts(format, data) {
                self.give_up("非対応形式（22.2ch など）");
            } else {
                match system.decode(format, data) {
                    Ok(()) => return Ok(()),
                    Err(why) => self.give_up(&why),
                }
            }
        }
        self.own.decode(format, data)
    }

    fn take(&mut self) -> Option<Sound> {
        match self.system.as_mut() {
            Some(system) => {
                let mut sound = system.take()?;
                if self.paired && sound.channels.len() == 2 {
                    sound.speakers = vec![Speaker::Unplaced; 2];
                }
                Some(sound)
            }
            None => self.own.take(),
        }
    }

    fn flush(&mut self) {
        if let Some(system) = self.system.as_mut() {
            system.flush();
        }
        self.own.flush();
    }

    fn reset(&mut self) {
        if let Some(system) = self.system.as_mut() {
            system.reset();
        }
        self.own.reset();
    }
}

/// 二か国語の AAC か。チャンネル構成 0 で、ブロックが PCE でなく SCE で始まる物（ARIB STD-B32 第二部 5.2.3）。
/// 頭のフィル要素は飛ばして見る。
pub(super) fn paired(format: Format, data: &[u8]) -> bool {
    const SCE: u32 = 0;
    const FIL: u32 = 6;
    if format.configuration != 0 {
        return false;
    }
    let mut reader = mediadecoder::bits::Reader::new(data);
    loop {
        if reader.left() < 3 {
            return false;
        }
        match reader.bits(3) {
            SCE => return true,
            FIL => {
                let mut count = reader.bits(4) as usize;
                if count == 15 {
                    count = 14 + reader.bits(8) as usize;
                }
                if reader.left() < count * 8 {
                    return false;
                }
                reader.skip(count * 8);
            }
            _ => return false,
        }
    }
}

/// 二か国語の AAC の `AudioSpecificConfig`。チャンネル構成 0 に、前の SCE 二つ（タグ 0 と 1）だけの
/// PCE（ISO/IEC 14496-3 4.4.1.1）を付ける。
#[cfg_attr(
    not(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android"),
        all(feature = "videotoolbox", target_vendor = "apple")
    )),
    allow(dead_code)
)]
fn paired_config(config: [u8; 2]) -> Vec<u8> {
    let index = u128::from((u16::from_be_bytes(config) >> 7) & 15);
    let fields: [(u128, u32); 14] = [
        (2, 5), // AAC-LC
        (index, 4),
        (0, 4), // チャンネル構成 0
        (0, 3), // GASpecificConfig：1024 サンプル、コアなし、拡張なし
        (0, 4), // PCE のタグ
        (1, 2), // LC
        (index, 4),
        (2, 4),  // 前の要素が二つ
        (0, 17), // 横・後ろ・LFE・付随データ・カップリングは無し
        (0, 3),  // ダウンミックスの指定は無し
        (0, 5),  // SCE、タグ 0
        (1, 5),  // SCE、タグ 1
        (0, 4),  // バイト境界（ここまで 60 ビット）
        (0, 8),  // コメントは無し
    ];
    let (bits, length) = fields
        .iter()
        .fold((0u128, 0u32), |(bits, length), (value, count)| {
            ((bits << count) | value, length + count)
        });
    bits.to_be_bytes()[16 - (length / 8) as usize..].to_vec()
}

/// AAC の `AudioSpecificConfig`（ISO/IEC 14496-3 1.6.2.1）の二バイト。HE-AAC も LC として渡す。
#[cfg_attr(
    not(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android"),
        all(feature = "videotoolbox", target_vendor = "apple")
    )),
    allow(dead_code)
)]
fn audio_specific_config(format: Format) -> Option<[u8; 2]> {
    const RATES: [u32; 12] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000,
    ];
    let index = RATES.iter().position(|rate| *rate == format.rate)? as u16;
    let object: u16 = match format.object {
        audio::Object::Main => 1,
        audio::Object::Ltp => 4,
        _ => 2,
    };
    let bits = (object << 11) | (index << 7) | (u16::from(format.configuration & 15) << 3);
    Some(bits.to_be_bytes())
}

/// チャンネル構成が示すチャンネル数（7 は 7.1ch で八つ）。
#[cfg_attr(
    not(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android"),
        all(feature = "videotoolbox", target_vendor = "apple")
    )),
    allow(dead_code)
)]
fn channels_of(configuration: u8) -> Option<u32> {
    match configuration {
        1..=6 => Some(u32::from(configuration)),
        7 => Some(8),
        _ => None,
    }
}

/// OS のチャンネル順を ZeroWave のチャンネル順へ並べ替える。無ければリアとサイドを同じものと見なす。
#[cfg_attr(
    not(any(
        all(feature = "mediafoundation", windows),
        all(feature = "android", target_os = "android")
    )),
    allow(dead_code)
)]
fn arrange(sound: &mut Sound, wanted: &[Speaker]) {
    if wanted.len() != sound.speakers.len() {
        return;
    }
    let alike = |speaker: Speaker| match speaker {
        Speaker::SideLeft => Speaker::BackLeft,
        Speaker::SideRight => Speaker::BackRight,
        other => other,
    };
    let mut order = Vec::with_capacity(wanted.len());
    let mut used = vec![false; sound.speakers.len()];
    for want in wanted {
        let found = (0..sound.speakers.len())
            .find(|at| !used[*at] && sound.speakers[*at] == *want)
            .or_else(|| {
                (0..sound.speakers.len())
                    .find(|at| !used[*at] && alike(sound.speakers[*at]) == alike(*want))
            });
        let Some(at) = found else {
            return;
        };
        used[at] = true;
        order.push(at);
    }
    let channels = std::mem::take(&mut sound.channels);
    let mut taken: Vec<Option<Vec<f32>>> = channels.into_iter().map(Some).collect();
    sound.channels = order
        .iter()
        .map(|at| taken[*at].take().unwrap_or_default())
        .collect();
    sound.speakers = wanted.to_vec();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_read_back_as_written() {
        for spelled in [
            "system",
            "system:Microsoft AAC Audio Decoder MFT",
            "zerowave",
        ] {
            let pick = SoundPick::parse(spelled).expect("読める");
            assert_eq!(pick.spelling(), spelled);
        }
        assert_eq!(SoundPick::parse("system:"), Some(SoundPick::System(None)));
        assert_eq!(SoundPick::parse("d3d12"), None);
    }

    #[test]
    fn the_config_of_stereo_lc_at_48k() {
        let format = Format {
            object: audio::Object::Lc,
            rate: 48_000,
            configuration: 2,
        };
        assert_eq!(audio_specific_config(format), Some([0x11, 0x90]));
    }

    #[test]
    fn a_pair_of_singles_is_spelled_with_a_program_element() {
        assert_eq!(
            paired_config([0x11, 0x80]),
            [0x11, 0x80, 0x04, 0xC8, 0x00, 0x00, 0x00, 0x10, 0x00]
        );
    }

    #[test]
    fn a_block_starting_with_a_single_after_a_fill_is_a_pair() {
        let format = Format {
            object: audio::Object::Lc,
            rate: 48_000,
            configuration: 0,
        };
        // FIL（3 バイト）→ SCE。
        assert!(paired(format, &[0xC6, 0x00, 0x00, 0x00, 0x00]));
        // PCE で始まるのは 22.2ch などの配置の指定。
        assert!(!paired(format, &[0xA0, 0x00]));
        assert!(!paired(
            Format {
                configuration: 2,
                ..format
            },
            &[0x00, 0x00]
        ));
    }

    #[test]
    fn five_one_from_wave_order_lands_in_zerowave_order() {
        use Speaker::*;
        let wave = [
            FrontLeft,
            FrontRight,
            FrontCentre,
            LowFrequency,
            SideLeft,
            SideRight,
        ];
        let mut sound = Sound {
            rate: 48_000,
            channels: (0..6).map(|at| vec![at as f32]).collect(),
            speakers: wave.to_vec(),
            ..Sound::default()
        };
        let wanted = audio::config::Layout::of_configuration(6)
            .expect("5.1")
            .speakers();
        arrange(&mut sound, &wanted);
        assert_eq!(sound.speakers, wanted);
        let centre = wanted
            .iter()
            .position(|speaker| *speaker == FrontCentre)
            .expect("中央");
        assert_eq!(sound.channels[centre], [2.0]);
    }
}
