// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Turns the coded access units the engine puts out into pictures. Anything else passes through.

use std::collections::{BTreeMap, VecDeque};

use mediadecoder::es::{Codec, Framing};

use crate::decode::weave::{Weaver, frame_of_readback};
use crate::decode::{Decode, Decoded, DeinterlaceBy, Error, Prefer};
use crate::engine::{AccessUnit, Out};

const MOST_HELD_TIMES: usize = 64;

/// 一枚も出力しないまま何回エラーになったら、プラットフォームのデコーダーを諦めるか。
const REFUSALS: u32 = 8;

struct Opened {
    stream: u16,
    codec: Codec,
    framing: Framing,
    decoder: Box<dyn Decode>,
    /// エラーになったら次に試す候補の番号（[`crate::decode::open_from`]）。`None` は内蔵のデコーダー。
    next: Option<usize>,
    made: bool,
    refused: u32,
    /// Not a queue: decoders reorder.
    times: BTreeMap<u64, i64>,
    /// 最後に時刻を付けたフレーム。デインターレースすると一枚から二枚出る。二枚目は一フィールド後。
    last: Option<(u64, i64)>,
    /// 1 ピクチャーが 1 フィールドの HEVC で、送ったピクチャーがトップフィールドか。空でなければ
    /// 二フィールドずつ一枚のフレームにする（[`crate::decode::weave`]）。
    fields: BTreeMap<u64, bool>,
    weaver: Weaver,
    /// 途中でデコーダーを切り替えたら先に渡す（でないと次のランダムアクセスポイントまでデコードできない）。
    parameters: Vec<u8>,
    /// 読み戻したフィールドを一枚にできなかったことを記録に出したか（一度だけ出す）。
    unreadable: bool,
}

fn parameter_sets(codec: Codec, framing: Framing, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in mediadecoder::es::Nals::new(data, framing) {
        let parameter = match codec {
            Codec::H264 => matches!(nal.header & 0x1f, 7 | 8),
            Codec::Hevc => matches!((nal.header >> 1) & 0x3f, 32..=34),
            Codec::Mpeg2 => false,
        };
        if parameter && let Some(bytes) = data.get(nal.at..nal.body.end) {
            out.extend_from_slice(bytes);
        }
    }
    out
}

pub struct Decoding {
    prefer: Prefer,
    by: DeinterlaceBy,
    open: Option<Opened>,
    generation: u64,
}

impl std::fmt::Debug for Decoding {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Decoding")
            .field("prefer", &self.prefer)
            .field("open", &self.open.as_ref().map(|open| open.stream))
            .finish()
    }
}

impl Decoding {
    pub fn new(prefer: Prefer) -> Self {
        Self::with_deinterlace_by(prefer, DeinterlaceBy::default())
    }

    pub fn with_deinterlace_by(prefer: Prefer, by: DeinterlaceBy) -> Self {
        Self {
            prefer,
            by,
            open: None,
            generation: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn forget(&mut self) {
        if self.open.take().is_some() {
            self.generation += 1;
        }
    }

    pub fn deinterlaces(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.decoder.deinterlaces())
    }

    pub fn decoder(&self) -> Option<(String, bool)> {
        self.open
            .as_ref()
            .map(|open| (open.decoder.name(), open.decoder.hardware()))
    }

    pub fn push(&mut self, out: Out, into: &mut VecDeque<Out>) {
        let Out::Access(unit) = out else {
            into.push_back(out);
            return;
        };
        let prefer = self.prefer;
        let by = self.by;
        let open = self.open_for(&unit);
        if unit.first {
            open.decoder.reset();
            open.times.clear();
            open.last = None;
            open.fields.clear();
            open.weaver.clear();
        }
        if unit.format.field_pictures
            && !open.decoder.marks_fields()
            && let Some(field) = unit.picture.lone_field
        {
            open.fields
                .insert(unit.tag, field == mediadecoder::es::Structure::Top);
            while open.fields.len() > MOST_HELD_TIMES {
                open.fields.pop_first();
            }
        }
        if let Some(at) = unit.at {
            open.times.insert(unit.tag, at);
        }
        let carried = parameter_sets(open.codec, open.framing, &unit.data);
        if !carried.is_empty() {
            open.parameters = carried;
        }
        let mut outcome = open
            .decoder
            .decode(unit.tag, &unit.picture, unit.format, &unit.data);
        // 一枚も出力しないままエラーが続いたら候補の次のデコーダーにフォールバックする。エラーの理由は区別しない
        // （環境ごとに違う）。一枚でも出ていれば、壊れたアクセスユニットがあっただけなので切り替えない。
        if outcome.is_err() && !open.made {
            open.refused += 1;
        }
        let hopeless = !open.made && open.refused >= REFUSALS;
        let mut fell = false;
        if (outcome.is_err() || hopeless)
            && let Some(next) = open.next
        {
            let unsupported = matches!(&outcome, Err(Error::Unsupported(_)));
            if unsupported || hopeless {
                let was = open.decoder.name();
                match &outcome {
                    Err(error) => {
                        tracing::warn!(
                            "{was} でデコードできない（{error:?}）。次のデコーダーに切り替える"
                        );
                    }
                    Ok(()) => tracing::info!("{was} から次のデコーダーに切り替える"),
                }
                (open.decoder, open.next) =
                    crate::decode::open_from(open.codec, open.framing, prefer, by, next);
                open.made = false;
                open.refused = 0;
                fell = true;
                open.times.clear();
                open.weaver.clear();
                if let Some(at) = unit.at {
                    open.times.insert(unit.tag, at);
                }
                let data = if parameter_sets(open.codec, open.framing, &unit.data).is_empty()
                    && !open.parameters.is_empty()
                {
                    let mut joined = open.parameters.clone();
                    joined.extend_from_slice(&unit.data);
                    std::borrow::Cow::Owned(joined)
                } else {
                    std::borrow::Cow::Borrowed(&unit.data[..])
                };
                outcome = open
                    .decoder
                    .decode(unit.tag, &unit.picture, unit.format, &data);
            }
        }
        if let Err(error) = outcome {
            into.push_back(Out::Trouble {
                stream: unit.stream,
                what: format!("{error:?}"),
            });
        }
        Self::drain(open, into);
        if fell {
            self.generation += 1;
        }
    }

    pub fn flush(&mut self, into: &mut VecDeque<Out>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.decoder.flush();
        Self::drain(open, into);
    }

    fn open_for(&mut self, unit: &AccessUnit) -> &mut Opened {
        let stale = self.open.as_ref().is_some_and(|open| {
            open.stream != unit.stream || open.codec != unit.codec || open.framing != unit.framing
        });
        if stale {
            self.open = None;
        }
        if self.open.is_none() {
            self.generation += 1;
        }
        let (prefer, by) = (self.prefer, self.by);
        self.open.get_or_insert_with(|| {
            let (decoder, next) = crate::decode::open_from(unit.codec, unit.framing, prefer, by, 0);
            Opened {
                stream: unit.stream,
                codec: unit.codec,
                framing: unit.framing,
                decoder,
                next,
                made: false,
                refused: 0,
                times: BTreeMap::new(),
                last: None,
                fields: BTreeMap::new(),
                weaver: Weaver::default(),
                parameters: Vec::new(),
                unreadable: false,
            }
        })
    }

    fn drain(open: &mut Opened, into: &mut VecDeque<Out>) {
        if !open.fields.is_empty() {
            Self::drain_fields(open, into);
            return;
        }
        while let Some(picture) = open.decoder.take() {
            open.made = true;
            let about = picture.about();
            let tag = about.tag;
            let at = match open.times.remove(&tag) {
                Some(at) => Some(at),
                // デインターレースした二枚目：一枚目の時刻に一フィールド足す。
                None => open.last.filter(|(last, _)| *last == tag).map(|(_, at)| {
                    let (num, den) = about.format.rate;
                    let field = if num == 0 {
                        0
                    } else {
                        crate::engine::clock::HZ * i64::from(den) / (2 * i64::from(num))
                    };
                    at + field
                }),
            };
            open.last = at.map(|at| (tag, at));
            while open.times.len() > MOST_HELD_TIMES {
                let Some(oldest) = open.times.keys().next().copied() else {
                    break;
                };
                open.times.remove(&oldest);
            }
            match at {
                Some(at) => into.push_back(Out::Picture { at, picture }),
                None => into.push_back(Out::Trouble {
                    stream: open.stream,
                    what: String::from("表示時刻の無いフレームが届いた"),
                }),
            }
        }
    }

    /// フィールドをメインメモリのフレームにしてから二つずつ一枚にする。読み戻せない GPU のフレームと、
    /// どちらのフィールドか分からないフレームは、そのまま出す。
    fn drain_fields(open: &mut Opened, into: &mut VecDeque<Out>) {
        #[cfg(unix)]
        if crate::decode::takes::takes_lone_fields() {
            Self::drain_lone_fields(open, into);
            return;
        }
        loop {
            let frame = match open.decoder.read_back() {
                Some(read) => match frame_of_readback(&read) {
                    Ok(frame) => Some(frame),
                    Err(why) => {
                        let tag = read.about.tag;
                        open.fields.remove(&tag);
                        open.times.remove(&tag);
                        if !open.unreadable {
                            open.unreadable = true;
                            tracing::warn!("読み戻したフィールドを組めないので捨てる: {why}");
                        }
                        None
                    }
                },
                None => match open.decoder.take() {
                    Some(Decoded::Planes(frame)) => Some(frame),
                    Some(picture) => {
                        open.made = true;
                        let tag = picture.about().tag;
                        open.fields.remove(&tag);
                        if let Some(at) = open.times.remove(&tag) {
                            into.push_back(Out::Picture { at, picture });
                        }
                        continue;
                    }
                    None => return,
                },
            };
            open.made = true;
            let Some(frame) = frame else {
                continue;
            };
            let tag = frame.tag;
            let at = open.times.remove(&tag);
            let Some(top) = open.fields.remove(&tag) else {
                if let Some(at) = at {
                    into.push_back(Out::Picture {
                        at,
                        picture: Decoded::Planes(frame),
                    });
                }
                continue;
            };
            if let Some((woven, Some(at))) = open.weaver.push(frame, top, at) {
                into.push_back(Out::Picture {
                    at,
                    picture: Decoded::Planes(woven),
                });
            }
        }
    }

    /// DMA-BUF のフィールドは一枚にせず、どちらのフィールドかを `structure` に入れてそのまま渡す（レンダラーが半行ずらして
    /// 縦に伸ばし、一フィールドずつ描く）。メインメモリのフレームはここで一枚にする。
    #[cfg(unix)]
    fn drain_lone_fields(open: &mut Opened, into: &mut VecDeque<Out>) {
        use mediadecoder::es::Structure;
        while let Some(picture) = open.decoder.take() {
            open.made = true;
            let tag = picture.about().tag;
            let at = open.times.remove(&tag);
            let top = open.fields.remove(&tag);
            match (picture, top) {
                (Decoded::Planes(frame), Some(top)) => {
                    if let Some((woven, Some(at))) = open.weaver.push(frame, top, at) {
                        into.push_back(Out::Picture {
                            at,
                            picture: Decoded::Planes(woven),
                        });
                    }
                }
                (mut picture, top) => {
                    if let (Decoded::Surface(surface), Some(top)) = (&mut picture, top) {
                        surface.about.structure = if top {
                            Structure::Top
                        } else {
                            Structure::Bottom
                        };
                    }
                    if let Some(at) = at {
                        into.push_back(Out::Picture { at, picture });
                    }
                }
            }
        }
    }
}
