// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! アクセスユニットをストリームごとに受けるフィルター。映像は切り出して符号化データのまま [`Out::Access`] に、音声はデコードして
//! [`Out::Sound`] に（パススルーなら ADTS で [`Out::Coded`]）。

use std::collections::{BTreeMap, VecDeque};

use mediadecoder::audio::{self, Sound, adts, latm};
use mediadecoder::es::{self, Access, Codec, Format, Framing, Structure};

use super::selector::Selector;
use super::{Audio, Kind, Program, Stream, Unit};
use crate::engine::clock::Wrap;
use crate::engine::{AccessUnit, DualMono, Out, Passthrough};
use crate::sound::iec61937;

pub const MOST_WAITED: u32 = 60;

#[derive(Default)]
pub struct Units {
    tracks: BTreeMap<u16, Track>,
}

impl std::fmt::Debug for Units {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Units")
            .field("streams", &self.kinds())
            .finish()
    }
}

impl Units {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn kinds(&self) -> Vec<(u16, Kind)> {
        self.tracks
            .iter()
            .map(|(id, track)| (*id, track.kind()))
            .collect()
    }

    pub fn heard(&self, id: u16) -> Option<&Heard> {
        match self.tracks.get(&id) {
            Some(Track::Sound(track)) => Some(&track.heard),
            _ => None,
        }
    }

    pub fn forget(&mut self, id: u16) {
        self.tracks.remove(&id);
    }

    pub fn streams_changed(&mut self, program: &Program) {
        for stream in &program.streams {
            self.stream_changed(stream);
        }
    }

    fn stream_changed(&mut self, stream: &Stream) {
        match stream.kind {
            // 別のコーデックのデコーダーに渡すと壊れた画像が出るだけ。
            Kind::Video(codec) => {
                let stale = match self.tracks.get(&stream.id) {
                    Some(Track::Video(track)) => track.codec != codec,
                    _ => true,
                };
                if stale {
                    self.tracks
                        .insert(stream.id, video_track(codec, stream.framing));
                }
            }
            Kind::Audio(framing) => {
                let dual = stream
                    .component
                    .as_ref()
                    .is_some_and(|component| component.dual_mono());
                let stale = match self.tracks.get(&stream.id) {
                    Some(Track::Sound(track)) => track.framing != framing || track.dual != dual,
                    _ => true,
                };
                if stale {
                    self.tracks.insert(stream.id, sound_track(framing, dual));
                }
            }
            Kind::Caption | Kind::Data | Kind::Other => {
                self.tracks.remove(&stream.id);
            }
        }
    }

    pub fn restart(&mut self, video: bool, sound: bool) {
        for track in self.tracks.values_mut() {
            match track {
                Track::Video(_) if video => track.reset(),
                Track::Sound(_) if sound => track.reset(),
                _ => {}
            }
        }
    }

    pub fn push(
        &mut self,
        unit: &Unit<'_>,
        heard: &Selector,
        passthrough: Passthrough,
        mode: DualMono,
        out: &mut VecDeque<Out>,
    ) {
        let chosen_sound = Some(unit.stream) == heard.sound();
        let beside = !chosen_sound && heard.sounds_beside().contains(&unit.stream);
        if Some(unit.stream) != heard.video()
            && !chosen_sound
            && !(beside && passthrough != Passthrough::On)
        {
            return;
        }
        // 選んでいない音声もデコードする（切り替えを待たせないため）。
        let passes = |format: &audio::Format| chosen_sound && passthrough.passes(format);
        let Some(track) = self.tracks.get_mut(&unit.stream) else {
            return;
        };
        match track {
            Track::Video(track) => {
                let mut found: Vec<Access> = Vec::new();
                track.splitter.push(unit.data, &mut found);
                if found.is_empty() {
                    track.blind += 1;
                    if track.blind == MOST_WAITED {
                        out.push_back(Out::Trouble {
                            stream: unit.stream,
                            what: format!(
                                "{MOST_WAITED} パケット読んでも映像のストリーム情報が来ない"
                            ),
                        });
                    }
                } else {
                    track.blind = 0;
                }
                let mut shown: i64 = 0;
                let period = {
                    let mut starting = 0i64;
                    let mut format = None;
                    for access in &found {
                        if access.format.is_some() {
                            format = access.format;
                            if access.picture.structure != Structure::Bottom {
                                starting += 1;
                            }
                        }
                    }
                    match (unit.pts, format) {
                        (Some(pts), Some(format)) => {
                            let at = track.wrap.feed(pts);
                            track.period(at, starting, format)
                        }
                        _ => 0,
                    }
                };
                for access in &found {
                    let Some(format) = access.format else {
                        continue;
                    };
                    let tag = track.fed;
                    track.fed += 1;
                    let starts = track.starts_frame(access.picture.structure);
                    if starts {
                        shown += 1;
                    }
                    // A sequence header is not an entry. Counting still happens for pictures passed
                    // over, since their place sets the times of the ones after.
                    if !track.entered {
                        track.waited += 1;
                        if access.picture.random_access {
                            track.entered = true;
                        // 諦めて始めるにしてもイントラピクチャーから（参照の無い予測ピクチャーでデコーダーが壊れることがある）。
                        } else if track.waited >= MOST_WAITED
                            && access.picture.coding == es::Coding::I
                        {
                            track.entered = true;
                            out.push_back(Out::Trouble {
                                stream: unit.stream,
                                what: format!(
                                    "{} 枚待ってもランダムアクセスポイントが無いので、イントラピクチャーから始める",
                                    track.waited
                                ),
                            });
                        } else {
                            continue;
                        }
                        // ランダムアクセスポイントの直後の先行ピクチャーは渡さない（H.265 8.1.3）。
                        track.leading = true;
                        track.entry = None;
                    }
                    let at = unit
                        .pts
                        .map(|pts| track.wrap.feed(pts) + (shown - 1) * period);
                    if track.leading {
                        match track.entry {
                            None => track.entry = at.or(Some(i64::MIN)),
                            // H.264 は先行ピクチャーを NAL の種類で見分けられないので時刻で見る。
                            Some(entry) => {
                                if access.picture.leading || at.is_some_and(|at| at < entry) {
                                    continue;
                                }
                                // 同じ時刻のピクチャーはランダムアクセスポイントのもう一方のフィールド（フィールド構造ピクチャー）。
                                if at.is_some_and(|at| at > entry) {
                                    track.leading = false;
                                }
                            }
                        }
                    }
                    out.push_back(Out::Access(Box::new(AccessUnit {
                        stream: unit.stream,
                        codec: track.codec,
                        framing: track.framing,
                        tag,
                        at,
                        first: std::mem::take(&mut track.fresh),
                        picture: access.picture,
                        format,
                        data: unit.data[access.range.clone()].to_vec(),
                    })));
                }
            }
            Track::Sound(track) => {
                // 録画のドロップで音声の時刻が映像から遅れていくので、離れたら PES の時刻に合わせ直す。
                if let Some(pts) = unit.pts {
                    let declared = track.wrap.feed(pts);
                    match track.at {
                        Some(running) if (running - declared).abs() <= SOUND_RESYNC => {}
                        _ => track.at = Some(declared),
                    }
                }
                let SoundTrack {
                    framing,
                    dual,
                    heard,
                    declared_rate,
                    latm,
                    decoder,
                    held,
                    at,
                    refused,
                    ..
                } = track.as_mut();
                let dual = *dual;
                match framing {
                    Audio::Adts => {
                        held.extend_from_slice(unit.data);
                        let taken = {
                            let mut frames = adts::Frames::new(held);
                            for frame in frames.by_ref() {
                                if passes(&frame.format) {
                                    hand_coded(
                                        frame.format,
                                        frame.blocks,
                                        frame.data,
                                        at,
                                        refused,
                                        unit.stream,
                                        out,
                                    );
                                } else {
                                    // ワンセグの HE-AAC は ADTS にコアの 24 kHz が書かれる。
                                    *declared_rate = Some(frame.format.rate);
                                    decode_sound(
                                        decoder.as_mut(),
                                        frame.format,
                                        frame.data,
                                        unit.stream,
                                        out,
                                    );
                                }
                            }
                            held.len() - frames.rest().len()
                        };
                        held.drain(..taken);
                    }
                    Audio::Loas | Audio::Latm => {
                        let loas = *framing == Audio::Loas;
                        let mut frames = Vec::new();
                        if loas {
                            held.extend_from_slice(unit.data);
                            let taken = latm.push(held, &mut frames);
                            held.drain(..taken);
                        } else {
                            latm.unit(unit.data, &mut frames);
                        }
                        for frame in &frames {
                            if passes(&frame.format) {
                                hand_coded(
                                    frame.format,
                                    1,
                                    &frame.data,
                                    at,
                                    refused,
                                    unit.stream,
                                    out,
                                );
                            } else {
                                if loas {
                                    *declared_rate = Some(frame.format.rate);
                                }
                                decode_sound(
                                    decoder.as_mut(),
                                    frame.format,
                                    &frame.data,
                                    unit.stream,
                                    out,
                                );
                            }
                        }
                    }
                    Audio::Mpeg => {
                        held.extend_from_slice(unit.data);
                        let taken = {
                            let mut frames = audio::mpeg::Frames::new(held);
                            for frame in frames.by_ref() {
                                decode_sound(
                                    decoder.as_mut(),
                                    frame.format,
                                    frame.data,
                                    unit.stream,
                                    out,
                                );
                            }
                            held.len() - frames.rest().len()
                        };
                        held.drain(..taken);
                    }
                }
                if passthrough != Passthrough::On || *framing == Audio::Mpeg {
                    drain_sound(
                        decoder.as_mut(),
                        unit.stream,
                        at,
                        dual,
                        heard,
                        *declared_rate,
                        mode,
                        out,
                    );
                }
            }
        }
    }
}

pub(crate) enum Track {
    Video(Box<VideoTrack>),
    Sound(Box<SoundTrack>),
}

pub(crate) struct VideoTrack {
    pub(crate) codec: Codec,
    pub(crate) framing: Framing,
    pub(crate) splitter: es::Splitter,
    pub(crate) wrap: Wrap,
    pub(crate) fed: u64,
    pub(crate) entered: bool,
    pub(crate) leading: bool,
    pub(crate) entry: Option<i64>,
    pub(crate) fresh: bool,
    pub(crate) waited: u32,
    /// Packets with no picture for want of parameter sets. Said once (it looks like a fault).
    pub(crate) blind: u32,
    pub(crate) half: Option<Structure>,
    pub(crate) last_unit: Option<(i64, i64)>,
}

/// A one-segment PMT says nothing of its sound, so the stream is the only word.
#[derive(Debug, Default)]
pub struct Heard {
    pub dual: bool,
    pub channels: usize,
    pub sbr: bool,
    pub rate: Option<u32>,
}

pub(crate) struct SoundTrack {
    pub(crate) framing: Audio,
    pub(crate) dual: bool,
    pub(crate) declared_rate: Option<u32>,
    pub(crate) heard: Heard,
    pub(crate) latm: latm::Parser,
    pub(crate) decoder: Box<dyn audio::Decoder + Send>,
    pub(crate) held: Vec<u8>,
    pub(crate) wrap: Wrap,
    pub(crate) at: Option<i64>,
    pub(crate) refused: bool,
}

impl VideoTrack {
    pub(crate) fn starts_frame(&mut self, structure: Structure) -> bool {
        match structure {
            Structure::Frame => {
                self.half = None;
                true
            }
            parity => match self.half.take() {
                Some(waiting) if waiting != parity => false,
                _ => {
                    self.half = Some(parity);
                    true
                }
            },
        }
    }

    /// Measured from the stream, not the sequence header (ワンセグ declares 15 fps and sends three
    /// every 300 ms). Where the count changes, or after a jump, the declared rate is used.
    pub(crate) fn period(&mut self, at: i64, pictures: i64, format: Format) -> i64 {
        let declared = frame(format);
        let measured = match self.last_unit {
            Some((last, held)) if held > 0 && at > last => {
                let interval = at - last;
                (interval <= crate::engine::clock::HZ && (held == pictures || declared == 0))
                    .then_some(interval / held)
            }
            _ => None,
        };
        self.last_unit = Some((at, pictures));
        measured.unwrap_or(declared)
    }
}

const SOUND_RESYNC: i64 = crate::engine::clock::HZ / 10;

pub(crate) fn frame(format: Format) -> i64 {
    if format.rate.0 == 0 {
        return 0;
    }
    i64::from(format.rate.1) * crate::engine::clock::HZ / i64::from(format.rate.0)
}

impl Track {
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Video(track) => Kind::Video(track.codec),
            Self::Sound(track) => Kind::Audio(track.framing),
        }
    }

    pub(crate) fn reset(&mut self) {
        match self {
            Self::Video(track) => {
                track.wrap.reset();
                track.half = None;
                track.entered = false;
                track.leading = false;
                track.entry = None;
                track.fresh = true;
                track.waited = 0;
                track.blind = 0;
                track.last_unit = None;
            }
            Self::Sound(track) => {
                track.latm.reset();
                track.decoder.reset();
                track.held.clear();
                track.wrap.reset();
                track.at = None;
            }
        }
    }
}

/// A fresh ADTS shell (the original's CRC position words are stripped).
pub(crate) fn hand_coded(
    format: audio::Format,
    blocks: usize,
    data: &[u8],
    at: &mut Option<i64>,
    refused: &mut bool,
    stream: u16,
    out: &mut VecDeque<Out>,
) {
    let Some(start) = *at else {
        return;
    };
    let Some(header) = adts::write_header(format, blocks, data.len()) else {
        if !*refused {
            *refused = true;
            out.push_back(Out::Trouble {
                stream,
                what: format!(
                    "{format:?} cannot be framed as ADTS and so cannot be passed through"
                ),
            });
        }
        return;
    };
    let mut frame = Vec::with_capacity(header.len() + data.len());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(data);
    let samples = (blocks * iec61937::BLOCK) as i64;
    *at = Some(start + samples * crate::engine::clock::HZ / i64::from(format.rate.max(1)));
    out.push_back(Out::Coded { at: start, frame });
}

pub(crate) fn decode_sound(
    decoder: &mut dyn audio::Decoder,
    format: audio::Format,
    data: &[u8],
    stream: u16,
    out: &mut VecDeque<Out>,
) {
    if let Err(error) = decoder.decode(format, data) {
        out.push_back(Out::Trouble {
            stream,
            what: format!("{error:?}"),
        });
    }
}

pub(crate) fn hand_over(stream: u16, at: i64, mut sound: Sound, dual: bool, mode: DualMono) -> Out {
    if dual {
        mode.apply(&mut sound);
    }
    Out::Sound { stream, at, sound }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_sound(
    decoder: &mut dyn audio::Decoder,
    stream: u16,
    at: &mut Option<i64>,
    dual: bool,
    heard: &mut Heard,
    declared_rate: Option<u32>,
    mode: DualMono,
    out: &mut VecDeque<Out>,
) {
    while let Some(sound) = decoder.take() {
        let Some(start) = *at else {
            continue;
        };
        let length = sound.len() as i64 * crate::engine::clock::HZ / i64::from(sound.rate.max(1));
        *at = Some(start + length);
        heard.dual = sounds_dual_mono(&sound);
        heard.channels = sound.channels.len();
        heard.sbr = declared_rate.is_some_and(|declared| sound.rate > declared);
        heard.rate = Some(sound.rate);
        out.push_back(hand_over(stream, start, sound, dual || heard.dual, mode));
    }
}

pub(crate) fn sounds_dual_mono(sound: &Sound) -> bool {
    sound.channels.len() == 2
        && sound.speakers.len() == 2
        && sound
            .speakers
            .iter()
            .all(|speaker| *speaker == audio::Speaker::Unplaced)
}

pub(crate) fn video_track(codec: Codec, framing: Framing) -> Track {
    Track::Video(Box::new(VideoTrack {
        codec,
        framing,
        splitter: es::Splitter::new(codec, framing),
        wrap: Wrap::new(),
        fed: 0,
        entered: false,
        leading: false,
        entry: None,
        fresh: true,
        waited: 0,
        blind: 0,
        half: None,
        last_unit: None,
    }))
}

pub(crate) fn sound_track(framing: Audio, dual: bool) -> Track {
    Track::Sound(Box::new(SoundTrack {
        framing,
        dual,
        latm: latm::Parser::new(),
        decoder: crate::decode::sound::open(match framing {
            Audio::Mpeg => crate::decode::sound::SoundCodec::Mpeg,
            _ => crate::decode::sound::SoundCodec::Aac,
        }),
        held: Vec::new(),
        wrap: Wrap::new(),
        at: None,
        refused: false,
        declared_rate: None,
        heard: Heard::default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fifteen() -> Format {
        Format {
            rate: (15_000, 1_001),
            ..Format::default()
        }
    }

    #[test]
    fn several_pictures_under_one_time_are_spaced_by_the_measured_interval() {
        let Track::Video(mut track) = video_track(Codec::H264, Framing::StartCodes) else {
            unreachable!()
        };
        assert_eq!(track.period(0, 3, fifteen()), 6_006);
        assert_eq!(track.period(27_027, 3, fifteen()), 9_009);
        assert_eq!(track.period(54_054, 3, fifteen()), 9_009);
    }

    #[test]
    fn a_unit_with_a_different_count_falls_back_to_the_declared_rate() {
        let Track::Video(mut track) = video_track(Codec::H264, Framing::StartCodes) else {
            unreachable!()
        };
        track.period(0, 3, fifteen());
        assert_eq!(track.period(27_027, 2, fifteen()), 6_006);
        assert_eq!(track.period(45_045, 5, fifteen()), 6_006);
        assert_eq!(track.period(75_075, 5, fifteen()), 6_006);
        assert_eq!(track.period(75_075 + 2 * 90_000, 5, fifteen()), 6_006);
    }
}
