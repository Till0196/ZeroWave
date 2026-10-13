// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

#![doc = include_str!("process.md")]

pub const OPEN: u32 = 1;
pub const SHARE: u32 = 2;
pub const RUN: u32 = 3;

pub const INPUT: u32 = 0;
pub const MIDDLE: u32 = 1;
pub const READY: u32 = 2;
pub const DONE: u32 = 3;

pub const JOBS: u32 = 2;

/// 連続するジョブは前と今のうち一つを共有する。
pub fn inputs(jobs: u32) -> u32 {
    jobs + 1
}

pub fn port_id(what: u32, index: u32) -> u32 {
    what * 256 + index
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Way {
    D3d12 = 1,
    Vulkan = 2,
    Metal = 3,
    Shm = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    /// GPU のフェンス（ready・done）。RUN の応答はキューに積んだらすぐ。
    Fence,
    /// RUN の前にフレームは書き終わっていて、応答が書き終えた合図。
    Notify,
}

impl Way {
    pub const ALL: [Self; 4] = [Self::D3d12, Self::Vulkan, Self::Metal, Self::Shm];

    pub fn name(self) -> &'static str {
        match self {
            Self::D3d12 => "d3d12",
            Self::Vulkan => "vulkan",
            Self::Metal => "metal",
            Self::Shm => "shm",
        }
    }

    pub fn of_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|way| way.name() == name)
    }

    pub fn of(value: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|way| *way as u32 == value)
    }

    pub fn sync(self) -> Sync {
        match self {
            Self::D3d12 | Self::Vulkan => Sync::Fence,
            Self::Metal | Self::Shm => Sync::Notify,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    F32 = 1,
    F16 = 2,
}

impl Format {
    pub const ALL: [Self; 2] = [Self::F32, Self::F16];

    pub fn of(value: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|format| *format as u32 == value)
    }

    pub fn bytes(self) -> u32 {
        match self {
            Self::F32 => 4,
            Self::F16 => 2,
        }
    }
}

/// `d3d12`・`vulkan`・`shm` の行の幅：幅 × 値のバイト数を 4 の倍数へ。
pub fn stride(width: u32, format: Format) -> u32 {
    (width * format.bytes()).next_multiple_of(4)
}

pub fn picture_bytes(height: u32, stride: u32) -> u64 {
    3 * u64::from(height) * u64::from(stride)
}

/// `way` は未知の値も来うるのでそのまま持つ。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Offer {
    pub way: u32,
    pub id: Vec<u8>,
}

impl Offer {
    pub fn new(way: Way, id: Vec<u8>) -> Self {
        Self {
            way: way as u32,
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub width: u32,
    pub height: u32,
    pub jobs: u32,
    pub offers: Vec<Offer>,
    pub formats: Vec<u32>,
}

impl Open {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put32(&mut out, self.width);
        put32(&mut out, self.height);
        put32(&mut out, self.jobs);
        put32(&mut out, self.offers.len() as u32);
        for offer in &self.offers {
            put32(&mut out, offer.way);
            put32(&mut out, offer.id.len() as u32);
            out.extend_from_slice(&offer.id);
        }
        put32(&mut out, self.formats.len() as u32);
        for format in &self.formats {
            put32(&mut out, *format);
        }
        out
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut at = Cursor::new(body, "OPEN");
        let width = at.u32()?;
        let height = at.u32()?;
        let jobs = at.u32()?;
        let offers = (0..at.u32()?)
            .map(|_| {
                let way = at.u32()?;
                let len = at.u32()? as usize;
                Ok(Offer {
                    way,
                    id: at.take(len)?.to_vec(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let formats = (0..at.u32()?)
            .map(|_| at.u32())
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            width,
            height,
            jobs,
            offers,
            formats,
        })
    }

    /// 本体の優先順に、使える転送方式と形式の最初のもの（プロセス側の例）。
    pub fn choose(
        &self,
        can: impl Fn(Way, &[u8]) -> bool,
        formats: &[Format],
    ) -> Option<(Way, &Offer, Format)> {
        let (way, offer) = self.offers.iter().find_map(|offer| {
            let way = Way::of(offer.way)?;
            can(way, &offer.id).then_some((way, offer))
        })?;
        let format = formats
            .iter()
            .copied()
            .find(|format| self.formats.contains(&(*format as u32)))?;
        Some((way, offer, format))
    }
}

/// レンダリングに使う GPU の `registryID` と、プロセスが受けるポートの mach のサービス名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalId {
    pub registry_id: u64,
    pub service: String,
}

impl MetalId {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.registry_id.to_le_bytes().to_vec();
        out.extend_from_slice(self.service.as_bytes());
        out
    }

    pub fn decode(id: &[u8]) -> Result<Self, String> {
        let mut at = Cursor::new(id, "metal 方式のデータ");
        let registry_id = at.u64()?;
        let service = std::str::from_utf8(at.bytes)
            .map_err(|_| String::from("metal 方式のサービス名が UTF-8 でない"))?;
        Ok(Self {
            registry_id,
            service: service.to_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened {
    pub way: u32,
    pub format: u32,
    pub jobs: u32,
}

impl Opened {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put32(&mut out, self.way);
        put32(&mut out, self.format);
        put32(&mut out, self.jobs);
        out
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut at = Cursor::new(body, "OPEN の応答");
        Ok(Self {
            way: at.u32()?,
            format: at.u32()?,
            jobs: at.u32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Share {
    pub what: u32,
    pub index: u32,
    pub stride: u32,
    pub size: u64,
    pub handle: u64,
}

impl Share {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put32(&mut out, self.what);
        put32(&mut out, self.index);
        put32(&mut out, self.stride);
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.handle.to_le_bytes());
        out
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut at = Cursor::new(body, "SHARE");
        Ok(Self {
            what: at.u32()?,
            index: at.u32()?,
            stride: at.u32()?,
            size: at.u64()?,
            handle: at.u64()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    pub value: u64,
    pub phase: f32,
    pub previous: u32,
    pub current: u32,
    pub middle: u32,
}

impl Run {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.value.to_le_bytes().to_vec();
        out.extend_from_slice(&self.phase.to_le_bytes());
        put32(&mut out, self.previous);
        put32(&mut out, self.current);
        put32(&mut out, self.middle);
        out
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut at = Cursor::new(body, "RUN");
        Ok(Self {
            value: at.u64()?,
            phase: f32::from_bits(at.u32()?),
            previous: at.u32()?,
            current: at.u32()?,
            middle: at.u32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ran {
    pub value: u64,
    pub gpu_ms: f32,
}

impl Ran {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.value.to_le_bytes().to_vec();
        out.extend_from_slice(&self.gpu_ms.to_le_bytes());
        out
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut at = Cursor::new(body, "RUN の応答");
        Ok(Self {
            value: at.u64()?,
            gpu_ms: f32::from_bits(at.u32()?),
        })
    }

    pub fn ms(&self) -> Option<f32> {
        (self.gpu_ms >= 0.0).then_some(self.gpu_ms)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Open(Open),
    Share(Share),
    Run(Run),
}

impl Request {
    pub fn decode(op: u32, body: &[u8]) -> Result<Self, String> {
        match op {
            OPEN => Open::decode(body).map(Self::Open),
            SHARE => Share::decode(body).map(Self::Share),
            RUN => Run::decode(body).map(Self::Run),
            _ => Err(format!("不明なメッセージ（{op}）")),
        }
    }
}

pub fn request(op: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    put32(&mut out, op);
    put32(&mut out, body.len() as u32);
    out.extend_from_slice(body);
    out
}

pub fn reply(result: Result<&[u8], &str>) -> Vec<u8> {
    let (status, body) = match result {
        Ok(body) => (0i32, body),
        Err(why) => (1, why.as_bytes()),
    };
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&status.to_le_bytes());
    put32(&mut out, body.len() as u32);
    out.extend_from_slice(body);
    out
}

/// 8 バイト：一つ目の値（op か状態）とペイロードの長さ。
pub fn head(bytes: [u8; 8]) -> (u32, usize) {
    let first = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    (first, len as usize)
}

/// 標準エラーの `@status <0〜1 か -> <テキスト>` の行。
pub fn status_line(line: &str) -> Option<(Option<f32>, String)> {
    let rest = line.trim_end().strip_prefix("@status ")?;
    let (progress, said) = rest.split_once(' ').unwrap_or((rest, ""));
    let progress = match progress {
        "-" => None,
        number => Some(number.parse::<f32>().ok()?.clamp(0.0, 1.0)),
    };
    Some((progress, said.trim().to_owned()))
}

pub fn status(progress: Option<f32>, said: &str) -> String {
    match progress {
        Some(progress) => format!("@status {:.3} {said}", progress.clamp(0.0, 1.0)),
        None => format!("@status - {said}"),
    }
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    what: &'static str,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, what }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        if self.bytes.len() < len {
            return Err(format!("{} が短い", self.what));
        }
        let (head, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        let mut word = [0u8; 8];
        word.copy_from_slice(b);
        Ok(u64::from_le_bytes(word))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Open {
        Open {
            width: 1440,
            height: 1080,
            jobs: JOBS,
            offers: vec![
                Offer::new(Way::D3d12, vec![1, 2, 3, 4, 5, 6, 7, 8]),
                Offer {
                    way: 99,
                    id: vec![9],
                },
                Offer::new(Way::Shm, Vec::new()),
            ],
            formats: vec![Format::F32 as u32, Format::F16 as u32],
        }
    }

    #[test]
    fn open_goes_round() {
        let open = open();
        let bytes = open.encode();
        assert_eq!(&bytes[0..12], &[0xa0, 5, 0, 0, 0x38, 4, 0, 0, 2, 0, 0, 0]);
        assert_eq!(Open::decode(&bytes), Ok(open));
    }

    #[test]
    fn a_short_open_is_refused() {
        let bytes = open().encode();
        for cut in [0, 7, 11, 16, 24, bytes.len() - 1] {
            assert!(Open::decode(&bytes[..cut]).is_err(), "{cut} バイト");
        }
    }

    #[test]
    fn the_first_usable_way_is_chosen() {
        let open = open();
        let (way, _, format) = open
            .choose(|way, id| way == Way::Shm || id == [0; 8], &[Format::F16])
            .expect("選べる");
        assert_eq!((way, format), (Way::Shm, Format::F16));
        let (way, offer, format) = open
            .choose(|_, _| true, &[Format::F32, Format::F16])
            .expect("選べる");
        assert_eq!((way, offer.id.len(), format), (Way::D3d12, 8, Format::F32));
        assert!(
            open.choose(|way, _| way == Way::Metal, &[Format::F32])
                .is_none()
        );
        let only_f32 = Open {
            formats: vec![1],
            ..open
        };
        assert!(only_f32.choose(|_, _| true, &[Format::F16]).is_none());
    }

    #[test]
    fn share_and_run_go_round() {
        let share = Share {
            what: MIDDLE,
            index: 1,
            stride: 5760,
            size: 18_874_368,
            handle: 0x1234,
        };
        let bytes = share.encode();
        assert_eq!(bytes.len(), 28);
        assert_eq!(&bytes[0..8], &[1, 0, 0, 0, 1, 0, 0, 0], "何か・番号");
        assert_eq!(Share::decode(&bytes), Ok(share));
        let run = Run {
            value: 7,
            phase: 0.5,
            previous: 2,
            current: 0,
            middle: 1,
        };
        assert_eq!(run.encode().len(), 24);
        assert_eq!(&run.encode()[12..24], &[2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(Run::decode(&run.encode()), Ok(run));
        assert!(Run::decode(&run.encode()[..20]).is_err());
        let opened = Opened {
            way: 4,
            format: 2,
            jobs: 1,
        };
        assert_eq!(Opened::decode(&opened.encode()), Ok(opened));
    }

    #[test]
    fn a_reply_says_which_job() {
        let ran = Ran {
            value: 9,
            gpu_ms: 1.5,
        };
        assert_eq!(ran.encode().len(), 12);
        assert_eq!(Ran::decode(&ran.encode()), Ok(ran));
        assert_eq!(ran.ms(), Some(1.5));
        let unknown = Ran {
            value: 10,
            gpu_ms: -1.0,
        };
        assert_eq!(Ran::decode(&unknown.encode()).map(|r| r.ms()), Ok(None));
        assert!(Ran::decode(&[0; 8]).is_err());
    }

    #[test]
    fn slots_follow_the_jobs() {
        assert_eq!(inputs(1), 2, "一つずつなら前と今");
        assert_eq!(inputs(JOBS), 3, "連続する二つのジョブは間の入力を共有する");
        assert_eq!(port_id(INPUT, 2), 2);
        assert_eq!(port_id(MIDDLE, 1), 257);
    }

    #[test]
    fn framing_carries_the_status() {
        let ran = Ran {
            value: 3,
            gpu_ms: 1.5,
        };
        let ok = reply(Ok(&ran.encode()));
        assert_eq!(head(ok[0..8].try_into().expect("8")), (0, 12));
        assert_eq!(Ran::decode(&ok[8..]), Ok(ran));
        let failed = reply(Err("だめ"));
        let (status, len) = head(failed[0..8].try_into().expect("8"));
        assert_eq!((status as i32, &failed[8..8 + len]), (1, "だめ".as_bytes()));
        assert_eq!(request(RUN, &[1, 2]), vec![3, 0, 0, 0, 2, 0, 0, 0, 1, 2]);
    }

    #[test]
    fn rows_are_padded_to_words() {
        assert_eq!(stride(1440, Format::F32), 5760);
        assert_eq!(stride(1440, Format::F16), 2880);
        assert_eq!(stride(721, Format::F16), 1444);
        assert_eq!(picture_bytes(1080, 5760), 3 * 1080 * 5760);
    }

    #[test]
    fn ways_have_names_and_syncs() {
        for way in Way::ALL {
            assert_eq!(Way::of_name(way.name()), Some(way));
            assert_eq!(Way::of(way as u32), Some(way));
        }
        assert_eq!(Way::of_name("cuda"), None);
        assert_eq!(Way::D3d12.sync(), Sync::Fence);
        assert_eq!(Way::Shm.sync(), Sync::Notify);
    }

    #[test]
    fn a_metal_id_carries_the_service() {
        let id = MetalId {
            registry_id: 0x1_0000_0abc,
            service: String::from("jp.zerowave.process.42.0"),
        };
        let bytes = id.encode();
        assert_eq!(&bytes[0..8], &0x1_0000_0abcu64.to_le_bytes());
        assert_eq!(&bytes[8..], b"jp.zerowave.process.42.0");
        assert_eq!(MetalId::decode(&bytes), Ok(id));
        let bare = MetalId::decode(&7u64.to_le_bytes()).expect("名前は空でよい");
        assert_eq!((bare.registry_id, bare.service.as_str()), (7, ""));
        assert!(MetalId::decode(&[1, 2, 3]).is_err(), "registryID が短い");
        let mut broken = 7u64.to_le_bytes().to_vec();
        broken.push(0xff);
        assert!(MetalId::decode(&broken).is_err(), "UTF-8 でない名前");
    }

    #[test]
    fn requests_are_told_apart_by_op() {
        let run = Run {
            value: 2,
            phase: 0.5,
            previous: 1,
            current: 2,
            middle: 0,
        };
        assert_eq!(Request::decode(RUN, &run.encode()), Ok(Request::Run(run)));
        assert_eq!(
            Request::decode(OPEN, &open().encode()),
            Ok(Request::Open(open()))
        );
        let share = Share {
            what: INPUT,
            index: 2,
            stride: 2880,
            size: 0,
            handle: 0,
        };
        assert_eq!(
            Request::decode(SHARE, &share.encode()),
            Ok(Request::Share(share))
        );
        assert!(Request::decode(9, &[]).is_err(), "未知の op");
        assert!(Request::decode(RUN, &run.encode()[..8]).is_err());
    }

    #[test]
    fn status_lines_go_round() {
        assert_eq!(
            status(Some(0.4), "RIFE のモデル"),
            "@status 0.400 RIFE のモデル"
        );
        assert_eq!(status(None, "読み込み"), "@status - 読み込み");
        assert_eq!(status(Some(2.0), "x"), "@status 1.000 x");
        for (progress, said) in [(Some(0.25f32), "エンジン"), (None, "用意")] {
            assert_eq!(
                status_line(&status(progress, said)),
                Some((progress, String::from(said)))
            );
        }
    }

    #[test]
    fn status_lines_say_how_far() {
        assert_eq!(
            status_line("@status 0.25 エンジンを作る\r"),
            Some((Some(0.25), String::from("エンジンを作る")))
        );
        assert_eq!(
            status_line("@status - 読み込み"),
            Some((None, String::from("読み込み")))
        );
        assert_eq!(status_line("@status 1.5"), Some((Some(1.0), String::new())));
        assert_eq!(
            status_line("@status x 何か"),
            None,
            "割合が読めない行はログへ"
        );
        assert_eq!(status_line("GPU 0 NVIDIA"), None);
    }
}
