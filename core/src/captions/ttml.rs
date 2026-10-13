// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MMT/TLV の字幕・文字スーパー（ARIB-TTML）。文書と同じ MPU のグリフ・画像・音をまとめて持つ。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use isdb::ttml::Document;

use super::{Choice, SUPERIMPOSE_FOR};
use crate::engine::CaptionStream;
use crate::sound::sounding::Say;

pub type Picture = crate::image::Rgba;

#[derive(Debug, Clone)]
pub struct Shown {
    pub document: Arc<Document>,
    /// 文書の時刻（ミリ秒）で、`since` の時点。
    pub origin_ms: u64,
    pub since: Instant,
    pub gaiji: Arc<BTreeMap<char, Arc<isdb::svg::Outline>>>,
    pub pictures: Arc<BTreeMap<String, Arc<Picture>>>,
}

impl Shown {
    pub fn now_ms(&self) -> u64 {
        self.origin_ms + self.since.elapsed().as_millis() as u64
    }

    pub fn animates(&self) -> bool {
        let at = self.now_ms();
        let doc = &self.document;
        let pending = |begin: Option<u64>, end: Option<u64>| {
            begin.is_some_and(|b| b > at) || end.is_some_and(|e| e > at)
        };
        doc.paragraphs.iter().any(|p| {
            pending(p.begin, p.end)
                || p.runs.iter().any(|r| pending(r.begin, r.end))
                || p.animation
                    .as_ref()
                    .is_some_and(|a| a.running(at.saturating_sub(p.begin.unwrap_or(doc.begin()))))
        }) || doc.images.iter().any(|i| pending(i.begin, i.end))
    }

    /// `end` の無いものが一つでもあれば偽。
    pub fn over(&self) -> bool {
        self.document.end().is_some_and(|end| self.now_ms() >= end)
    }
}

/// 字幕の追加情報が無いアセットの表示モード（受信時も記録再生時も選択表示）。
const DMF_SELECTABLE: u8 = 0b1010;

/// 文書に続くサブサンプル（外字のフォント・画像・音）を待つ上限。欠けてもこの後は手元の物で出す。
const RESOURCES_WITHIN: std::time::Duration = std::time::Duration::from_secs(2);

/// 同じ MPU の続くサブサンプルがそろうまで出さずに持つ文書（STD-B60 表 9-1 の `last_subsample_number`）。
/// 先に出すと、外字のフォントが届くまで外字が代わりの字で描かれ、画像も抜ける。
#[derive(Debug)]
struct Waiting {
    document: Document,
    pid: u16,
    sequence: u8,
    last: u8,
    arrived: std::collections::BTreeSet<u8>,
    since: Instant,
    sound: Option<Sender<Say>>,
}

impl Waiting {
    fn complete(&self) -> bool {
        (1..=self.last).all(|subsample| self.arrived.contains(&subsample))
    }
}

#[derive(Debug, Default)]
pub struct State {
    caption: Option<Arc<Shown>>,
    superimpose: Option<Arc<Shown>>,
    /// そのアセットが MPT から消えたら、出しているものも消す。
    caption_pid: Option<u16>,
    superimpose_pid: Option<u16>,
    /// MPU が変わったら捨てる（TR-B39 8.9）。
    subsamples: BTreeMap<u8, (u8, Vec<u8>)>,
    subsamples_of: Option<(u16, u8)>,
    drawn: BTreeMap<u8, Option<Arc<Picture>>>,
    /// 外字のアウトライン。グリフは文書の後に届くので、保持している文書へ足す。
    fonts: BTreeMap<char, Arc<isdb::svg::Outline>>,
    /// 続くサブサンプルを待っている文書（字幕、文字スーパー）。
    waiting: [Option<Waiting>; 2],
    dirty: bool,
}

impl State {
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// 出すかどうか（字幕、文字スーパー）は [`Choice::actual`] で決まる。
    pub fn shown(
        &self,
        caption: bool,
        superimpose: bool,
    ) -> (Option<Arc<Shown>>, Option<Arc<Shown>>) {
        (
            self.caption.clone().filter(|_| caption),
            self.superimpose.clone().filter(|_| superimpose),
        )
    }

    /// 字幕は `end` で（無ければ次の文書まで）、文字スーパーは `end` か三分で消す。
    pub fn expire(&mut self) {
        for slot in 0..2 {
            if self.waiting[slot]
                .as_ref()
                .is_some_and(|waiting| waiting.since.elapsed() > RESOURCES_WITHIN)
            {
                self.release(slot);
            }
        }
        if self.caption.as_ref().is_some_and(|shown| shown.over()) {
            self.caption = None;
            self.dirty = true;
        }
        if self
            .superimpose
            .as_ref()
            .is_some_and(|shown| shown.over() || shown.since.elapsed() > SUPERIMPOSE_FOR)
        {
            self.superimpose = None;
            self.dirty = true;
        }
    }

    pub fn assets_left(&mut self, streams: &[CaptionStream]) {
        let carried =
            |pid: Option<u16>| pid.is_some_and(|pid| streams.iter().any(|c| c.pid == pid));
        if self.caption_pid.is_some() && !carried(self.caption_pid) {
            self.caption_pid = None;
            self.caption = None;
            self.dirty = true;
        }
        if self.superimpose_pid.is_some() && !carried(self.superimpose_pid) {
            self.superimpose_pid = None;
            self.superimpose = None;
            self.dirty = true;
        }
        for waiting in &mut self.waiting {
            if waiting.as_ref().is_some_and(|w| !carried(Some(w.pid))) {
                *waiting = None;
            }
        }
    }

    pub fn clear_superimpose(&mut self) {
        self.superimpose = None;
        self.waiting[1] = None;
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.caption = None;
        self.superimpose = None;
        self.caption_pid = None;
        self.superimpose_pid = None;
        self.subsamples.clear();
        self.subsamples_of = None;
        self.drawn.clear();
        self.waiting = [None, None];
        self.dirty = true;
    }

    pub fn reset(&mut self) {
        self.clear();
        self.fonts.clear();
    }

    fn refresh(&mut self) {
        let fonts = Arc::new(self.fonts.clone());
        let pictures = Arc::new(self.pictures());
        for shown in [&mut self.caption, &mut self.superimpose]
            .into_iter()
            .flatten()
        {
            let mut next = (**shown).clone();
            next.gaiji = fonts.clone();
            next.pictures = pictures.clone();
            *shown = Arc::new(next);
            self.dirty = true;
        }
    }

    fn pictures(&mut self) -> BTreeMap<String, Arc<Picture>> {
        let Self {
            subsamples, drawn, ..
        } = self;
        subsamples
            .iter()
            .filter_map(|(index, (kind, bytes))| {
                let picture = drawn.entry(*index).or_insert_with(|| {
                    match *kind {
                        isdb::mhcaption::DATA_PNG => decode_png(bytes),
                        isdb::mhcaption::DATA_SVG => {
                            std::str::from_utf8(bytes).ok().and_then(draw_svg)
                        }
                        _ => None,
                    }
                    .map(Arc::new)
                });
                Some((format!("subt://{index}"), Arc::clone(picture.as_ref()?)))
            })
            .collect()
    }

    fn aiff(&self, index: Option<u8>) -> Option<&[u8]> {
        fn aiff((kind, bytes): &(u8, Vec<u8>)) -> Option<&[u8]> {
            (*kind == isdb::mhcaption::DATA_AIFF).then_some(bytes.as_slice())
        }
        match index {
            Some(index) => self.subsamples.get(&index).and_then(aiff),
            None => self.subsamples.values().find_map(aiff),
        }
    }

    pub fn take(
        &mut self,
        stream: &CaptionStream,
        data: &[u8],
        streams: &[CaptionStream],
        captions: &mut Choice,
        superimposes: &mut Choice,
        sound: Option<&Sender<Say>>,
    ) {
        let Some(mfu) = isdb::mhcaption::mfu(data) else {
            return;
        };
        // 次の MPU が始まったら、待っている文書は手元の物で出す（前の MPU のサブサンプルを捨てる前に）。
        for slot in 0..2 {
            if self.waiting[slot]
                .as_ref()
                .is_some_and(|w| w.pid == stream.pid && w.sequence != mfu.sequence)
            {
                self.release(slot);
            }
        }
        let of = (stream.pid, mfu.sequence);
        if self.subsamples_of != Some(of) {
            self.subsamples_of = Some(of);
            self.subsamples.clear();
            self.drawn.clear();
        }
        match mfu.data_type {
            isdb::mhcaption::DATA_TTML => {}
            isdb::mhcaption::DATA_FONT_SVG => {
                let Ok(svg) = std::str::from_utf8(mfu.data) else {
                    return;
                };
                let glyphs = isdb::svg::font_glyphs(svg);
                if glyphs.is_empty() {
                    return;
                }
                self.fonts
                    .extend(glyphs.into_iter().map(|(c, o)| (c, Arc::new(o))));
                self.refresh();
                self.arrived(stream.pid, mfu.sequence, mfu.subsample);
                return;
            }
            kind => {
                self.subsamples
                    .insert(mfu.subsample, (kind, mfu.data.to_vec()));
                self.drawn.remove(&mfu.subsample);
                if kind == isdb::mhcaption::DATA_PNG {
                    self.refresh();
                }
                self.arrived(stream.pid, mfu.sequence, mfu.subsample);
                return;
            }
        }
        let info = stream.info.as_ref();
        // MMT は言語ごとに別のアセット。
        let kind = info.map_or(0, |i| i.kind);
        let same_kind: Vec<&CaptionStream> = streams
            .iter()
            .filter(|c| c.info.as_ref().map_or(0, |i| i.kind) == kind)
            .collect();
        // 字幕（0）と文字スーパー（1）は言語を別々に選ぶ。
        let choice = if kind == 1 { superimposes } else { captions };
        {
            let languages: Vec<[u8; 3]> = same_kind
                .iter()
                .map(|c| {
                    let mut iso = [b' '; 3];
                    if let Some(i) = &c.info {
                        for (slot, byte) in iso.iter_mut().zip(i.language.bytes()) {
                            *slot = byte;
                        }
                    }
                    iso
                })
                .collect();
            let modes: Vec<u8> = same_kind
                .iter()
                .map(|c| c.info.as_ref().map_or(DMF_SELECTABLE, |i| i.dmf))
                .collect();
            if choice.set_languages(languages, modes) {
                // 出す言語が変わったので、前の言語の文書は消す。
                if kind == 1 {
                    self.superimpose = None;
                } else {
                    self.caption = None;
                }
                self.waiting[usize::from(kind == 1)] = None;
                self.dirty = true;
            }
        }
        let number = same_kind
            .iter()
            .position(|c| c.pid == stream.pid)
            .map_or(1, |i| i as u8 + 1);
        if number != choice.actual().max(1) {
            return;
        }
        let Some(xml) = isdb::mhcaption::ttml_text(&mfu, info.is_some_and(|i| i.compression == 1))
        else {
            return;
        };
        let screen = match info.map(|i| i.resolution) {
            Some(0) => (1920, 1080),
            Some(2) => (7680, 4320),
            _ => (3840, 2160),
        };
        let Some(mut document) = isdb::ttml::parse(&xml, screen) else {
            return;
        };
        // TMD=1111 なら書いてある時刻は見ない（TR-B39 8.5.9.2）。
        if info.is_none_or(|i| i.tmd == 0b1111) {
            document.ignore_times();
        }
        let slot = usize::from(info.is_some_and(|i| i.kind == 1));
        let waiting = Waiting {
            document,
            pid: stream.pid,
            sequence: mfu.sequence,
            last: mfu.last_subsample,
            arrived: std::collections::BTreeSet::new(),
            since: Instant::now(),
            sound: sound.cloned(),
        };
        if waiting.complete() {
            self.waiting[slot] = None;
            self.show(slot, waiting);
        } else {
            self.waiting[slot] = Some(waiting);
        }
    }

    /// 待っている文書のサブサンプルが届いた。そろったら出す。
    fn arrived(&mut self, pid: u16, sequence: u8, subsample: u8) {
        for slot in 0..2 {
            let Some(waiting) = self.waiting[slot].as_mut() else {
                continue;
            };
            if waiting.pid != pid || waiting.sequence != sequence {
                continue;
            }
            waiting.arrived.insert(subsample);
            if waiting.complete() {
                self.release(slot);
            }
        }
    }

    fn release(&mut self, slot: usize) {
        if let Some(waiting) = self.waiting[slot].take() {
            self.show(slot, waiting);
        }
    }

    /// 文書を出す。時刻は文書を受け取った時点から数える（サブサンプルを待った分だけ先へ進んでいる）。
    fn show(&mut self, slot: usize, waiting: Waiting) {
        let Waiting {
            document,
            pid,
            since,
            sound,
            ..
        } = waiting;
        // `romsound://N` は内蔵音、`subt://N` は同じ MPU の AIFF（STD-B62 第一編第三部 3.4、3.6）。
        if let Some(sound) = sound.as_ref() {
            for audio in &document.audio {
                if let Some(number) = audio
                    .src
                    .strip_prefix("romsound://")
                    .and_then(|n| n.parse::<u32>().ok())
                {
                    let _ = sound.send(Say::Chime(number));
                } else {
                    let index = audio
                        .src
                        .strip_prefix("subt://")
                        .and_then(|n| n.parse::<u8>().ok());
                    if let Some(aiff) = self.aiff(index) {
                        let _ = sound.send(Say::Aiff(aiff.to_vec()));
                    }
                }
            }
        }
        let mut pictures = self.pictures();
        for (id, bytes) in &document.embedded {
            if let Some(picture) = decode_png(bytes) {
                pictures.insert(format!("#{id}"), Arc::new(picture));
            }
        }
        // 文書の時刻は最初に出すものの `begin`。
        let shown = (!document.is_clear()).then(|| {
            Arc::new(Shown {
                origin_ms: document.begin(),
                document: Arc::new(document),
                since,
                gaiji: Arc::new(self.fonts.clone()),
                pictures: Arc::new(pictures),
            })
        });
        if slot == 1 {
            self.superimpose = shown;
            self.superimpose_pid = Some(pid);
        } else {
            self.caption = shown;
            self.caption_pid = Some(pid);
        }
        self.dirty = true;
    }
}

fn draw_svg(svg: &str) -> Option<Picture> {
    let drawing = isdb::svg::Drawing::parse(svg)?;
    let (w, h) = drawing.size();
    // 負・0・無限大はレンダリングしない（`as usize` が巨大になる）。
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let scale = (2048.0 / w.max(h)).min(1.0);
    let width = ((w * scale).round() as usize).max(1);
    let height = ((h * scale).round() as usize).max(1);
    Some(Picture::new(
        width as u32,
        height as u32,
        drawing.rasterise(width, height),
    ))
}

/// 8K の字幕プレーンまで。PNG のヘッダーにある大きさで確保するので先に確かめる。
const PICTURE_MOST: [u32; 2] = [7680, 4320];

fn decode_png(bytes: &[u8]) -> Option<Picture> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    if info.width > PICTURE_MOST[0] || info.height > PICTURE_MOST[1] {
        return None;
    }
    let mut buffer = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let rgba = crate::image::rgba_of_png(info.color_type, &buffer[..info.buffer_size()])?;
    Some(Picture::new(info.width, info.height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r##"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:tts="http://www.w3.org/ns/ttml#styling" xmlns:arib-tt="http://www.arib.or.jp/ns/arib-tt" xml:lang="ja"><head><styling><style xml:id="s" tts:fontSize="144px 144px" tts:lineHeight="240px" tts:color="#ffffffff"/></styling><layout><region xml:id="r" tts:origin="1592px 1796px" tts:extent="640px 240px"/></layout></head><body><div><p region="r"><span style="s">外字&#xe054;</span></p></div></body></tt>"##;
    const FONT: &str = r#"<svg><defs><font><font-face units-per-em="1024" ascent="880" descent="144"/><glyph unicode="&#xe054;" horiz-adv-x="1024" d="M0,-144 L1024,-144 L1024,880 L0,880 Z"/></font></defs></svg>"#;

    /// 字幕の MFU（STD-B60 表 9-1、サイズは 2 バイト、一覧なし）。
    fn mfu(sequence: u8, subsample: u8, last: u8, kind: u8, body: &str) -> Vec<u8> {
        let mut bytes = vec![0x30, sequence, subsample, last, kind << 4];
        bytes.extend_from_slice(&(body.len() as u16).to_be_bytes());
        bytes.extend_from_slice(body.as_bytes());
        bytes
    }

    fn stream() -> CaptionStream {
        CaptionStream {
            pid: 0x100,
            tag: 0,
            info: None,
            one_seg: false,
        }
    }

    fn take(state: &mut State, bytes: &[u8]) {
        let stream = stream();
        let (mut captions, mut superimposes) = (Choice::default(), Choice::default());
        state.take(
            &stream,
            bytes,
            std::slice::from_ref(&stream),
            &mut captions,
            &mut superimposes,
            None,
        );
    }

    #[test]
    fn a_document_waits_for_its_font_and_shows_with_it() {
        let mut state = State::default();
        take(&mut state, &mfu(1, 0, 1, isdb::mhcaption::DATA_TTML, DOC));
        assert!(
            state.shown(true, true).0.is_none(),
            "フォントが届くまで出さない"
        );
        take(
            &mut state,
            &mfu(1, 1, 1, isdb::mhcaption::DATA_FONT_SVG, FONT),
        );
        let shown = state.shown(true, true).0.expect("そろったら出す");
        assert!(
            shown.gaiji.contains_key(&'\u{e054}'),
            "最初から外字の輪郭を持つ"
        );
    }

    #[test]
    fn a_document_with_nothing_to_follow_shows_at_once() {
        let mut state = State::default();
        take(&mut state, &mfu(1, 0, 0, isdb::mhcaption::DATA_TTML, DOC));
        assert!(state.shown(true, true).0.is_some());
    }

    #[test]
    fn the_next_unit_shows_a_document_whose_font_was_lost() {
        let mut state = State::default();
        take(&mut state, &mfu(1, 0, 1, isdb::mhcaption::DATA_TTML, DOC));
        take(
            &mut state,
            &mfu(2, 1, 1, isdb::mhcaption::DATA_FONT_SVG, FONT),
        );
        assert!(
            state.shown(true, true).0.is_some(),
            "次の MPU が始まったら手元の物で出す"
        );
    }
}
