// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 放送を映像と音声にする。ストリームを見つけ、アクセスユニットを切り出し、音声をデコードする。

use std::collections::{BTreeMap, VecDeque};

use crate::decode::Decoded;
use mediadecoder::audio::Sound;
use mediadecoder::es::{self, Codec, Format, Framing};

use crate::engine::clock::{Clock, Drift};
use crate::filters::analyzer::Analyzer;
use crate::filters::counter::Counter;
use crate::filters::logos::Logos;
use crate::filters::packet::ts::Mixing;
use crate::filters::packet::{PacketSink, Parser, Section};
use crate::filters::selector::{Changed, Selector};
use crate::filters::tables::Tables;
use crate::filters::units::Units;
use crate::filters::{
    Component, Container, Counted, Counts, Kind, Program, Stream, Unit, captions, data,
};
use crate::guide::Guide;
use crate::si;
pub use crate::sound::Passthrough;

pub mod clock;
pub mod decoding;

pub use crate::filters::selector::can_decode;

pub const QUEUE: u64 = 4_800;

/// 読めないパケットがいくつ届き、読めるものが無ければ通知するか。受信の開始直後はこう届くことがある。
const UNREADABLE_ENOUGH_VIDEO: u64 = 2000;
const UNREADABLE_ENOUGH_SOUND: u64 = 300;

pub const NOT_READABLE: &str = "映像・音声を読めない";
pub const MIXED_STREAMS: &str = "複数TS多重化方式の放送です";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DualMono {
    #[default]
    Main,
    Sub,
    Both,
}

impl DualMono {
    pub fn next(self) -> Self {
        match self {
            Self::Main => Self::Sub,
            Self::Sub => Self::Both,
            Self::Both => Self::Main,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Main => "主音声",
            Self::Sub => "副音声",
            Self::Both => "主+副",
        }
    }

    pub(crate) fn apply(self, sound: &mut Sound) {
        if sound.channels.len() != 2 {
            return;
        }
        match self {
            Self::Both => {}
            Self::Main => sound.channels[1] = sound.channels[0].clone(),
            Self::Sub => sound.channels[0] = sound.channels[1].clone(),
        }
    }
}

#[derive(Debug)]
pub enum Out {
    /// One coded video access unit, decoded by [`crate::engine::decoding::Decoding`].
    Access(Box<AccessUnit>),
    Picture {
        at: i64,
        picture: Decoded,
    },
    Sound {
        stream: u16,
        at: i64,
        sound: Sound,
    },
    /// One whole ADTS frame for passthrough ([`Engine::set_passthrough`]).
    Coded {
        at: i64,
        frame: Vec<u8>,
    },
    Trouble {
        stream: u16,
        what: String,
    },
    /// The programme was switched ([`Engine::want`]). Its clock need not continue the old one's,
    /// so a caller restarts its clock and drops what it queued.
    Switched,
    DataStreams(Vec<DataStream>),
    /// MMT のデータ放送があるか。`None` なら開いていたアプリを閉じる（TR-B39 11.1.2）。
    ApplicationService(Option<crate::filters::ApplicationService>),
    Data {
        pid: u16,
        section: Vec<u8>,
    },
    Item {
        pid: u16,
        mpu: u32,
        item: u32,
        fragmentation: u8,
        data: Vec<u8>,
    },
    CaptionStreams(Vec<CaptionStream>),
    Caption {
        pid: u16,
        mpu: u32,
        at: Option<i64>,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionStream {
    pub pid: u16,
    pub tag: u16,
    pub info: Option<isdb::mhcaption::SubtitleInfo>,
    /// One-segment (C profile) coding (TR-B14 Part 3, chapter 6).
    pub one_seg: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataStream {
    pub pid: u16,
    pub tag: u16,
    pub coding: u16,
    pub entry: bool,
    pub auto_start: bool,
    pub mmt: bool,
}

/// `tag` is what the picture comes back called.
#[derive(Debug, Clone)]
pub struct AccessUnit {
    pub stream: u16,
    pub codec: Codec,
    pub framing: Framing,
    pub tag: u64,
    pub at: Option<i64>,
    pub first: bool,
    pub picture: es::Picture,
    pub format: Format,
    pub data: Vec<u8>,
}

pub struct Engine {
    parser: Parser,
    analyzer: Analyzer,
    selector: Selector,
    units: Units,
    counter: Counter,
    dual_mono: DualMono,
    passthrough: Passthrough,
    clock: Clock,
    drift: Drift,
    out: VecDeque<Out>,
    guide: Guide,
    si: si::Info,
    logos: Logos,
    said_unreadable: Option<(Option<u16>, Option<u16>)>,
    /// 一つの TS でなければ（混ざっているか、まだ分からなければ）映像・音声を開かず、番組表も読まない。
    mixing: Mixing,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Engine")
            .field("chosen", &self.selector.chosen())
            .field("clock", &self.clock)
            .finish_non_exhaustive()
    }
}

struct Work<'a> {
    mixing: &'a mut Mixing,
    analyzer: &'a mut Analyzer,
    selector: &'a mut Selector,
    units: &'a mut Units,
    counter: &'a mut Counter,
    dual_mono: DualMono,
    passthrough: Passthrough,
    clock: &'a mut Clock,
    drift: &'a mut Drift,
    out: &'a mut VecDeque<Out>,
    guide: &'a mut Guide,
    si: &'a mut si::Info,
    logos: &'a mut Logos,
}

impl Work<'_> {
    fn tables(&mut self) -> Tables<'_> {
        Tables {
            guide: self.guide,
            si: self.si,
        }
    }

    fn program_changed(&mut self, program: &Program, had: Option<&Program>) {
        if *self.mixing != Mixing::One {
            return;
        }
        for id in had.iter().flat_map(|had| &had.streams).map(|s| s.id) {
            if program.streams.iter().any(|s| s.id == id) || self.analyzer.shared(id) {
                continue;
            }
            self.units.forget(id);
            self.counter.forget(id);
        }
        self.units.streams_changed(program);
        let changed = self.selector.follow(self.analyzer.known(), program);
        restart(self.units, self.out, self.clock, self.drift, changed);
        if self.selector.wanted() == Some(program.number) {
            data::announce(program, self.out);
        }
    }
}

impl PacketSink for Work<'_> {
    fn on_frame_header(&mut self, header: &isdb::tsmf::Info) {
        self.si.set_frame_header(header.clone());
    }

    fn on_mixing(&mut self, mixing: Mixing) {
        let was = std::mem::replace(self.mixing, mixing);
        if was == Mixing::One {
            *self.units = Units::new();
            self.out.clear();
            self.out.push_back(Out::Switched);
        }
        if mixing == Mixing::Mixed {
            self.out.push_back(Out::Trouble {
                stream: mpegts::demux::PAT_PID,
                what: MIXED_STREAMS.to_owned(),
            });
        } else if mixing == Mixing::One
            && let Some(program) = self
                .selector
                .wanted()
                .and_then(|number| self.analyzer.known().get(&number).cloned())
        {
            // 一つの TS と分かるまで開かなかった映像・音声を開く。
            self.program_changed(&program, None);
        }
    }

    fn on_programs(&mut self, numbers: &[u16]) {
        for program in self.analyzer.programs(numbers) {
            for stream in &program.streams {
                if !self.analyzer.shared(stream.id) {
                    self.units.forget(stream.id);
                    self.counter.forget(stream.id);
                }
            }
            if *self.mixing == Mixing::One && self.selector.wanted() == Some(program.number) {
                let changed = self.selector.lost(self.analyzer.known());
                restart(self.units, self.out, self.clock, self.drift, changed);
                self.out.push_back(Out::Trouble {
                    stream: program.map,
                    what: format!("番組 {} が放送から消えた", program.number),
                });
            }
        }
    }

    fn on_section(&mut self, section: &Section<'_>) {
        if *self.mixing != Mixing::One
            && section.origin == crate::filters::packet::Origin::Ts
            && crate::filters::tables::is_event_table(section.pid)
        {
            return;
        }
        let had = self
            .analyzer
            .known()
            .get(&section_program(section))
            .cloned();
        if let Some(program) = self.analyzer.section(section) {
            let had = had.filter(|had| had.number == program.number);
            self.program_changed(&program, had.as_ref());
            self.logos.on_program(&program);
        }
        self.tables().feed(section);
        if *self.mixing != Mixing::One {
            return;
        }
        if section.origin == crate::filters::packet::Origin::Ts
            && self.logos.wants(section.pid)
            && let Some(module) = self.logos.section(section.pid, section.data)
        {
            self.tables().on_logo_module(&module);
        }
        data::forward(
            self.selector.playing(self.analyzer.known()),
            section,
            self.out,
        );
    }

    fn on_mmt_table(&mut self, _packet_id: u16, table: &[u8]) {
        self.tables().mmt_table(table);
    }

    fn on_package(&mut self, _packet_id: u16, table: &isdb::mmttables::PackageTable) {
        let had = self.analyzer.known().get(&table.service_id()).cloned();
        if let Some(program) = self.analyzer.package(table) {
            self.program_changed(&program, had.as_ref());
        }
    }

    fn on_unit(&mut self, unit: &Unit<'_>) {
        if *self.mixing != Mixing::One {
            return;
        }
        let Some(kind) = self.analyzer.kind(unit.stream) else {
            return;
        };
        // 字幕・文字スーパー（ARIB STD-B24 第三編）は PES をそのまま渡す。
        if kind == Kind::Caption {
            let playing = self.selector.playing(self.analyzer.known());
            captions::forward(playing, unit.stream, 0, unit.pts, unit.data, self.out);
            return;
        }
        if !kind.played() {
            return;
        }
        self.counter.unit_arrived(unit.stream);
        self.units.push(
            unit,
            self.selector,
            self.passthrough,
            self.dual_mono,
            self.out,
        );
    }

    fn on_caption(&mut self, packet_id: u16, mpu: u32, at: Option<i64>, data: &[u8]) {
        let playing = self.selector.playing(self.analyzer.known());
        captions::forward(playing, packet_id, mpu, at, data, self.out);
    }

    fn on_item(&mut self, packet_id: u16, mpu: u32, item: u32, fragmentation: u8, data: &[u8]) {
        let playing = self.selector.playing(self.analyzer.known());
        data::forward_item(playing, packet_id, mpu, item, fragmentation, data, self.out);
    }
}

fn section_program(section: &Section<'_>) -> u16 {
    if !section.program_map {
        return 0;
    }
    section
        .data
        .get(3..5)
        .map_or(0, |bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
}

impl Engine {
    pub fn new(container: Container, rate: u32) -> Self {
        Self {
            parser: Parser::new(container),
            analyzer: Analyzer::new(),
            selector: Selector::new(),
            units: Units::new(),
            counter: Counter::new(),
            dual_mono: DualMono::default(),
            passthrough: Passthrough::Off,
            clock: Clock::new(rate),
            drift: Drift::new(QUEUE),
            out: VecDeque::new(),
            guide: Guide::new(),
            si: si::Info::new(),
            logos: Logos::new(),
            said_unreadable: None,
            mixing: Mixing::One,
        }
    }

    pub fn container(&self) -> Container {
        self.parser.container()
    }

    /// 複数の TS が混ざっている（TSMF の多重フレームを一つの TS に分けずに受けている）。
    pub fn mixed_streams(&self) -> bool {
        self.mixing == Mixing::Mixed
    }

    pub fn mixing(&self) -> Mixing {
        self.mixing
    }

    pub fn push(&mut self, bytes: &[u8]) {
        let mut kept = self.selector.sounds_beside().to_vec();
        kept.extend(self.selector.video());
        kept.extend(self.selector.sound());
        self.parser.keep(&kept);
        let mut work = Work {
            mixing: &mut self.mixing,
            analyzer: &mut self.analyzer,
            selector: &mut self.selector,
            units: &mut self.units,
            counter: &mut self.counter,
            dual_mono: self.dual_mono,
            passthrough: self.passthrough,
            clock: &mut self.clock,
            drift: &mut self.drift,
            out: &mut self.out,
            guide: &mut self.guide,
            si: &mut self.si,
            logos: &mut self.logos,
        };
        self.parser.push(bytes, &mut work);
        self.notice_unreadable();
    }

    fn notice_unreadable(&mut self) {
        if self.mixing != Mixing::One {
            return;
        }
        let playing = self.selector.chosen();
        if self.said_unreadable == Some(playing) {
            return;
        }
        for (id, enough) in [
            (playing.0, UNREADABLE_ENOUGH_VIDEO),
            (playing.1, UNREADABLE_ENOUGH_SOUND),
        ] {
            let Some(id) = id else {
                continue;
            };
            let scrambled = self.parser.scrambled(id);
            if scrambled < enough || self.counter.arrived(id) != 0 {
                continue;
            }
            self.said_unreadable = Some(playing);
            self.out.push_back(Out::Trouble {
                stream: id,
                what: format!("{NOT_READABLE}（{scrambled} パケット）"),
            });
            return;
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Out> {
        self.out.pop_front()
    }

    pub fn clock(&mut self) -> &mut Clock {
        &mut self.clock
    }

    pub fn ratio(&mut self) -> f64 {
        self.drift.update(self.clock.queued())
    }

    pub fn streams(&self) -> Vec<(u16, Kind)> {
        self.units.kinds()
    }

    pub fn chosen(&self) -> (Option<u16>, Option<u16>) {
        self.selector.chosen()
    }

    pub fn identifiers(&self) -> Vec<u16> {
        self.parser.identifiers()
    }

    pub fn counted(&self, stream: u16) -> Option<Counted> {
        self.parser.counted(stream)
    }

    pub fn counts(&self) -> Counts {
        if self.mixing != Mixing::One {
            return self.parser.counts();
        }
        self.counter.counts(&self.parser, self.playing())
    }

    pub fn guide(&self) -> &Guide {
        &self.guide
    }

    pub fn guide_mut(&mut self) -> &mut Guide {
        &mut self.guide
    }

    pub fn si(&self) -> &si::Info {
        &self.si
    }

    /// PAT・PLT に載っているサービス。放送休止の判定に使う（ARIB TR-B14 第二部 表 6-4）。
    pub fn listed(&self) -> Option<Vec<u16>> {
        match self.container() {
            Container::Ts => self.analyzer.listed().map(<[u16]>::to_vec),
            Container::Tlv => self.si.listed_packages(),
        }
    }

    pub fn programs(&self) -> &BTreeMap<u16, Program> {
        self.analyzer.known()
    }

    pub fn play(&mut self, program: u16) -> bool {
        let changed = self.selector.play(self.analyzer.known(), program);
        self.restart_if(changed)
    }

    pub fn best(&self) -> Option<u16> {
        Selector::best(
            self.analyzer.known(),
            |id| self.parser.scrambled(id),
            |id| self.counter.arrived(id),
        )
    }

    pub fn wanted(&self) -> Option<u16> {
        self.selector.wanted()
    }

    pub fn want(&mut self, program: u16) {
        let was = self.playing().map(|program| program.number);
        let Some(changed) = self.selector.want(self.analyzer.known(), program) else {
            return;
        };
        self.restart(changed);
        if was.is_some_and(|was| was != program) {
            self.out.push_back(Out::Switched);
            let playing = self.selector.playing(self.analyzer.known());
            self.counter.rebase(&self.parser, playing);
        }
        // テーブルは変わらないが、データ放送と字幕のコンポーネントは変わる。
        if self.mixing == Mixing::One
            && let Some(found) = self.analyzer.known().get(&program)
        {
            data::announce(found, &mut self.out);
        }
    }

    fn playing(&self) -> Option<&Program> {
        self.selector.playing(self.analyzer.known())
    }

    pub fn pictures(&self) -> Vec<Stream> {
        self.selector.pictures(self.analyzer.known())
    }

    pub fn picture_at(&self) -> Option<usize> {
        let video = self.selector.video()?;
        self.pictures().iter().position(|stream| stream.id == video)
    }

    pub fn play_picture(&mut self, id: u16) -> bool {
        let changed = self.selector.play_picture(self.analyzer.known(), id);
        self.restart_if(changed)
    }

    pub fn next_picture(&mut self) -> Option<Stream> {
        let pictures = self.pictures();
        if pictures.len() < 2 {
            return None;
        }
        let at = self.picture_at().map_or(0, |at| (at + 1) % pictures.len());
        let next = pictures[at].clone();
        self.play_picture(next.id).then_some(next)
    }

    pub fn is_stand_in(&self, id: u16) -> bool {
        self.selector.is_stand_in(self.analyzer.known(), id)
    }

    pub fn picture_is_stand_in(&self) -> bool {
        self.selector.video().is_some_and(|id| self.is_stand_in(id))
    }

    /// A sound the PMT does not describe is described by the EIT, else by the decoder.
    pub fn sounds(&self) -> Vec<Stream> {
        let Some(program) = self.playing() else {
            return Vec::new();
        };
        let mut sounds: Vec<Stream> = program.sounds().cloned().collect();
        let guide = self.guide();
        let described = self
            .si
            .identity()
            .zip(guide.now())
            .and_then(|(identity, now)| {
                guide.at(
                    crate::guide::Key {
                        network: identity.network,
                        service: program.number,
                    },
                    now,
                )
            })
            .map(|programme| programme.audios.clone())
            .unwrap_or_default();
        for (n, stream) in sounds.iter_mut().enumerate() {
            let said = described
                .iter()
                .find(|sound| sound.tag.is_some() && sound.tag == stream.tag)
                .or_else(|| described.get(n));
            match (&mut stream.component, said) {
                (Some(component), Some(said)) => component.enrich(said),
                (Some(_), None) => {}
                (None, Some(said)) => stream.component = Some(said.clone()),
                (None, None) => stream.component = self.heard_component(stream.id, stream.tag),
            }
        }
        sounds
    }

    /// Whether this stream turned out to be HE-AAC; nothing in the tables says.
    pub fn sound_is_he_aac(&self, stream: u16) -> bool {
        self.units.heard(stream).is_some_and(|heard| heard.sbr)
    }

    pub fn sound_rate(&self, stream: u16) -> Option<u32> {
        self.units.heard(stream).and_then(|heard| heard.rate)
    }

    fn heard_component(&self, stream: u16, tag: Option<u16>) -> Option<Component> {
        let heard = self.units.heard(stream)?;
        let kind = match (heard.channels, heard.dual) {
            (0, _) => return None,
            (1, _) => 0x01,
            (2, true) => 0x02,
            (2, false) => 0x03,
            (6, _) => 0x09,
            (24, _) => 0x11,
            _ => return None,
        };
        Some(Component {
            kind,
            tag,
            language: String::new(),
            second: None,
            main: false,
            name: None,
            quality: 0,
            sampling: 0,
            simulcast: None,
        })
    }

    pub fn sound_at(&self) -> Option<usize> {
        let sound = self.selector.sound()?;
        self.sounds().iter().position(|stream| stream.id == sound)
    }

    /// Every sound of the programme is decoded, so the change is immediate.
    pub fn play_sound(&mut self, id: u16) -> bool {
        let sounds = self.sounds();
        let changed = self.selector.play_sound(self.analyzer.known(), &sounds, id);
        self.restart_if(changed)
    }

    pub fn next_sound(&mut self) -> Option<Stream> {
        let sounds = self.sounds();
        if sounds.len() < 2 {
            return None;
        }
        let at = self.sound_at().map_or(0, |at| (at + 1) % sounds.len());
        let next = sounds[at].clone();
        self.play_sound(next.id).then_some(next)
    }

    pub fn dual_mono(&self) -> DualMono {
        self.dual_mono
    }

    pub fn set_dual_mono(&mut self, mode: DualMono) {
        self.dual_mono = mode;
    }

    /// Hands sound over coded ([`Out::Coded`]) for an IEC 61937 link. 22.2ch cannot be passed
    /// through (ADTS cannot name it).
    pub fn set_passthrough(&mut self, on: Passthrough) {
        self.passthrough = on;
    }

    pub fn sound_is_dual_mono(&self) -> bool {
        let Some(sound) = self.selector.sound() else {
            return false;
        };
        let said = self
            .sounds()
            .iter()
            .find(|stream| stream.id == sound)
            .and_then(|stream| stream.component.as_ref())
            .is_some_and(|component| component.dual_mono());
        said || self.units.heard(sound).is_some_and(|heard| heard.dual)
    }

    fn restart_if(&mut self, changed: Option<Changed>) -> bool {
        let Some(changed) = changed else {
            return false;
        };
        self.restart(changed);
        true
    }

    fn restart(&mut self, changed: Changed) {
        restart(
            &mut self.units,
            &mut self.out,
            &mut self.clock,
            &mut self.drift,
            changed,
        );
    }
}

/// A sound change must not restart the picture. Nothing where nothing changed: the tables repeat.
fn restart(
    units: &mut Units,
    out: &mut VecDeque<Out>,
    clock: &mut Clock,
    drift: &mut Drift,
    changed: Changed,
) {
    if !changed.any() {
        return;
    }
    if changed.video {
        clock.reset();
    }
    if changed.sound {
        drift.reset();
    }
    out.retain(|held| match held {
        Out::Access(_) | Out::Picture { .. } => !changed.video,
        Out::Sound { .. } | Out::Coded { .. } => !changed.sound,
        Out::Trouble { .. }
        | Out::Switched
        | Out::DataStreams(_)
        | Out::ApplicationService(_)
        | Out::Data { .. }
        | Out::Item { .. }
        | Out::CaptionStreams(_)
        | Out::Caption { .. } => true,
    });
    units.restart(changed.video, changed.sound);
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{DualMono, Out};
    use crate::filters::units::{hand_coded, hand_over};
    use mediadecoder::audio::{Sound, Speaker};

    fn pair() -> Sound {
        Sound {
            rate: 48_000,
            channels: vec![vec![1.0, 1.0], vec![-1.0, -1.0]],
            speakers: vec![Speaker::FrontLeft, Speaker::FrontRight],
            mixdown: Default::default(),
        }
    }

    #[test]
    fn wanting_a_known_programme_announces_its_data_streams() {
        use crate::filters::{Container, Kind, Program, Stream};
        let mut engine = super::Engine::new(Container::Ts, 48_000);
        let data = Stream {
            id: 0x040c,
            kind: Kind::Data,
            framing: mediadecoder::es::Framing::StartCodes,
            declared: String::from("0x0d"),
            tag: Some(0x40),
            component: None,
            name: None,
            hierarchy: None,
            data: Some(isdb::bxml::DataComponent {
                id: 0x0007,
                additional: vec![0x30, 0x40, 0x0f, 0x00],
            }),
            decode: None,
        };
        engine.analyzer.insert(Program {
            number: 531,
            map: 0x1fc8,
            streams: vec![data],
            pcr: 0x1fff,
            ecm: Vec::new(),
            application: None,
            region: None,
            emergency_news: None,
            emergency: Vec::new(),
        });
        engine.want(531);
        let announced = std::iter::from_fn(|| engine.next())
            .find_map(|out| match out {
                Out::DataStreams(streams) => Some(streams),
                _ => None,
            })
            .expect("コンポーネントが通知される");
        assert_eq!(announced.len(), 1);
        assert_eq!(announced[0].tag, 0x40);
        assert!(announced[0].entry && announced[0].auto_start);
    }

    #[test]
    fn a_half_of_a_dual_mono_pair_is_played_on_both_sides() {
        let mut main = pair();
        DualMono::Main.apply(&mut main);
        assert_eq!(main.channels, vec![vec![1.0, 1.0], vec![1.0, 1.0]]);

        let mut sub = pair();
        DualMono::Sub.apply(&mut sub);
        assert_eq!(sub.channels, vec![vec![-1.0, -1.0], vec![-1.0, -1.0]]);

        let mut both = pair();
        DualMono::Both.apply(&mut both);
        assert_eq!(both.channels, pair().channels);
    }

    #[test]
    fn a_stream_that_is_not_dual_mono_is_handed_over_as_it_is() {
        let Out::Sound { at, sound, .. } = hand_over(1, 90_000, pair(), false, DualMono::Sub)
        else {
            panic!("sound came back as something else");
        };
        assert_eq!(at, 90_000);
        assert_eq!(sound.channels, pair().channels);

        let Out::Sound { sound, .. } = hand_over(1, 0, pair(), true, DualMono::Sub) else {
            panic!("sound came back as something else");
        };
        assert_eq!(
            sound.channels,
            vec![vec![-1.0, -1.0], vec![-1.0, -1.0]],
            "the mode was not obeyed for a stream the map did call dual mono"
        );
    }

    #[test]
    fn coded_frames_come_out_whole_and_in_time() {
        use crate::sound::iec61937;
        use mediadecoder::audio::{Format, Object, adts};

        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 6,
        };
        let mut out = VecDeque::new();
        let mut at = Some(90_000);
        let mut refused = false;
        for _ in 0..3 {
            hand_coded(
                format,
                1,
                &[0xa5; 400],
                &mut at,
                &mut refused,
                0x1100,
                &mut out,
            );
        }
        assert!(!refused);
        assert_eq!(out.len(), 3);

        let times: Vec<_> = out
            .iter()
            .map(|one| match one {
                Out::Coded { at, .. } => *at,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(times, [90_000, 91_920, 93_840]);
        assert_eq!(at, Some(95_760));

        let Some(Out::Coded { frame, .. }) = out.pop_front() else {
            panic!("a coded frame");
        };
        let header = adts::Header::read(&frame).expect("a header");
        assert_eq!(header.format, format);
        assert_eq!(header.length, 407);
        assert_eq!(frame.len(), 407);
        let mut burst = Vec::new();
        assert_eq!(iec61937::pack(&frame, &mut burst).expect("a burst"), 1024);
    }

    #[test]
    fn a_high_efficiency_frame_is_worth_the_same_span() {
        use mediadecoder::audio::{Format, Object};

        let format = Format {
            object: Object::Sbr,
            rate: 24000,
            configuration: 2,
        };
        let mut out = VecDeque::new();
        let mut at = Some(0);
        let mut refused = false;
        hand_coded(
            format,
            1,
            &[0xa5; 100],
            &mut at,
            &mut refused,
            0x1100,
            &mut out,
        );
        assert_eq!(at, Some(3840));
    }

    #[test]
    fn sound_that_cannot_be_passed_through_is_said_once() {
        use mediadecoder::audio::{Format, Object};

        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 0,
        };
        let mut out = VecDeque::new();
        let mut at = Some(0);
        let mut refused = false;
        for _ in 0..50 {
            hand_coded(
                format,
                1,
                &[0xa5; 400],
                &mut at,
                &mut refused,
                0x1100,
                &mut out,
            );
        }
        assert!(refused);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Out::Trouble { stream: 0x1100, .. }));
        assert_eq!(at, Some(0));
    }

    #[test]
    fn a_coded_frame_with_no_time_is_dropped() {
        use mediadecoder::audio::{Format, Object};

        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 2,
        };
        let mut out = VecDeque::new();
        let mut at = None;
        let mut refused = false;
        hand_coded(
            format,
            1,
            &[0xa5; 400],
            &mut at,
            &mut refused,
            0x1100,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn sound_that_is_not_a_pair_is_left_alone() {
        for count in [1usize, 6] {
            let mut sound = Sound {
                rate: 48_000,
                channels: (0..count).map(|n| vec![n as f32]).collect(),
                speakers: Vec::new(),
                mixdown: Default::default(),
            };
            let had = sound.channels.clone();
            DualMono::Sub.apply(&mut sound);
            assert_eq!(sound.channels, had, "{count} channels were rearranged");
        }
    }

    #[test]
    fn the_modes_go_round() {
        let mut mode = DualMono::default();
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(mode);
            mode = mode.next();
        }
        assert_eq!(
            seen,
            [
                DualMono::Main,
                DualMono::Sub,
                DualMono::Both,
                DualMono::Main
            ]
        );
    }
}
