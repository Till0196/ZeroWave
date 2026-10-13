// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The stateful flavour: the decoder parses and reorders itself. It says nothing
//! about field order, so the demuxer's reading is kept and matched back by timestamp.

use std::collections::HashMap;

use mediadecoder::es::{Framing, Picture};

use crate::decode::v4l2::Held;

/// A safety valve, not a queue depth: decoders run 40+ units behind.
const DEPTH: usize = 256;

#[derive(Debug)]
pub struct Stateful {
    framing: Framing,
    said: HashMap<u64, Held>,
    /// A whole millisecond apart: `struct timeval` carries only microseconds.
    next: u64,
    draining: bool,
}

impl Stateful {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            said: HashMap::new(),
            next: 1_000_000,
            draining: false,
        }
    }

    /// Only Annex B; a length-prefixed stream falls back.
    pub fn takes(&self) -> bool {
        matches!(self.framing, Framing::StartCodes)
    }

    pub fn begin(&mut self, tag: u64, picture: &Picture) -> u64 {
        let ts = self.next;
        self.next += 1_000_000;
        self.said.insert(
            ts,
            Held {
                slot: ts,
                tag,
                bidirectional: false,
                unshowable: false,
                structure: picture.structure,
                top_field_first: picture.top_field_first,
                repeat_first_field: picture.repeat_first_field,
                progressive: picture.progressive,
                order: 0,
            },
        );
        ts
    }

    /// Two field units make one frame with one timestamp, so a miss is not a fault.
    pub fn finish(&mut self, ts: u64) -> Option<Held> {
        self.said.remove(&ts)
    }

    /// Not every access unit becomes a picture, so the oldest beyond [`DEPTH`] go.
    pub fn forget_stale(&mut self) {
        while self.said.len() > DEPTH {
            let Some(oldest) = self.said.keys().min().copied() else {
                return;
            };
            self.said.remove(&oldest);
        }
    }

    pub fn draining(&self) -> bool {
        self.draining
    }

    pub fn drain(&mut self) {
        self.draining = true;
    }

    pub fn reset(&mut self) {
        self.said.clear();
        self.draining = false;
    }

    pub fn outstanding(&self) -> usize {
        self.said.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::es::{Coding, Structure};

    fn field(top: bool) -> Picture {
        Picture {
            coding: Coding::I,
            structure: if top {
                Structure::Top
            } else {
                Structure::Bottom
            },
            top_field_first: true,
            repeat_first_field: false,
            progressive: false,
            ..Picture::default()
        }
    }

    #[test]
    fn what_the_demuxer_said_comes_back_with_the_picture() {
        let mut stateful = Stateful::new(Framing::StartCodes);
        let first = stateful.begin(7, &field(true));
        let second = stateful.begin(8, &field(false));
        assert_ne!(first, second);
        let back = stateful.finish(second).expect("the second field");
        assert_eq!(back.tag, 8);
        assert_eq!(back.structure, Structure::Bottom);
        assert_eq!(stateful.outstanding(), 1);
        assert!(stateful.finish(second).is_none());
    }

    #[test]
    fn every_timestamp_survives_the_kernels_clock() {
        use crate::decode::v4l2::uapi::Timeval;
        let mut stateful = Stateful::new(Framing::StartCodes);
        for tag in 0..64 {
            let ts = stateful.begin(tag, &field(true));
            assert_eq!(Timeval::from_ns(ts).to_ns(), ts);
        }
    }
}
