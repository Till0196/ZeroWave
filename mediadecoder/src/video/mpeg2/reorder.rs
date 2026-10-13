// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-2 のピクチャーを表示の順へ並べ直す。参照ピクチャーは次の参照を復号したときに、
//! B ピクチャーは届いたときに出す。どのデコーダーもこの規則を同じに守らないと順序がずれるので、
//! 規則はここに一つだけ置き、各デコーダーは [`Frames`] で自分のピクチャーを表す。

use crate::es::Structure;

/// Where a picture goes when the reordering lets go of it.
pub trait Frames {
    /// A surface number, a timestamp, an index into a pool.
    type Picture: Copy;

    /// The next picture to show; the reordering gives up its hold.
    fn show(&mut self, picture: Self::Picture);

    /// The same, for a picture still kept as a reference. A path that counts
    /// holds must take one of its own.
    fn show_and_keep(&mut self, picture: Self::Picture);

    fn discard(&mut self, picture: Self::Picture);
}

#[derive(Debug, Clone, Copy)]
pub struct Reorder<P> {
    older: Option<P>,
    newer: Option<P>,
    current: Option<P>,
    pending: Option<Structure>,
    bidirectional: bool,
    unshowable: bool,
}

impl<P: Copy> Default for Reorder<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Copy> Reorder<P> {
    pub const fn new() -> Self {
        Self {
            older: None,
            newer: None,
            current: None,
            pending: None,
            bidirectional: false,
            unshowable: false,
        }
    }

    pub fn older(&self) -> Option<P> {
        self.older
    }

    pub fn newer(&self) -> Option<P> {
        self.newer
    }

    pub fn current(&self) -> Option<P> {
        self.current
    }

    pub fn pending(&self) -> Option<Structure> {
        self.pending
    }

    pub fn second(&self, structure: Structure) -> bool {
        match (structure, self.pending) {
            (Structure::Frame, _) => false,
            (parity, Some(first)) => first != parity,
            _ => false,
        }
    }

    /// Makes room for a new picture, giving back one begun but never finished. Separate from
    /// `begin` so a pooled path gets the room back first, or refused decodes empty the pool.
    pub fn open(&mut self, frames: &mut impl Frames<Picture = P>) {
        if self.pending.is_some() {
            self.close(frames);
        }
        if let Some(stale) = self.current.take() {
            frames.discard(stale);
        }
    }

    /// Starts a picture after [`Reorder::open`]. `unshowable`: a B picture with
    /// nothing to predict forward from.
    pub fn begin(
        &mut self,
        picture: P,
        structure: Structure,
        bidirectional: bool,
        unshowable: bool,
    ) {
        self.current = Some(picture);
        self.bidirectional = bidirectional;
        self.unshowable = unshowable;
        self.pending = match structure {
            Structure::Frame => None,
            parity => Some(parity),
        };
    }

    /// Closes the current picture: I/P becomes a reference, B is shown at once.
    pub fn close(&mut self, frames: &mut impl Frames<Picture = P>) {
        self.pending = None;
        let Some(current) = self.current.take() else {
            return;
        };
        let unshowable = std::mem::take(&mut self.unshowable);
        if self.bidirectional {
            // Leading B pictures of an open GOP predict from a picture never had: dropped.
            if unshowable {
                frames.discard(current);
            } else {
                frames.show(current);
            }
            return;
        }
        if let Some(newer) = self.newer {
            frames.show_and_keep(newer);
        }
        if let Some(older) = self.older {
            frames.discard(older);
        }
        self.older = self.newer;
        self.newer = Some(current);
    }

    /// End of stream: what is held comes out. A lone first field is dropped.
    pub fn flush(&mut self, frames: &mut impl Frames<Picture = P>) {
        self.pending = None;
        if let Some(current) = self.current.take() {
            frames.discard(current);
        }
        if let Some(newer) = self.newer.take() {
            frames.show(newer);
        }
        if let Some(older) = self.older.take() {
            frames.discard(older);
        }
    }

    /// Gives up everything held (a change of channel); nothing comes out.
    pub fn reset(&mut self, frames: &mut impl Frames<Picture = P>) {
        for picture in self
            .older
            .take()
            .into_iter()
            .chain(self.newer.take())
            .chain(self.current.take())
        {
            frames.discard(picture);
        }
        self.pending = None;
        self.bidirectional = false;
        self.unshowable = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq, Eq)]
    struct Log(Vec<String>);

    impl Frames for Log {
        type Picture = u32;

        fn show(&mut self, picture: u32) {
            self.0.push(format!("show {picture}"));
        }

        fn show_and_keep(&mut self, picture: u32) {
            self.0.push(format!("keep {picture}"));
        }

        fn discard(&mut self, picture: u32) {
            self.0.push(format!("drop {picture}"));
        }
    }

    fn picture(order: &mut Reorder<u32>, log: &mut Log, at: u32, bidirectional: bool) {
        let unshowable = bidirectional && order.older().is_none();
        order.open(log);
        order.begin(at, Structure::Frame, bidirectional, unshowable);
        order.close(log);
    }

    #[test]
    fn a_reference_comes_out_when_the_next_one_is_decoded() {
        let mut order = Reorder::new();
        let mut log = Log::default();
        picture(&mut order, &mut log, 0, false);
        assert_eq!(log, Log(Vec::new()), "the first reference came out at once");
        picture(&mut order, &mut log, 3, false);
        picture(&mut order, &mut log, 1, true);
        picture(&mut order, &mut log, 2, true);
        picture(&mut order, &mut log, 6, false);
        assert_eq!(
            log.0,
            ["keep 0", "show 1", "show 2", "keep 3", "drop 0"],
            "the pictures came out in the order they are shown"
        );
    }

    #[test]
    fn the_leading_pictures_of_an_open_group_are_dropped() {
        let mut order = Reorder::new();
        let mut log = Log::default();
        picture(&mut order, &mut log, 1, true);
        picture(&mut order, &mut log, 2, true);
        assert_eq!(log.0, ["drop 1", "drop 2"]);
        picture(&mut order, &mut log, 0, false);
        picture(&mut order, &mut log, 3, false);
        picture(&mut order, &mut log, 4, true);
        assert_eq!(log.0[2..], ["keep 0", "show 4"]);
    }

    #[test]
    fn a_field_pair_is_one_picture_and_half_a_pair_is_none() {
        let mut order = Reorder::new();
        let mut log = Log::default();
        order.open(&mut log);
        order.begin(0, Structure::Top, false, false);
        assert!(order.second(Structure::Bottom));
        assert!(!order.second(Structure::Top));
        assert!(!order.second(Structure::Frame));
        order.close(&mut log);
        assert_eq!(order.newer(), Some(0));
        order.open(&mut log);
        order.begin(1, Structure::Top, false, false);
        order.flush(&mut log);
        assert_eq!(log.0, ["drop 1", "show 0"]);
    }
}
