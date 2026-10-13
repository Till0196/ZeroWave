// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What the engine does while the transmission changes under it (programmes leaving, sounds
//! appearing, identifiers reassigned). Built rather than recorded, so each change is exact.

use mediadecoder::es::Codec;
use mpegts::{PACKET, SYNC, crc32};
use zwcore::engine::{DualMono, Engine, Out};
use zwcore::filters::{Container, Kind};

const PAT_PID: u16 = 0x0000;
const EIT_PID: u16 = 0x0012;
const EIT_OWN: u8 = 0x4e;

const ONE: u16 = 101;
const ONE_MAP: u16 = 0x0101;
const ONE_VIDEO: u16 = 0x0111;
const ONE_SOUND: u16 = 0x0121;
const TWO: u16 = 102;
const TWO_MAP: u16 = 0x0102;
const TWO_VIDEO: u16 = 0x0112;
const TWO_SOUND: u16 = 0x0122;

const MPEG2: u8 = 0x02;
const H264: u8 = 0x1b;
const AAC: u8 = 0x0f;

fn pat(version: u8, programs: &[(u16, u16)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&[0xC1 | (version << 1), 0x00, 0x00]);
    for (number, pid) in programs {
        body.extend_from_slice(&number.to_be_bytes());
        body.extend_from_slice(&(pid | 0xE000).to_be_bytes());
    }
    section(0x00, &body)
}

fn pmt(program: u16, version: u8, streams: &[(u8, u16)]) -> Vec<u8> {
    let told: Vec<Told> = streams
        .iter()
        .map(|(stream_type, pid)| Told {
            stream_type: *stream_type,
            pid: *pid,
            tag: None,
            sound: None,
            pair: None,
        })
        .collect();
    described(program, version, &told)
}

#[derive(Clone, Copy)]
struct Told {
    stream_type: u8,
    pid: u16,
    tag: Option<u8>,
    sound: Option<(u8, &'static str)>,
    /// A hierarchical transmission descriptor: whether this is the picture to show, and the
    /// other half's identifier.
    pair: Option<(bool, u16)>,
}

impl Told {
    fn video(pid: u16) -> Self {
        Self {
            stream_type: MPEG2,
            pid,
            tag: None,
            sound: None,
            pair: None,
        }
    }

    fn tagged_video(pid: u16, tag: u8) -> Self {
        Self {
            tag: Some(tag),
            ..Self::video(pid)
        }
    }

    fn half(pid: u16, tag: u8, show: bool, other: u16) -> Self {
        Self {
            stream_type: MPEG2,
            pid,
            tag: Some(tag),
            sound: None,
            pair: Some((show, other)),
        }
    }

    fn sound(pid: u16, tag: u8, kind: u8, language: &'static str) -> Self {
        Self {
            stream_type: AAC,
            pid,
            tag: Some(tag),
            sound: Some((kind, language)),
            pair: None,
        }
    }

    fn descriptors(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(tag) = self.tag {
            out.extend_from_slice(&[0x52, 0x01, tag]);
        }
        if let Some((show, other)) = self.pair {
            out.extend_from_slice(&[
                0xc0,
                0x03,
                0xfe | u8::from(show),
                0xe0 | (other >> 8) as u8,
                other as u8,
            ]);
        }
        if let Some((kind, language)) = self.sound {
            let mut body = vec![0x02, kind, self.tag.unwrap_or(0), 0x0f, 0xff, 0x40];
            body.extend_from_slice(language.as_bytes());
            out.push(0xc4);
            out.push(body.len() as u8);
            out.extend_from_slice(&body);
        }
        out
    }
}

fn described(program: u16, version: u8, streams: &[Told]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&program.to_be_bytes());
    body.extend_from_slice(&[0xC1 | (version << 1), 0x00, 0x00]);
    let pcr = streams.first().map_or(0x1FFF, |told| told.pid);
    body.extend_from_slice(&(pcr | 0xE000).to_be_bytes());
    body.extend_from_slice(&0xF000u16.to_be_bytes());
    for told in streams {
        let descriptors = told.descriptors();
        body.push(told.stream_type);
        body.extend_from_slice(&(told.pid | 0xE000).to_be_bytes());
        body.extend_from_slice(&(0xF000 | descriptors.len() as u16).to_be_bytes());
        body.extend_from_slice(&descriptors);
    }
    section(0x02, &body)
}

/// The present event of a service, describing these sounds by component tag (a TS puts the
/// audio component descriptor here, not in the map).
fn eit(service: u16, version: u8, sounds: &[Described]) -> Vec<u8> {
    eit_at(service, version, 0, sounds)
}

#[derive(Clone, Copy)]
struct Described {
    tag: u8,
    kind: u8,
    language: &'static str,
    main: bool,
    /// What the transmission calls it, in ARIB's eight-unit code (neither ASCII nor UTF-8).
    name: &'static [u8],
}

impl Described {
    fn new(tag: u8, kind: u8, language: &'static str, main: bool) -> Self {
        Self {
            tag,
            kind,
            language,
            main,
            name: &[],
        }
    }

    fn called(self, name: &'static [u8]) -> Self {
        Self { name, ..self }
    }
}

fn eit_at(service: u16, version: u8, number: u8, sounds: &[Described]) -> Vec<u8> {
    eit_full(service, version, number, sounds, &[])
}

fn eit_full(
    service: u16,
    version: u8,
    number: u8,
    sounds: &[Described],
    pictures: &[(u8, &[u8])],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&service.to_be_bytes());
    body.extend_from_slice(&[0xC1 | (version << 1), number, 0x01]);
    body.extend_from_slice(&[0x00, 0x01, 0x00, 0x02, 0x00, EIT_OWN]);
    let mut described = Vec::new();
    for (tag, name) in pictures {
        let mut one = vec![0xf1, 0xb3, *tag];
        one.extend_from_slice(b"jpn");
        one.extend_from_slice(name);
        described.push(0x50);
        described.push(one.len() as u8);
        described.extend_from_slice(&one);
    }
    for sound in sounds {
        let mut one = vec![
            0xf2,
            sound.kind,
            sound.tag,
            AAC,
            0xff,
            if sound.main { 0x4f } else { 0x0f },
        ];
        one.extend_from_slice(sound.language.as_bytes());
        one.extend_from_slice(sound.name);
        described.push(0xc4);
        described.push(one.len() as u8);
        described.extend_from_slice(&one);
    }
    body.extend_from_slice(&1234u16.to_be_bytes());
    body.extend_from_slice(&[0xff; 8]);
    body.extend_from_slice(&(0x8000 | described.len() as u16).to_be_bytes());
    body.extend_from_slice(&described);
    section(EIT_OWN, &body)
}

fn section(table_id: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![table_id, 0x00, 0x00];
    out.extend_from_slice(body);
    let length = out.len() + 4 - 3;
    out[1] = 0xB0 | ((length >> 8) as u8 & 0x0F);
    out[2] = (length & 0xFF) as u8;
    out.extend_from_slice(&crc32(&out).to_be_bytes());
    out
}

#[derive(Default)]
struct Tape {
    bytes: Vec<u8>,
    counters: std::collections::BTreeMap<u16, u8>,
}

impl Tape {
    fn send(&mut self, pid: u16, section: &[u8]) -> &mut Self {
        let counter = self.counters.entry(pid).or_default();
        let mut packet = [0xFFu8; PACKET];
        packet[0] = SYNC;
        packet[1] = 0x40 | ((pid >> 8) as u8 & 0x1F);
        packet[2] = (pid & 0xFF) as u8;
        packet[3] = 0x10 | (*counter & 0x0F);
        *counter = counter.wrapping_add(1);
        let mut payload = vec![0x00];
        payload.extend_from_slice(section);
        assert!(
            payload.len() <= PACKET - 4,
            "a section that needs two packets"
        );
        packet[4..4 + payload.len()].copy_from_slice(&payload);
        self.bytes.extend_from_slice(&packet);
        self
    }

    fn both(&mut self) -> &mut Self {
        self.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP), (TWO, TWO_MAP)]))
            .send(
                ONE_MAP,
                &pmt(ONE, 0, &[(MPEG2, ONE_VIDEO), (AAC, ONE_SOUND)]),
            )
            .send(
                TWO_MAP,
                &pmt(TWO, 0, &[(MPEG2, TWO_VIDEO), (AAC, TWO_SOUND)]),
            )
    }
}

fn engine() -> Engine {
    Engine::new(Container::Ts, 48_000)
}

fn open(engine: &Engine) -> Vec<u16> {
    let mut out: Vec<u16> = engine.streams().into_iter().map(|(id, _)| id).collect();
    out.sort_unstable();
    out
}

fn trouble(engine: &mut Engine) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(got) = engine.next() {
        if let Out::Trouble { what, .. } = got {
            out.push(what);
        }
    }
    out
}

#[test]
fn what_arrived_on_each_identifier_is_counted() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.push(&tape.bytes);

    assert_eq!(engine.identifiers(), [PAT_PID, ONE_MAP, TWO_MAP]);
    let counted = engine
        .counted(PAT_PID)
        .expect("association table が数えられていない");
    assert_eq!(counted.arrived, 1);
    assert_eq!(counted.passed, 1);
    assert_eq!(counted.lost, 0);
    assert_eq!(counted.errors, 0);
    assert_eq!(
        engine.counted(ONE_VIDEO),
        None,
        "何も来ていない識別子は無い"
    );
}

#[test]
fn a_programme_the_table_stops_naming_is_forgotten() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.push(&tape.bytes);
    assert_eq!(engine.programs().len(), 2);
    assert_eq!(open(&engine), [ONE_VIDEO, TWO_VIDEO, ONE_SOUND, TWO_SOUND]);

    // The multiplexer goes on sending the dropped programme's map; that is ordinary.
    let mut later = Tape::default();
    later.send(PAT_PID, &pat(1, &[(ONE, ONE_MAP)]));
    for version in 1..4 {
        later.send(
            TWO_MAP,
            &pmt(TWO, version, &[(MPEG2, TWO_VIDEO), (AAC, TWO_SOUND)]),
        );
    }
    engine.push(&later.bytes);
    assert_eq!(
        engine.programs().keys().copied().collect::<Vec<_>>(),
        [ONE],
        "a programme that is off the air is still described"
    );
    assert_eq!(
        open(&engine),
        [ONE_VIDEO, ONE_SOUND],
        "the decoders of a programme that has left are still open"
    );
}

/// A programme that comes back with the same map, byte for byte, is described again.
#[test]
fn a_programme_that_comes_back_is_described_again() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.push(&tape.bytes);

    let mut gone = Tape::default();
    gone.send(PAT_PID, &pat(1, &[(ONE, ONE_MAP)]));
    engine.push(&gone.bytes);
    assert_eq!(engine.programs().len(), 1);

    let mut back = Tape::default();
    back.send(PAT_PID, &pat(2, &[(ONE, ONE_MAP), (TWO, TWO_MAP)]))
        .send(
            TWO_MAP,
            &pmt(TWO, 0, &[(MPEG2, TWO_VIDEO), (AAC, TWO_SOUND)]),
        );
    engine.push(&back.bytes);
    assert_eq!(
        engine.programs().keys().copied().collect::<Vec<_>>(),
        [ONE, TWO],
        "a programme that came back with the map it left with was not taken"
    );
    assert_eq!(open(&engine), [ONE_VIDEO, TWO_VIDEO, ONE_SOUND, TWO_SOUND]);
}

#[test]
fn a_stream_another_programme_still_names_is_kept() {
    let shared = ONE_SOUND;
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP), (TWO, TWO_MAP)]))
        .send(ONE_MAP, &pmt(ONE, 0, &[(MPEG2, ONE_VIDEO), (AAC, shared)]))
        .send(TWO_MAP, &pmt(TWO, 0, &[(MPEG2, TWO_VIDEO), (AAC, shared)]));
    let mut engine = engine();
    engine.push(&tape.bytes);
    assert_eq!(open(&engine), [ONE_VIDEO, TWO_VIDEO, shared]);

    let mut later = Tape::default();
    later.send(ONE_MAP, &pmt(ONE, 1, &[(MPEG2, ONE_VIDEO)]));
    engine.push(&later.bytes);
    assert!(
        open(&engine).contains(&shared),
        "a stream the other programme still names was dropped: {:?}",
        open(&engine)
    );

    let mut last = Tape::default();
    last.send(TWO_MAP, &pmt(TWO, 1, &[(MPEG2, TWO_VIDEO)]));
    engine.push(&last.bytes);
    assert_eq!(
        open(&engine),
        [ONE_VIDEO, TWO_VIDEO],
        "the last programme let go and the stream stayed"
    );
}

/// An identifier given to another coding gets the decoder for that coding (silent when wrong).
#[test]
fn an_identifier_given_to_another_coding_is_decoded_as_it() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)])).send(
        ONE_MAP,
        &pmt(ONE, 0, &[(MPEG2, ONE_VIDEO), (AAC, ONE_SOUND)]),
    );
    let mut engine = engine();
    engine.push(&tape.bytes);
    assert_eq!(
        engine
            .streams()
            .into_iter()
            .find(|(id, _)| *id == ONE_VIDEO),
        Some((ONE_VIDEO, Kind::Video(Codec::Mpeg2)))
    );

    let mut later = Tape::default();
    later.send(
        ONE_MAP,
        &pmt(ONE, 1, &[(H264, ONE_VIDEO), (AAC, ONE_SOUND)]),
    );
    engine.push(&later.bytes);
    assert_eq!(
        engine
            .streams()
            .into_iter()
            .find(|(id, _)| *id == ONE_VIDEO),
        Some((ONE_VIDEO, Kind::Video(Codec::H264))),
        "the identifier kept the decoder of the coding it used to carry"
    );
}

#[test]
fn an_identifier_that_becomes_data_loses_its_decoder() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)])).send(
        ONE_MAP,
        &pmt(ONE, 0, &[(MPEG2, ONE_VIDEO), (AAC, ONE_SOUND)]),
    );
    let mut engine = engine();
    engine.push(&tape.bytes);
    assert_eq!(open(&engine), [ONE_VIDEO, ONE_SOUND]);

    let mut later = Tape::default();
    later.send(
        ONE_MAP,
        &pmt(ONE, 1, &[(MPEG2, ONE_VIDEO), (0x0d, ONE_SOUND)]),
    );
    engine.push(&later.bytes);
    assert_eq!(open(&engine), [ONE_VIDEO]);
}

#[test]
fn what_is_played_follows_the_map() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert_eq!(engine.chosen(), (Some(ONE_VIDEO), Some(ONE_SOUND)));

    let moved = 0x0113;
    let mut later = Tape::default();
    later.send(ONE_MAP, &pmt(ONE, 1, &[(MPEG2, moved), (AAC, ONE_SOUND)]));
    engine.push(&later.bytes);
    assert_eq!(
        engine.chosen(),
        (Some(moved), Some(ONE_SOUND)),
        "the picture that is played is the one that has gone"
    );
}

#[test]
fn switching_programme_starts_the_counts_afresh() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert!(engine.counts().packets > 0, "何も数えていない");
    engine.want(TWO);
    assert_eq!(engine.counts().packets, 0, "前の番組のぶんが残っている");
    let mut more = Tape::default();
    more.both();
    engine.push(&more.bytes);
    assert!(engine.counts().packets > 0);
}

/// A map that repeats unchanged is not news (else the player never starts).
#[test]
fn a_map_that_repeats_unchanged_changes_nothing() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    let before = engine.chosen();

    let mut again = Tape::default();
    for _ in 0..8 {
        again
            .send(PAT_PID, &pat(0, &[(ONE, ONE_MAP), (TWO, TWO_MAP)]))
            .send(
                ONE_MAP,
                &pmt(ONE, 0, &[(MPEG2, ONE_VIDEO), (AAC, ONE_SOUND)]),
            );
    }
    engine.push(&again.bytes);
    assert_eq!(engine.chosen(), before);
    assert_eq!(open(&engine), [ONE_VIDEO, TWO_VIDEO, ONE_SOUND, TWO_SOUND]);
    assert!(
        trouble(&mut engine).is_empty(),
        "a map that said nothing new was reported as something"
    );
}

#[test]
fn a_change_to_a_stream_nobody_plays_leaves_the_clock_alone() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    engine.clock().wrote(90_000, 480);
    assert_eq!(engine.clock().now(), Some(90_000));

    let mut later = Tape::default();
    later.send(
        ONE_MAP,
        &pmt(
            ONE,
            1,
            &[(MPEG2, ONE_VIDEO), (AAC, ONE_SOUND), (0x06, 0x0131)],
        ),
    );
    engine.push(&later.bytes);
    assert_eq!(engine.chosen(), (Some(ONE_VIDEO), Some(ONE_SOUND)));
    assert_eq!(
        engine.clock().now(),
        Some(90_000),
        "the clock was started over for a stream nobody is playing"
    );
}

#[test]
fn a_programme_being_watched_that_leaves_says_so() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(TWO);
    engine.push(&tape.bytes);
    assert_eq!(engine.chosen(), (Some(TWO_VIDEO), Some(TWO_SOUND)));

    let mut later = Tape::default();
    later.send(PAT_PID, &pat(1, &[(ONE, ONE_MAP)]));
    engine.push(&later.bytes);
    assert_eq!(engine.chosen(), (None, None));
    let said = trouble(&mut engine);
    assert!(
        said.iter().any(|what| what.contains("102")),
        "nothing was said about the programme that left: {said:?}"
    );
}

const MONO: u8 = 0x01;
const DUAL: u8 = 0x02;
const STEREO: u8 = 0x03;

fn two_sounds(version: u8) -> Vec<u8> {
    described(
        ONE,
        version,
        &[
            Told::video(ONE_VIDEO),
            Told::sound(ONE_SOUND, 0x10, STEREO, "jpn"),
            Told::sound(0x0122, 0x11, MONO, "eng"),
        ],
    )
}

#[test]
fn what_the_map_says_about_a_sound_is_read() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &two_sounds(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let sounds = engine.sounds();
    assert_eq!(sounds.len(), 2);
    assert_eq!(sounds[0].tag, Some(0x10));
    assert_eq!(sounds[1].tag, Some(0x11));
    let first = sounds[0].component.as_ref().expect("a described sound");
    assert_eq!(first.language, "jpn");
    assert_eq!(first.channels(), 2);
    assert!(!first.dual_mono());
    let second = sounds[1].component.as_ref().expect("a described sound");
    assert_eq!(second.language, "eng");
    assert_eq!(second.channels(), 1);
    assert_eq!(engine.sound_at(), Some(0));
}

#[test]
fn the_sound_a_viewer_chose_is_the_one_played() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &two_sounds(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert_eq!(engine.chosen().1, Some(ONE_SOUND));

    let next = engine.next_sound().expect("a second sound");
    assert_eq!(next.id, 0x0122);
    assert_eq!(engine.chosen().1, Some(0x0122));
    assert_eq!(engine.sound_at(), Some(1));

    assert_eq!(engine.next_sound().map(|stream| stream.id), Some(ONE_SOUND));
}

/// The sound a viewer chose survives a map update that reorders the list (why the tag is read).
#[test]
fn the_chosen_sound_survives_the_list_being_reordered() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &two_sounds(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    engine.next_sound().expect("a second sound");
    assert_eq!(engine.chosen().1, Some(0x0122));

    let mut later = Tape::default();
    later.send(
        ONE_MAP,
        &described(
            ONE,
            1,
            &[
                Told::video(ONE_VIDEO),
                Told::sound(ONE_SOUND, 0x10, STEREO, "jpn"),
                Told::sound(0x0123, 0x12, STEREO, "kor"),
                Told::sound(0x0124, 0x11, MONO, "eng"),
            ],
        ),
    );
    engine.push(&later.bytes);
    assert_eq!(engine.sounds().len(), 3);
    assert_eq!(
        engine.chosen().1,
        Some(0x0124),
        "the sound that was chosen was not followed to where it moved"
    );
    let played = engine.sounds();
    let played = played
        .iter()
        .find(|stream| Some(stream.id) == engine.chosen().1)
        .expect("the chosen sound");
    assert_eq!(
        played.component.as_ref().map(|one| one.language.as_str()),
        Some("eng"),
        "a different language is being played than the one chosen"
    );
}

#[test]
fn a_chosen_sound_that_leaves_falls_back() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &two_sounds(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    engine.next_sound().expect("a second sound");

    let mut later = Tape::default();
    later.send(
        ONE_MAP,
        &described(
            ONE,
            1,
            &[
                Told::video(ONE_VIDEO),
                Told::sound(ONE_SOUND, 0x10, STEREO, "jpn"),
            ],
        ),
    );
    engine.push(&later.bytes);
    assert_eq!(engine.sounds().len(), 1);
    assert_eq!(engine.chosen().1, Some(ONE_SOUND));
}

#[test]
fn dual_mono_is_told_apart_from_stereo() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)])).send(
        ONE_MAP,
        &described(
            ONE,
            0,
            &[
                Told::video(ONE_VIDEO),
                Told::sound(ONE_SOUND, 0x10, DUAL, "jpn"),
                Told::sound(0x0122, 0x11, STEREO, "jpn"),
            ],
        ),
    );
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let sounds = engine.sounds();
    let dual = sounds[0].component.as_ref().expect("a described sound");
    let stereo = sounds[1].component.as_ref().expect("a described sound");
    assert!(dual.dual_mono());
    assert!(!stereo.dual_mono());
    assert_eq!(dual.channels(), stereo.channels(), "both are two channels");

    assert!(engine.sound_is_dual_mono());
    assert_eq!(engine.dual_mono(), DualMono::Main);
    engine.set_dual_mono(DualMono::Sub);
    assert_eq!(engine.dual_mono(), DualMono::Sub);

    engine.play_sound(0x0122);
    assert!(
        !engine.sound_is_dual_mono(),
        "the stereo stream is being played as two programmes"
    );
}

#[test]
fn a_sound_of_another_programme_is_not_taken() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert!(!engine.play_sound(TWO_SOUND));
    assert_eq!(engine.chosen(), (Some(ONE_VIDEO), Some(ONE_SOUND)));
}

const ONE_BACKUP: u16 = 0x0110;

fn rain_fade(version: u8) -> Vec<u8> {
    described(
        ONE,
        version,
        &[
            Told::half(ONE_BACKUP, 0x01, false, ONE_VIDEO),
            Told::half(ONE_VIDEO, 0x00, true, ONE_BACKUP),
            Told::sound(ONE_SOUND, 0x10, STEREO, "jpn"),
        ],
    )
}

#[test]
fn a_rain_fade_pair_is_read() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &rain_fade(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let pictures = engine.pictures();
    assert_eq!(pictures.len(), 2);
    let backup = pictures[0].hierarchy.as_ref().expect("a paired picture");
    let main = pictures[1].hierarchy.as_ref().expect("a paired picture");
    assert_eq!(
        backup.level, 1,
        "the stand-in was read as the picture to show"
    );
    assert_eq!(main.level, 0);
    assert_eq!(backup.reference, Some(ONE_VIDEO));
    assert_eq!(main.reference, Some(ONE_BACKUP));
    assert!(engine.is_stand_in(ONE_BACKUP));
    assert!(!engine.is_stand_in(ONE_VIDEO));
}

/// A stream marked low quality with no counterpart (`fe ff ff`, as BS sends) is not a stand-in.
#[test]
fn a_stream_marked_low_quality_with_no_counterpart_is_not_a_stand_in() {
    let lone = Told {
        stream_type: AAC,
        pid: ONE_SOUND,
        tag: Some(0x10),
        sound: None,
        pair: Some((false, 0x1fff)),
    };
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &described(ONE, 0, &[Told::video(ONE_VIDEO), lone]));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let sounds = engine.sounds();
    assert_eq!(sounds.len(), 1);
    assert_eq!(
        sounds[0].hierarchy.as_ref().map(|pair| pair.level),
        Some(1),
        "what the transmission said was not read"
    );
    assert_eq!(
        sounds[0].hierarchy.as_ref().and_then(|pair| pair.reference),
        None,
        "the null identifier was taken for a counterpart"
    );
    assert!(
        !engine.is_stand_in(ONE_SOUND),
        "the only sound there is was called a fallback"
    );
    assert_eq!(engine.chosen().1, Some(ONE_SOUND));
}

#[test]
fn the_picture_played_is_not_merely_the_first() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &rain_fade(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert_eq!(engine.chosen().0, Some(ONE_VIDEO));
    assert!(!engine.picture_is_stand_in());
    assert_eq!(engine.picture_at(), Some(1));
}

#[test]
fn a_programme_with_no_pair_takes_the_first_picture() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert_eq!(engine.chosen().0, Some(ONE_VIDEO));
    assert_eq!(engine.pictures().len(), 1);
    assert!(!engine.picture_is_stand_in());
}

#[test]
fn a_viewer_can_ask_for_the_stand_in() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &rain_fade(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let next = engine.next_picture().expect("a second picture");
    assert_eq!(next.id, ONE_BACKUP);
    assert_eq!(engine.chosen().0, Some(ONE_BACKUP));
    assert!(engine.picture_is_stand_in());
    assert_eq!(engine.chosen().1, Some(ONE_SOUND));

    assert_eq!(engine.next_picture().map(|one| one.id), Some(ONE_VIDEO));
}

#[test]
fn the_stand_in_is_not_climbed_out_of_by_a_map_update() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &rain_fade(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    engine.next_picture().expect("a second picture");
    assert_eq!(engine.chosen().0, Some(ONE_BACKUP));

    let mut later = Tape::default();
    later.send(
        ONE_MAP,
        &described(
            ONE,
            1,
            &[
                Told::half(0x0118, 0x01, false, 0x0119),
                Told::half(0x0119, 0x00, true, 0x0118),
                Told::sound(ONE_SOUND, 0x10, STEREO, "jpn"),
                Told::sound(0x0122, 0x11, MONO, "eng"),
            ],
        ),
    );
    engine.push(&later.bytes);
    assert_eq!(
        engine.chosen().0,
        Some(0x0118),
        "the viewer was put back on the picture the rain is taking away"
    );
    assert!(engine.picture_is_stand_in());
}

#[test]
fn a_picture_of_another_programme_is_not_taken() {
    let mut tape = Tape::default();
    tape.both();
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert!(!engine.play_picture(TWO_VIDEO));
    assert_eq!(engine.chosen(), (Some(ONE_VIDEO), Some(ONE_SOUND)));
}

fn bare_sounds(version: u8) -> Vec<u8> {
    described(
        ONE,
        version,
        &[
            Told::video(ONE_VIDEO),
            Told {
                stream_type: AAC,
                pid: ONE_SOUND,
                tag: Some(0x10),
                sound: None,
                pair: None,
            },
            Told {
                stream_type: AAC,
                pid: 0x0122,
                tag: Some(0x11),
                sound: None,
                pair: None,
            },
        ],
    )
}

/// What the sounds are comes out of the event table, not the map.
#[test]
fn what_the_sounds_are_comes_out_of_the_event_table() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &bare_sounds(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert_eq!(engine.sounds().len(), 2);
    assert!(
        engine.sounds().iter().all(|one| one.component.is_none()),
        "the map described the sounds, which a transport stream does not do"
    );

    let mut event = Tape::default();
    event.send(
        EIT_PID,
        &eit(
            ONE,
            0,
            &[
                Described::new(0x10, DUAL, "jpn", true),
                Described::new(0x11, STEREO, "eng", false),
            ],
        ),
    );
    engine.push(&event.bytes);

    let sounds = engine.sounds();
    let first = sounds[0].component.as_ref().expect("a described sound");
    assert_eq!(first.language, "jpn");
    assert!(first.dual_mono());
    assert!(first.main);
    let second = sounds[1].component.as_ref().expect("a described sound");
    assert_eq!(second.language, "eng");
    assert!(!second.dual_mono());
    assert!(!second.main);
    assert!(engine.sound_is_dual_mono());
}

#[test]
fn the_event_table_may_arrive_before_the_map() {
    let mut tape = Tape::default();
    tape.send(
        EIT_PID,
        &eit(
            ONE,
            0,
            &[
                Described::new(0x10, DUAL, "jpn", true),
                Described::new(0x11, STEREO, "eng", false),
            ],
        ),
    )
    .send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
    .send(ONE_MAP, &bare_sounds(0));
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let sounds = engine.sounds();
    assert_eq!(sounds.len(), 2);
    assert_eq!(
        sounds[0]
            .component
            .as_ref()
            .map(|one| one.language.as_str()),
        Some("jpn"),
        "the event table was thrown away because the map had not arrived"
    );
    assert!(engine.sound_is_dual_mono());
}

#[test]
fn the_following_event_does_not_describe_the_present_one() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &bare_sounds(0))
        .send(
            EIT_PID,
            &eit_at(
                ONE,
                0,
                1,
                &[
                    Described::new(0x10, DUAL, "kor", true),
                    Described::new(0x11, MONO, "kor", false),
                ],
            ),
        );
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert!(
        engine.sounds().iter().all(|one| one.component.is_none()),
        "the event that has not started yet described the one on the air"
    );

    let mut now = Tape::default();
    now.send(
        EIT_PID,
        &eit(ONE, 0, &[Described::new(0x10, STEREO, "jpn", true)]),
    );
    engine.push(&now.bytes);
    assert_eq!(
        engine.sounds()[0]
            .component
            .as_ref()
            .map(|one| one.language.as_str()),
        Some("jpn")
    );
}

#[test]
fn the_event_table_of_another_service_is_not_used() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &bare_sounds(0))
        .send(
            EIT_PID,
            &eit(TWO, 0, &[Described::new(0x10, DUAL, "kor", true)]),
        );
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert!(
        engine.sounds().iter().all(|one| one.component.is_none()),
        "another service's event table described this one's sounds"
    );
}

#[test]
fn the_sound_played_is_not_merely_the_first() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)])).send(
        ONE_MAP,
        &described(
            ONE,
            0,
            &[
                Told::video(ONE_VIDEO),
                Told {
                    stream_type: AAC,
                    pid: 0x0122,
                    tag: Some(0x11),
                    sound: None,
                    pair: Some((false, ONE_SOUND)),
                },
                Told {
                    stream_type: AAC,
                    pid: ONE_SOUND,
                    tag: Some(0x10),
                    sound: None,
                    pair: Some((true, 0x0122)),
                },
            ],
        ),
    );
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    assert_eq!(engine.sounds().len(), 2);
    assert_eq!(
        engine.chosen().1,
        Some(ONE_SOUND),
        "the sound that stands in during rain fade is being played in the clear"
    );
    assert!(engine.is_stand_in(0x0122));
}

const CALLED_JAPANESE: &[u8] = &[0x46, 0x7c, 0x4b, 0x5c, 0x38, 0x6c];
const CALLED_STADIUM: &[u8] = &[0x35, 0x65, 0x3e, 0x6c, 0x32, 0x3b, 0x40, 0x3c];

/// What a sound is called is read out of ARIB's eight-unit code (two `jpn` stereo sounds
/// differ only by their text).
#[test]
fn what_a_sound_is_called_is_decoded() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &bare_sounds(0))
        .send(
            EIT_PID,
            &eit(
                ONE,
                0,
                &[
                    Described::new(0x10, STEREO, "jpn", true).called(CALLED_JAPANESE),
                    Described::new(0x11, STEREO, "jpn", false).called(CALLED_STADIUM),
                ],
            ),
        );
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let sounds = engine.sounds();
    let first = sounds[0].component.as_ref().expect("a described sound");
    let second = sounds[1].component.as_ref().expect("a described sound");
    assert_eq!(first.name.as_deref(), Some("日本語"));
    assert_eq!(second.name.as_deref(), Some("球場音声"));
    assert_eq!(
        first.language, second.language,
        "the two are told apart by the code, so the text proves nothing"
    );
}

const CALLED_PICTURE_TWO: &[u8] = &[0x31, 0x47, 0x41, 0x7c, 0x23, 0x32];

#[test]
fn what_a_picture_is_called_is_decoded() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(
            ONE_MAP,
            &described(
                ONE,
                0,
                &[
                    Told::tagged_video(ONE_VIDEO, 0x00),
                    Told::tagged_video(ONE_VIDEO + 1, 0x01),
                    Told::sound(ONE_SOUND, 0x10, STEREO, "jpn"),
                ],
            ),
        )
        .send(
            EIT_PID,
            &eit_full(
                ONE,
                0,
                0,
                &[Described::new(0x10, STEREO, "jpn", true)],
                &[(0x00, &[]), (0x01, CALLED_PICTURE_TWO)],
            ),
        );
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);

    let pictures = engine.pictures();
    assert_eq!(pictures.len(), 2);
    assert_eq!(pictures[0].name, None, "the default name is not a name");
    assert_eq!(pictures[1].name.as_deref(), Some("映像２"));
    assert!(
        pictures
            .iter()
            .all(|picture| !engine.is_stand_in(picture.id)),
        "two named pictures are two pictures, not a pair"
    );
}

#[test]
fn a_sound_with_no_text_is_not_given_an_empty_name() {
    let mut tape = Tape::default();
    tape.send(PAT_PID, &pat(0, &[(ONE, ONE_MAP)]))
        .send(ONE_MAP, &bare_sounds(0))
        .send(
            EIT_PID,
            &eit(ONE, 0, &[Described::new(0x10, STEREO, "jpn", true)]),
        );
    let mut engine = engine();
    engine.want(ONE);
    engine.push(&tape.bytes);
    let sounds = engine.sounds();
    let first = sounds[0].component.as_ref().expect("a described sound");
    assert_eq!(first.name, None);
    assert_eq!(first.language, "jpn");
}
