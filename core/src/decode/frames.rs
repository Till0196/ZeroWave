// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! DPB から見たサーフェスの管理。四つのバックエンドで共通。

use std::collections::VecDeque;

use mediadecoder::es::Picture;
use mediadecoder::video::h264::dpb as h264;
use mediadecoder::video::hevc::dpb as hevc;
use mediadecoder::video::mpeg2::reorder;

/// サーフェスを貸し出す側。各バックエンドの `Pool` が実装する（数え方は [`crate::decode::slots`]）。
pub trait Surfaces {
    type Id: Copy;

    fn take(&mut self) -> Option<Self::Id>;

    fn hold(&mut self, id: Self::Id);

    fn release(&mut self, id: Self::Id);

    /// HEVC で受け取らなかった参照の代わり（規格 8.3.3）に使うサーフェス。表示はしない。`None` なら作らない
    /// （まだデコードしていないサーフェスを参照に使えないバックエンド）。
    fn stand_in(&mut self) -> Option<Self::Id> {
        None
    }
}

pub trait Named<Id> {
    fn name(&self) -> Id;
}

/// 代わりのピクチャー（[`Surfaces::stand_in`]）の、表示の順番を待つ側の記録。
pub trait StandIn<Id> {
    fn stand_in(slot: Id, poc: i32) -> Self;
}

/// 表示の順番を待つ一枚。`Id` はスロット番号（VA-API・NVDEC・D3D12）か出力のタイムスタンプ（V4L2）。
#[derive(Debug, Clone, Copy)]
pub struct Held<Id> {
    pub slot: Id,
    pub tag: u64,
    pub bidirectional: bool,
    /// 表示できないか（オープン GOP の先頭の B）。`Mpeg2::unshowable` と合わせないと順序がずれる。
    pub unshowable: bool,
    pub structure: mediadecoder::es::Structure,
    pub top_field_first: bool,
    pub repeat_first_field: bool,
    pub progressive: bool,
    pub order: i32,
}

impl<Id: Copy> Named<Id> for Held<Id> {
    fn name(&self) -> Id {
        self.slot
    }
}

impl<Id> Held<Id> {
    pub fn about(&self, format: mediadecoder::es::Format) -> crate::decode::About {
        crate::decode::About {
            tag: self.tag,
            format,
            structure: self.structure,
            top_field_first: self.top_field_first,
            repeat_first_field: self.repeat_first_field,
            progressive: self.progressive,
        }
    }
}

impl<Id: Default> Default for Held<Id> {
    fn default() -> Self {
        Self {
            slot: Id::default(),
            tag: 0,
            bidirectional: false,
            unshowable: false,
            structure: mediadecoder::es::Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: false,
            order: 0,
        }
    }
}

impl<Id> StandIn<Id> for Held<Id> {
    fn stand_in(slot: Id, poc: i32) -> Self {
        Self {
            slot,
            tag: 0,
            bidirectional: false,
            unshowable: true,
            structure: mediadecoder::es::Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: true,
            order: poc,
        }
    }
}

impl<Id> h264::Ordered for Held<Id> {
    fn set_order(&mut self, order: i32) {
        self.order = order;
    }
}

#[derive(Debug)]
pub struct Frames<'a, P: Surfaces, H> {
    pub pool: &'a mut P,
    pub ready: &'a mut VecDeque<H>,
}

impl<P, H> h264::Slots for Frames<'_, P, H>
where
    P: Surfaces,
    H: Copy + Named<P::Id>,
{
    type Id = P::Id;
    type Held = H;

    fn take(&mut self, _stores: &[h264::Store<Self::Id, H>]) -> Option<Self::Id> {
        self.pool.take()
    }

    fn release(&mut self, id: Self::Id) {
        self.pool.release(id);
    }

    fn emit(&mut self, store: &h264::Store<Self::Id, H>) {
        self.pool.hold(store.slot);
        self.ready.push_back(store.held);
    }
}

impl<P, H> hevc::Frames for Frames<'_, P, H>
where
    P: Surfaces,
    P::Id: PartialEq + std::fmt::Debug,
    H: Copy + Named<P::Id> + StandIn<P::Id>,
{
    type Id = P::Id;
    type Held = H;

    fn take(&mut self, _live: &[Self::Id]) -> Option<Self::Id> {
        self.pool.take()
    }

    fn release(&mut self, id: Self::Id) {
        self.pool.release(id);
    }

    fn output(&mut self, id: Self::Id, held: &H) {
        self.pool.hold(id);
        self.ready.push_back(*held);
    }

    fn stand_in(&mut self, _live: &[Self::Id], poc: i32) -> Option<(Self::Id, H)> {
        let slot = self.pool.stand_in()?;
        tracing::debug!("HEVC: 受け取っていない参照（POC {poc}）の代わりのピクチャーを作る");
        Some((slot, H::stand_in(slot, poc)))
    }
}

/// MPEG-2 の並べ替えから見たとき。ピクチャーが渡されるので、ID はピクチャーから取る。
impl<P, H> reorder::Frames for Frames<'_, P, H>
where
    P: Surfaces,
    H: Copy + Named<P::Id>,
{
    type Picture = H;

    fn show(&mut self, held: H) {
        self.ready.push_back(held);
    }

    fn show_and_keep(&mut self, held: H) {
        self.pool.hold(held.name());
        self.ready.push_back(held);
    }

    fn discard(&mut self, held: H) {
        self.pool.release(held.name());
    }
}

/// H.264 の clause 8.2 と、その前後。四つのバックエンドで共通。
#[derive(Debug)]
pub struct H264Order<Id> {
    pub dpb: h264::Dpb<Id, Held<Id>>,
    pub current: Option<h264::Current>,
    pub ready: VecDeque<Held<Id>>,
}

impl<Id: Copy + PartialEq + std::fmt::Debug> Default for H264Order<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id: Copy + PartialEq + std::fmt::Debug> H264Order<Id> {
    pub fn new() -> Self {
        let mut dpb = h264::Dpb::new();
        dpb.capacity = 16;
        Self {
            dpb,
            current: None,
            ready: VecDeque::new(),
        }
    }

    pub fn take(&mut self) -> Option<Held<Id>> {
        self.ready.pop_front()
    }

    pub fn flush<P: Surfaces<Id = Id>>(&mut self, pool: &mut P) {
        self.finish(pool);
        {
            let Self { dpb, ready, .. } = self;
            let mut slots = Frames { pool, ready };
            while dpb.bump(&mut slots) {}
        }
        for store in std::mem::take(&mut self.dpb.stores) {
            pool.release(store.slot);
        }
        self.dpb.pending = None;
    }

    pub fn reset<P: Surfaces<Id = Id>>(&mut self, pool: &mut P) {
        self.finish(pool);
        for store in std::mem::take(&mut self.dpb.stores) {
            pool.release(store.slot);
        }
        for held in self.ready.drain(..) {
            pool.release(held.slot);
        }
        self.current = None;
        self.dpb.pending = None;
        self.dpb.forget();
    }

    pub fn start<P: Surfaces<Id = Id>>(
        &mut self,
        pool: &mut P,
        tag: u64,
        picture: &Picture,
        header: &mediadecoder::video::h264::params::Header,
        sps: &mediadecoder::video::h264::params::Sps,
    ) -> Result<(), crate::decode::Error> {
        let stale = self.current.take();
        let Self { dpb, ready, .. } = self;
        let current = dpb.start(
            &mut Frames { pool, ready },
            stale,
            sps,
            header,
            |slot, order| Held {
                slot,
                tag,
                bidirectional: false,
                unshowable: false,
                structure: picture.structure,
                // フィールドピクチャーのマークは 0。対はデコード順（トップが先）で来る。
                top_field_first: match picture.structure {
                    mediadecoder::es::Structure::Frame => picture.top_field_first,
                    mediadecoder::es::Structure::Top => true,
                    mediadecoder::es::Structure::Bottom => false,
                },
                repeat_first_field: picture.repeat_first_field,
                progressive: picture.progressive,
                order,
            },
        )?;
        self.current = Some(current);
        Ok(())
    }

    /// デコード中のピクチャーを閉じて表示できるようにする。無くても害は無い。
    pub fn finish<P: Surfaces<Id = Id>>(&mut self, pool: &mut P) {
        let Some(current) = self.current.take() else {
            return;
        };
        let Self { dpb, ready, .. } = self;
        dpb.finish(&mut Frames { pool, ready }, current);
    }
}

/// HEVC の clause 8.3 と、その前後。デコード中のピクチャー（`Current`）は各バックエンドが持つ。
#[derive(Debug)]
pub struct HevcOrder<Id> {
    pub dpb: hevc::Dpb<Id, Held<Id>>,
    pub ready: VecDeque<Held<Id>>,
    /// 開始点のリーディングピクチャーか（スライスは飛ばす）。
    pub skipping: bool,
}

impl<Id: Copy + PartialEq + std::fmt::Debug> Default for HevcOrder<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id: Copy + PartialEq + std::fmt::Debug> HevcOrder<Id> {
    pub fn new() -> Self {
        Self {
            dpb: hevc::Dpb::new(0, 16),
            ready: VecDeque::new(),
            skipping: false,
        }
    }

    pub fn take(&mut self) -> Option<Held<Id>> {
        self.ready.pop_front()
    }

    /// 保持しているピクチャーを全部出す（end of stream）。デコード中のピクチャーを先に入れること。
    pub fn flush<P: Surfaces<Id = Id>>(&mut self, pool: &mut P) {
        let Self { dpb, ready, .. } = self;
        dpb.bump_all(&mut Frames { pool, ready });
        dpb.clear(&mut Frames { pool, ready });
    }

    /// 全部捨てる（再選局）。`open` はデコード中のサーフェス。
    pub fn reset<P: Surfaces<Id = Id>>(&mut self, pool: &mut P, open: Option<Id>) {
        if let Some(slot) = open {
            pool.release(slot);
        }
        {
            let Self { dpb, ready, .. } = self;
            dpb.reset(&mut Frames { pool, ready });
        }
        for held in self.ready.drain(..) {
            pool.release(held.slot);
        }
        self.skipping = false;
    }

    /// ピクチャーを開始してサーフェスを探す（clause 8.3）。飛ばすリーディングピクチャーは `None`。
    /// 先に [`mediadecoder::video::hevc::dpb::Dpb::limits`] を設定する。
    pub fn begin<P: Surfaces<Id = Id>>(
        &mut self,
        pool: &mut P,
        sps: &mediadecoder::video::hevc::params::Sps,
        header: &mediadecoder::video::hevc::params::Header,
        kind: u8,
        temporal: u8,
    ) -> Result<Option<(Id, i32)>, crate::decode::Error> {
        self.skipping = false;
        let Self { dpb, ready, .. } = self;
        let Some(poc) = dpb.begin(&mut Frames { pool, ready }, sps, header, kind, temporal) else {
            self.skipping = true;
            return Ok(None);
        };
        // 参照が欠けているピクチャー（代わりのピクチャーも作れなかったもの）はデコードしない。ずれた参照を渡すと
        // デバイスごと落ちる GPU がある。
        if dpb.sets.iter().flatten().any(|entry| entry.id.is_none()) {
            self.skipping = true;
            return Ok(None);
        }
        let Some(slot) = dpb.free(&mut Frames { pool, ready }) else {
            return Err(crate::decode::Error::Damaged(
                "a picture buffer that cannot be made room in",
            ));
        };
        Ok(Some((slot, poc)))
    }

    pub fn held(slot: Id, tag: u64, picture: &Picture, poc: i32) -> Held<Id> {
        Held {
            slot,
            tag,
            bidirectional: false,
            unshowable: false,
            structure: picture.structure,
            top_field_first: picture.top_field_first,
            repeat_first_field: picture.repeat_first_field,
            progressive: picture.progressive,
            order: poc,
        }
    }

    pub fn push<P: Surfaces<Id = Id>>(
        &mut self,
        pool: &mut P,
        slot: Id,
        poc: i32,
        output: bool,
        held: Held<Id>,
    ) {
        let Self { dpb, ready, .. } = self;
        dpb.push(&mut Frames { pool, ready }, slot, poc, output, held);
    }
}

/// MPEG-2 の並べ替え（[`mediadecoder::video::mpeg2::reorder`]）と、その前後。
#[derive(Debug)]
pub struct Mpeg2Order<Id> {
    pub reorder: reorder::Reorder<Held<Id>>,
    pub ready: VecDeque<Held<Id>>,
}

impl<Id: Copy> Default for Mpeg2Order<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id: Copy> Mpeg2Order<Id> {
    pub fn new() -> Self {
        Self {
            reorder: reorder::Reorder::new(),
            ready: VecDeque::new(),
        }
    }

    pub fn take(&mut self) -> Option<Held<Id>> {
        self.ready.pop_front()
    }

    pub fn flush<P: Surfaces<Id = Id>>(&mut self, pool: &mut P) {
        let Self { reorder, ready } = self;
        reorder.flush(&mut Frames { pool, ready });
    }

    pub fn reset<P: Surfaces<Id = Id>>(&mut self, pool: &mut P) {
        {
            let Self { reorder, ready } = self;
            reorder.reset(&mut Frames { pool, ready });
        }
        for held in self.ready.drain(..) {
            pool.release(held.slot);
        }
    }

    /// ピクチャーを開始するか、二つ目のフィールドを一つ目に足す。二つ目なら true（同じサーフェス）。
    pub fn begin<P: Surfaces<Id = Id>>(
        &mut self,
        pool: &mut P,
        tag: u64,
        picture: &Picture,
    ) -> Result<bool, crate::decode::Error> {
        let second = self.reorder.second(picture.structure);
        if second {
            return Ok(true);
        }
        {
            let Self { reorder, ready } = self;
            reorder.open(&mut Frames { pool, ready });
        }
        let slot = pool
            .take()
            .ok_or(crate::decode::Error::Unavailable(String::from(
                "サーフェスが足りません",
            )))?;
        let bidirectional = picture.coding == mediadecoder::es::Coding::B;
        // 前方の参照が無い B（オープン GOP の先頭）。出力しない。
        let unshowable = bidirectional && self.reorder.older().is_none();
        let held = Held {
            slot,
            tag,
            bidirectional,
            unshowable,
            structure: picture.structure,
            // フィールドピクチャーのマークは 0。対はデコード順で来る。
            top_field_first: match picture.structure {
                mediadecoder::es::Structure::Frame => picture.top_field_first,
                mediadecoder::es::Structure::Top => true,
                mediadecoder::es::Structure::Bottom => false,
            },
            repeat_first_field: picture.repeat_first_field,
            progressive: picture.progressive,
            order: 0,
        };
        self.reorder
            .begin(held, picture.structure, bidirectional, unshowable);
        Ok(false)
    }

    pub fn current(&self) -> Option<Held<Id>> {
        self.reorder.current()
    }

    pub fn references(
        &self,
        coding: mediadecoder::es::Coding,
    ) -> (Option<Held<Id>>, Option<Held<Id>>) {
        match coding {
            mediadecoder::es::Coding::B => (self.reorder.older(), self.reorder.newer()),
            _ => (self.reorder.newer(), None),
        }
    }

    pub fn pending(&self) -> Option<mediadecoder::es::Structure> {
        self.reorder.pending()
    }

    pub fn close<P: Surfaces<Id = Id>>(&mut self, pool: &mut P) {
        let Self { reorder, ready } = self;
        reorder.close(&mut Frames { pool, ready });
    }
}
