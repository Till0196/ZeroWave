// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-1 and MPEG-2 audio, layers I and II (ISO/IEC 11172-3, 13818-3). No Japanese broadcast
//! standard uses it, but cable operators' own channels do (stream type 0x03/0x04). No layer III.

use std::collections::VecDeque;

use super::{Decoder, Error, Format, Object, Sound, Speaker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// True for MPEG-1, false for MPEG-2 (the lower sampling frequencies).
    pub mpeg1: bool,
    pub layer: u8,
    pub crc: bool,
    pub bitrate: u32,
    pub rate: u32,
    pub padding: bool,
    /// Nought stereo, one joint stereo, two dual channel, three mono.
    pub mode: u8,
    pub mode_extension: u8,
    pub length: usize,
}

const HEADER: usize = 4;

/// Bit rates in kilobits by table and four bit index; free format (0) and 15 are refused.
const BITRATES: [[u32; 16]; 4] = [
    [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0,
    ],
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
    ],
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0,
    ],
    [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ],
];

impl Header {
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let b = bytes.get(..HEADER)?;
        if b[0] != 0xff || b[1] & 0xe0 != 0xe0 {
            return None;
        }
        let mpeg1 = match (b[1] >> 3) & 0x03 {
            0b11 => true,
            0b10 => false,
            // MPEG-2.5 and the reserved value.
            _ => return None,
        };
        let layer = match (b[1] >> 1) & 0x03 {
            0b11 => 1,
            0b10 => 2,
            _ => return None,
        };
        let crc = b[1] & 0x01 == 0;
        let bitrate_index = usize::from(b[2] >> 4);
        let rate_index = usize::from((b[2] >> 2) & 0x03);
        if rate_index == 3 {
            return None;
        }
        let table = usize::from(!mpeg1) * 2 + usize::from(layer - 1);
        let bitrate = BITRATES[table][bitrate_index] * 1000;
        if bitrate == 0 {
            return None;
        }
        let rate = [44100, 48000, 32000][rate_index] / if mpeg1 { 1 } else { 2 };
        let padding = (b[2] >> 1) & 0x01 != 0;
        let mode = b[3] >> 6;
        let mode_extension = (b[3] >> 4) & 0x03;
        let length = if layer == 1 {
            (12 * bitrate / rate + u32::from(padding)) as usize * 4
        } else {
            (144 * bitrate / rate + u32::from(padding)) as usize
        };
        Some(Self {
            mpeg1,
            layer,
            crc,
            bitrate,
            rate,
            padding,
            mode,
            mode_extension,
            length,
        })
    }

    pub fn channels(&self) -> usize {
        if self.mode == 3 { 1 } else { 2 }
    }

    /// What the header declares, as a [`Format`] (object 32/33 for the layer).
    pub fn format(&self) -> Format {
        Format {
            object: if self.layer == 1 {
                Object::Layer1
            } else {
                Object::Layer2
            },
            rate: self.rate,
            configuration: self.channels() as u8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub header: Header,
    pub format: Format,
    pub data: &'a [u8],
}

#[derive(Debug)]
pub struct Frames<'a> {
    bytes: &'a [u8],
    at: usize,
    skipped: usize,
}

impl<'a> Frames<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            skipped: 0,
        }
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.at.min(self.bytes.len())..]
    }

    pub fn skipped(&self) -> usize {
        self.skipped
    }
}

impl<'a> Iterator for Frames<'a> {
    type Item = Frame<'a>;

    fn next(&mut self) -> Option<Frame<'a>> {
        loop {
            let here = self.bytes.get(self.at..)?;
            if here.len() < HEADER {
                return None;
            }
            let Some(header) = Header::read(here) else {
                self.at += 1;
                self.skipped += 1;
                continue;
            };
            if here.len() < header.length {
                return None;
            }
            // A header is only believed if another follows it (or the stream ends).
            if let Some(next) = here.get(header.length..)
                && next.len() >= HEADER
                && Header::read(next).is_none()
            {
                self.at += 1;
                self.skipped += 1;
                continue;
            }
            let data = &here[..header.length];
            self.at += header.length;
            return Some(Frame {
                header,
                format: header.format(),
                data,
            });
        }
    }
}

struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn read(&mut self, count: u32) -> u32 {
        let mut out = 0u32;
        for _ in 0..count {
            let byte = self.bytes.get(self.at >> 3).copied().unwrap_or(0);
            out = (out << 1) | u32::from((byte >> (7 - (self.at & 7))) & 1);
            self.at += 1;
        }
        out
    }

    fn overran(&self) -> bool {
        self.at > self.bytes.len() * 8
    }
}

/// Table 3-B.1: 2^(1 - index/3). Sixty-three is not a scalefactor.
fn scalefactor(index: u32) -> f32 {
    2.0f32.powf(1.0 - index as f32 / 3.0)
}

/// A quantiser class; `grouped` classes pack three samples into one code.
#[derive(Debug, Clone, Copy)]
struct Quantiser {
    levels: u32,
    grouped: bool,
    bits: u32,
    c: f32,
    d: f32,
}

impl Quantiser {
    /// The class with this many levels (Table 3-B.4 for layer II, 2^n - 1 for layer I).
    fn with_levels(levels: u32) -> Self {
        let (grouped, bits) = match levels {
            3 => (true, 5),
            5 => (true, 7),
            9 => (true, 10),
            other => (false, (other + 1).trailing_zeros()),
        };
        // Requantisation is `c · (x + d)`; the constants close the gap of the missing top level.
        let (c, d) = match levels {
            3 => (4.0 / 3.0, 0.5),
            5 => (8.0 / 5.0, 0.5),
            9 => (16.0 / 9.0, 0.5),
            other => {
                let n = (other + 1).trailing_zeros();
                let full = (1u64 << n) as f32;
                (full / (full - 1.0), 1.0 / (1u64 << (n - 1)) as f32)
            }
        };
        Self {
            levels,
            grouped,
            bits,
            c,
            d,
        }
    }

    fn sample_bits(&self) -> u32 {
        32 - self.levels.leading_zeros()
    }

    fn value(&self, code: u32) -> f32 {
        let nb = self.sample_bits();
        // The most significant bit is the inverted sign.
        let flipped = code ^ (1 << (nb - 1));
        let signed = if flipped & (1 << (nb - 1)) != 0 {
            flipped as i64 - (1i64 << nb)
        } else {
            flipped as i64
        };
        let fraction = signed as f32 / (1u64 << (nb - 1)) as f32;
        self.c * (fraction + self.d)
    }

    fn three(&self, bits: &mut Bits<'_>) -> [f32; 3] {
        if self.grouped {
            let mut code = bits.read(self.bits);
            let mut out = [0.0; 3];
            for sample in &mut out {
                *sample = self.value(code % self.levels);
                code /= self.levels;
            }
            out
        } else {
            let nb = self.sample_bits();
            [
                self.value(bits.read(nb)),
                self.value(bits.read(nb)),
                self.value(bits.read(nb)),
            ]
        }
    }
}

/// Layer II bit allocation lists (Tables 3-B.2a–d of 11172-3, B.1 of 13818-3).
const LISTS: [&[u32]; 6] = [
    &[
        3, 7, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16383, 32767, 65535,
    ],
    &[
        3, 5, 7, 9, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 65535,
    ],
    &[3, 5, 7, 9, 15, 31, 65535],
    &[3, 5, 65535],
    &[
        3, 5, 9, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16383, 32767,
    ],
    &[3, 5, 9, 15, 31, 63, 127],
];

/// Which list each subband uses, per table; the length is the subband limit.
const TABLES: [&[u8]; 5] = [
    &[
        0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3,
    ],
    &[
        0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3,
    ],
    &[4, 4, 5, 5, 5, 5, 5, 5],
    &[4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    // 13818-3 B.1: 30 subbands, for the lower sampling frequencies.
    &[
        1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    ],
];

/// The seventh list, only in the MPEG-2 table: three levels of two bits.
const LOW: &[u32] = &[3, 5, 7];

fn list(which: u8) -> &'static [u32] {
    if which == 6 {
        LOW
    } else {
        LISTS[usize::from(which)]
    }
}

/// Bits an allocation from this list takes: the smallest that holds the list and nought.
fn allocation_bits(levels: &[u32]) -> u32 {
    (levels.len() as u32 + 1)
        .next_power_of_two()
        .trailing_zeros()
}

/// Which table a layer II frame uses (11172-3 2.4.2.3, 13818-3 2.4.2.3).
fn table_of(header: &Header) -> &'static [u8] {
    if !header.mpeg1 {
        return TABLES[4];
    }
    let per_channel = header.bitrate / header.channels() as u32;
    let index = if per_channel <= 48_000 {
        if header.rate == 32_000 { 3 } else { 2 }
    } else if per_channel <= 80_000 || header.rate == 48_000 {
        0
    } else {
        1
    };
    TABLES[index]
}

#[derive(Debug, Clone)]
struct Synthesis {
    v: Vec<f32>,
}

impl Synthesis {
    fn new() -> Self {
        Self { v: vec![0.0; 1024] }
    }

    fn reset(&mut self) {
        self.v.iter_mut().for_each(|x| *x = 0.0);
    }

    fn run(&mut self, subbands: &[f32; 32], out: &mut [f32]) {
        self.v.copy_within(0..960, 64);
        for i in 0..64 {
            let mut sum = 0.0f32;
            for (k, s) in subbands.iter().enumerate() {
                sum += cosine(i, k) * s;
            }
            self.v[i] = sum;
        }
        let mut u = [0.0f32; 512];
        for i in 0..8 {
            for j in 0..32 {
                u[i * 64 + j] = self.v[i * 128 + j];
                u[i * 64 + 32 + j] = self.v[i * 128 + 96 + j];
            }
        }
        for (w, d) in u.iter_mut().zip(WINDOW.iter()) {
            *w *= d;
        }
        for (j, sample) in out.iter_mut().enumerate().take(32) {
            let mut sum = 0.0f32;
            for i in 0..16 {
                sum += u[j + 32 * i];
            }
            *sample = sum;
        }
    }
}

fn cosine(i: usize, k: usize) -> f32 {
    use std::sync::OnceLock;
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut out = Vec::with_capacity(64 * 32);
        for i in 0..64 {
            for k in 0..32 {
                let angle = (16 + i) as f64 * (2 * k + 1) as f64 * std::f64::consts::PI / 64.0;
                out.push(angle.cos() as f32);
            }
        }
        out
    });
    table[i * 32 + k]
}

#[derive(Debug, Clone)]
pub struct Mpeg {
    synthesis: [Synthesis; 2],
    ready: VecDeque<Sound>,
}

impl Default for Mpeg {
    fn default() -> Self {
        Self::new()
    }
}

impl Mpeg {
    pub fn new() -> Self {
        Self {
            synthesis: [Synthesis::new(), Synthesis::new()],
            ready: VecDeque::new(),
        }
    }

    /// Layer II (11172-3 2.4.1.6, 2.4.2.3).
    fn layer2(&mut self, header: &Header, bits: &mut Bits<'_>) -> Result<Sound, Error> {
        let channels = header.channels();
        let table = table_of(header);
        let sblimit = table.len();
        let bound = if header.mode == 1 {
            (4 + usize::from(header.mode_extension) * 4).min(sblimit)
        } else {
            sblimit
        };
        let mut allocation = [[0u32; 32]; 2];
        for sb in 0..sblimit {
            let levels = list(table[sb]);
            let nbal = allocation_bits(levels);
            let shared = sb >= bound;
            for ch in 0..channels {
                if shared && ch == 1 {
                    allocation[1][sb] = allocation[0][sb];
                } else {
                    allocation[ch][sb] = bits.read(nbal);
                }
                if allocation[ch][sb] as usize > levels.len() {
                    return Err(Error::Damaged("layer II allocation"));
                }
            }
        }
        let mut scfsi = [[0u32; 32]; 2];
        for sb in 0..sblimit {
            for ch in 0..channels {
                if allocation[ch][sb] != 0 {
                    scfsi[ch][sb] = bits.read(2);
                }
            }
        }
        let mut scale = [[[1.0f32; 3]; 32]; 2];
        for sb in 0..sblimit {
            for ch in 0..channels {
                if allocation[ch][sb] == 0 {
                    continue;
                }
                let s = &mut scale[ch][sb];
                match scfsi[ch][sb] {
                    0 => {
                        s[0] = scalefactor(bits.read(6));
                        s[1] = scalefactor(bits.read(6));
                        s[2] = scalefactor(bits.read(6));
                    }
                    1 => {
                        s[0] = scalefactor(bits.read(6));
                        s[1] = s[0];
                        s[2] = scalefactor(bits.read(6));
                    }
                    2 => {
                        s[0] = scalefactor(bits.read(6));
                        s[1] = s[0];
                        s[2] = s[0];
                    }
                    _ => {
                        s[0] = scalefactor(bits.read(6));
                        s[2] = scalefactor(bits.read(6));
                        s[1] = s[2];
                    }
                }
            }
        }
        if bits.overran() {
            return Err(Error::Damaged("layer II side information"));
        }
        let mut subbands = vec![[[0.0f32; 32]; 36]; channels];
        for granule in 0..12 {
            let part = granule / 4;
            for sb in 0..sblimit {
                let levels = list(table[sb]);
                let shared = sb >= bound;
                for ch in 0..channels {
                    let index = allocation[ch][sb];
                    if index == 0 {
                        continue;
                    }
                    if shared && ch == 1 {
                        // Read once for both channels, scaled apart.
                        continue;
                    }
                    let quantiser = Quantiser::with_levels(levels[index as usize - 1]);
                    let three = quantiser.three(bits);
                    for (t, value) in three.iter().enumerate() {
                        let at = granule * 3 + t;
                        subbands[ch][at][sb] = value * scale[ch][sb][part];
                        if shared && channels == 2 {
                            subbands[1][at][sb] = value * scale[1][sb][part];
                        }
                    }
                }
            }
        }
        if bits.overran() {
            return Err(Error::Damaged("layer II samples"));
        }
        Ok(self.synthesise(header, &subbands, 36))
    }

    /// Layer I (11172-3 2.4.1.5, 2.4.2.2).
    // 同じ添字で複数のテーブルを引くので、範囲の for のほうが読める。
    #[allow(clippy::needless_range_loop)]
    fn layer1(&mut self, header: &Header, bits: &mut Bits<'_>) -> Result<Sound, Error> {
        let channels = header.channels();
        let bound = if header.mode == 1 {
            4 + usize::from(header.mode_extension) * 4
        } else {
            32
        };
        let mut allocation = [[0u32; 32]; 2];
        for sb in 0..32 {
            for ch in 0..channels {
                if sb >= bound && ch == 1 {
                    allocation[1][sb] = allocation[0][sb];
                } else {
                    allocation[ch][sb] = bits.read(4);
                }
                if allocation[ch][sb] == 15 {
                    return Err(Error::Damaged("layer I allocation"));
                }
            }
        }
        let mut scale = [[1.0f32; 32]; 2];
        for sb in 0..32 {
            for ch in 0..channels {
                if allocation[ch][sb] != 0 {
                    scale[ch][sb] = scalefactor(bits.read(6));
                }
            }
        }
        if bits.overran() {
            return Err(Error::Damaged("layer I side information"));
        }
        let mut subbands = vec![[[0.0f32; 32]; 36]; channels];
        for t in 0..12 {
            for sb in 0..32 {
                for ch in 0..channels {
                    let index = allocation[ch][sb];
                    if index == 0 || (sb >= bound && ch == 1) {
                        continue;
                    }
                    let nb = index + 1;
                    let quantiser = Quantiser::with_levels((1 << nb) - 1);
                    let value = quantiser.value(bits.read(nb));
                    subbands[ch][t][sb] = value * scale[ch][sb];
                    if sb >= bound && channels == 2 {
                        subbands[1][t][sb] = value * scale[1][sb];
                    }
                }
            }
        }
        if bits.overran() {
            return Err(Error::Damaged("layer I samples"));
        }
        Ok(self.synthesise(header, &subbands, 12))
    }

    fn synthesise(&mut self, header: &Header, subbands: &[[[f32; 32]; 36]], times: usize) -> Sound {
        let channels = header.channels();
        let mut out = vec![vec![0.0f32; times * 32]; channels];
        for ch in 0..channels {
            for t in 0..times {
                self.synthesis[ch].run(&subbands[ch][t], &mut out[ch][t * 32..t * 32 + 32]);
            }
        }
        Sound {
            rate: header.rate,
            speakers: if channels == 1 {
                vec![Speaker::FrontCentre]
            } else {
                vec![Speaker::FrontLeft, Speaker::FrontRight]
            },
            channels: out,
            mixdown: Default::default(),
        }
    }
}

impl Decoder for Mpeg {
    fn decode(&mut self, _format: Format, data: &[u8]) -> Result<(), Error> {
        let Some(header) = Header::read(data) else {
            return Err(Error::Damaged("MPEG audio header"));
        };
        if data.len() < header.length {
            return Err(Error::Damaged("MPEG audio frame short"));
        }
        let mut bits = Bits {
            bytes: &data[..header.length],
            at: HEADER * 8 + if header.crc { 16 } else { 0 },
        };
        let sound = match header.layer {
            1 => self.layer1(&header, &mut bits)?,
            _ => self.layer2(&header, &mut bits)?,
        };
        self.ready.push_back(sound);
        Ok(())
    }

    fn take(&mut self) -> Option<Sound> {
        self.ready.pop_front()
    }

    fn reset(&mut self) {
        for synthesis in &mut self.synthesis {
            synthesis.reset();
        }
        self.ready.clear();
    }
}

/// The synthesis window, Table 3-B.3 of ISO/IEC 11172-3（桁は規格のまま、f32 で切り捨て）。
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const WINDOW: [f32; 512] = [
    0.000000000, -0.000015259, -0.000015259, -0.000015259, -0.000015259, -0.000015259, -0.000015259, -0.000030518,
    -0.000030518, -0.000030518, -0.000030518, -0.000045776, -0.000045776, -0.000061035, -0.000061035, -0.000076294,
    -0.000076294, -0.000091553, -0.000106812, -0.000106812, -0.000122070, -0.000137329, -0.000152588, -0.000167847,
    -0.000198364, -0.000213623, -0.000244141, -0.000259399, -0.000289917, -0.000320435, -0.000366211, -0.000396729,
    -0.000442505, -0.000473022, -0.000534058, -0.000579834, -0.000625610, -0.000686646, -0.000747681, -0.000808716,
    -0.000885010, -0.000961304, -0.001037598, -0.001113892, -0.001205444, -0.001296997, -0.001388550, -0.001480103,
    -0.001586914, -0.001693726, -0.001785278, -0.001907349, -0.002014160, -0.002120972, -0.002243042, -0.002349854,
    -0.002456665, -0.002578735, -0.002685547, -0.002792358, -0.002899170, -0.002990723, -0.003082275, -0.003173828,
    0.003250122, 0.003326416, 0.003387451, 0.003433228, 0.003463745, 0.003479004, 0.003479004, 0.003463745,
    0.003417969, 0.003372192, 0.003280640, 0.003173828, 0.003051758, 0.002883911, 0.002700806, 0.002487183,
    0.002227783, 0.001937866, 0.001617432, 0.001266479, 0.000869751, 0.000442505, -0.000030518, -0.000549316,
    -0.001098633, -0.001693726, -0.002334595, -0.003005981, -0.003723145, -0.004486084, -0.005294800, -0.006118774,
    -0.007003784, -0.007919312, -0.008865356, -0.009841919, -0.010848999, -0.011886597, -0.012939453, -0.014022827,
    -0.015121460, -0.016235352, -0.017349243, -0.018463135, -0.019577026, -0.020690918, -0.021789551, -0.022857666,
    -0.023910522, -0.024932861, -0.025909424, -0.026840210, -0.027725220, -0.028533936, -0.029281616, -0.029937744,
    -0.030532837, -0.031005859, -0.031387329, -0.031661987, -0.031814575, -0.031845093, -0.031738281, -0.031478882,
    0.031082153, 0.030517578, 0.029785156, 0.028884888, 0.027801514, 0.026535034, 0.025085449, 0.023422241,
    0.021575928, 0.019531250, 0.017257690, 0.014801025, 0.012115479, 0.009231567, 0.006134033, 0.002822876,
    -0.000686646, -0.004394531, -0.008316040, -0.012420654, -0.016708374, -0.021179199, -0.025817871, -0.030609131,
    -0.035552979, -0.040634155, -0.045837402, -0.051132202, -0.056533813, -0.061996460, -0.067520142, -0.073059082,
    -0.078628540, -0.084182739, -0.089706421, -0.095169067, -0.100540161, -0.105819702, -0.110946655, -0.115921021,
    -0.120697021, -0.125259399, -0.129562378, -0.133590698, -0.137298584, -0.140670776, -0.143676758, -0.146255493,
    -0.148422241, -0.150115967, -0.151306152, -0.151962280, -0.152069092, -0.151596069, -0.150497437, -0.148773193,
    -0.146362305, -0.143264771, -0.139450073, -0.134887695, -0.129577637, -0.123474121, -0.116577148, -0.108856201,
    0.100311279, 0.090927124, 0.080688477, 0.069595337, 0.057617188, 0.044784546, 0.031082153, 0.016510010,
    0.001068115, -0.015228271, -0.032379150, -0.050354004, -0.069168091, -0.088775635, -0.109161377, -0.130310059,
    -0.152206421, -0.174789429, -0.198059082, -0.221984863, -0.246505737, -0.271591187, -0.297210693, -0.323318481,
    -0.349868774, -0.376800537, -0.404083252, -0.431655884, -0.459472656, -0.487472534, -0.515609741, -0.543823242,
    -0.572036743, -0.600219727, -0.628295898, -0.656219482, -0.683914185, -0.711318970, -0.738372803, -0.765029907,
    -0.791213989, -0.816864014, -0.841949463, -0.866363525, -0.890090942, -0.913055420, -0.935195923, -0.956481934,
    -0.976852417, -0.996246338, -1.014617920, -1.031936646, -1.048156738, -1.063217163, -1.077117920, -1.089782715,
    -1.101211548, -1.111373901, -1.120223999, -1.127746582, -1.133926392, -1.138763428, -1.142211914, -1.144287109,
    1.144989014, 1.144287109, 1.142211914, 1.138763428, 1.133926392, 1.127746582, 1.120223999, 1.111373901,
    1.101211548, 1.089782715, 1.077117920, 1.063217163, 1.048156738, 1.031936646, 1.014617920, 0.996246338,
    0.976852417, 0.956481934, 0.935195923, 0.913055420, 0.890090942, 0.866363525, 0.841949463, 0.816864014,
    0.791213989, 0.765029907, 0.738372803, 0.711318970, 0.683914185, 0.656219482, 0.628295898, 0.600219727,
    0.572036743, 0.543823242, 0.515609741, 0.487472534, 0.459472656, 0.431655884, 0.404083252, 0.376800537,
    0.349868774, 0.323318481, 0.297210693, 0.271591187, 0.246505737, 0.221984863, 0.198059082, 0.174789429,
    0.152206421, 0.130310059, 0.109161377, 0.088775635, 0.069168091, 0.050354004, 0.032379150, 0.015228271,
    -0.001068115, -0.016510010, -0.031082153, -0.044784546, -0.057617188, -0.069595337, -0.080688477, -0.090927124,
    0.100311279, 0.108856201, 0.116577148, 0.123474121, 0.129577637, 0.134887695, 0.139450073, 0.143264771,
    0.146362305, 0.148773193, 0.150497437, 0.151596069, 0.152069092, 0.151962280, 0.151306152, 0.150115967,
    0.148422241, 0.146255493, 0.143676758, 0.140670776, 0.137298584, 0.133590698, 0.129562378, 0.125259399,
    0.120697021, 0.115921021, 0.110946655, 0.105819702, 0.100540161, 0.095169067, 0.089706421, 0.084182739,
    0.078628540, 0.073059082, 0.067520142, 0.061996460, 0.056533813, 0.051132202, 0.045837402, 0.040634155,
    0.035552979, 0.030609131, 0.025817871, 0.021179199, 0.016708374, 0.012420654, 0.008316040, 0.004394531,
    0.000686646, -0.002822876, -0.006134033, -0.009231567, -0.012115479, -0.014801025, -0.017257690, -0.019531250,
    -0.021575928, -0.023422241, -0.025085449, -0.026535034, -0.027801514, -0.028884888, -0.029785156, -0.030517578,
    0.031082153, 0.031478882, 0.031738281, 0.031845093, 0.031814575, 0.031661987, 0.031387329, 0.031005859,
    0.030532837, 0.029937744, 0.029281616, 0.028533936, 0.027725220, 0.026840210, 0.025909424, 0.024932861,
    0.023910522, 0.022857666, 0.021789551, 0.020690918, 0.019577026, 0.018463135, 0.017349243, 0.016235352,
    0.015121460, 0.014022827, 0.012939453, 0.011886597, 0.010848999, 0.009841919, 0.008865356, 0.007919312,
    0.007003784, 0.006118774, 0.005294800, 0.004486084, 0.003723145, 0.003005981, 0.002334595, 0.001693726,
    0.001098633, 0.000549316, 0.000030518, -0.000442505, -0.000869751, -0.001266479, -0.001617432, -0.001937866,
    -0.002227783, -0.002487183, -0.002700806, -0.002883911, -0.003051758, -0.003173828, -0.003280640, -0.003372192,
    -0.003417969, -0.003463745, -0.003479004, -0.003479004, -0.003463745, -0.003433228, -0.003387451, -0.003326416,
    0.003250122, 0.003173828, 0.003082275, 0.002990723, 0.002899170, 0.002792358, 0.002685547, 0.002578735,
    0.002456665, 0.002349854, 0.002243042, 0.002120972, 0.002014160, 0.001907349, 0.001785278, 0.001693726,
    0.001586914, 0.001480103, 0.001388550, 0.001296997, 0.001205444, 0.001113892, 0.001037598, 0.000961304,
    0.000885010, 0.000808716, 0.000747681, 0.000686646, 0.000625610, 0.000579834, 0.000534058, 0.000473022,
    0.000442505, 0.000396729, 0.000366211, 0.000320435, 0.000289917, 0.000259399, 0.000244141, 0.000213623,
    0.000198364, 0.000167847, 0.000152588, 0.000137329, 0.000122070, 0.000106812, 0.000106812, 0.000091553,
    0.000076294, 0.000076294, 0.000061035, 0.000061035, 0.000045776, 0.000045776, 0.000030518, 0.000030518,
    0.000030518, 0.000030518, 0.000015259, 0.000015259, 0.000015259, 0.000015259, 0.000015259, 0.000015259,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_ii_header_is_read() {
        // Sync, MPEG-1, layer II, no CRC; 256 kbit/s, 48 kHz; stereo.
        let header = Header::read(&[0xff, 0xfd, 0xc4, 0x00]).expect("a header");
        assert!(header.mpeg1);
        assert_eq!(header.layer, 2);
        assert!(!header.crc);
        assert_eq!(header.bitrate, 256_000);
        assert_eq!(header.rate, 48_000);
        assert_eq!(header.length, 768);
        assert_eq!(header.channels(), 2);
        let padded = Header::read(&[0xff, 0xfd, 0xc6, 0xc0]).expect("a header");
        assert_eq!(padded.length, 769);
        assert_eq!(padded.channels(), 1);
        assert!(Header::read(&[0xff, 0xfd, 0x04, 0x00]).is_none());
        assert!(Header::read(&[0xff, 0xfd, 0xf4, 0x00]).is_none());
        assert!(Header::read(&[0xff, 0xfd, 0xcc, 0x00]).is_none());
        let low = Header::read(&[0xff, 0xf5, 0xe4, 0x00]).expect("a header");
        assert!(!low.mpeg1);
        assert_eq!(low.rate, 24_000);
        assert_eq!(low.bitrate, 160_000);
        assert_eq!(low.length, 960);
    }

    #[test]
    fn requantisation_is_symmetric_and_reaches_the_ends() {
        let three = Quantiser::with_levels(3);
        assert!((three.value(0) + 2.0 / 3.0).abs() < 1e-6);
        assert!(three.value(1).abs() < 1e-6);
        assert!((three.value(2) - 2.0 / 3.0).abs() < 1e-6);
        let five = Quantiser::with_levels(5);
        assert!((five.value(0) + 0.8).abs() < 1e-6);
        assert!((five.value(4) - 0.8).abs() < 1e-6);
        let fifteen = Quantiser::with_levels(15);
        assert!((fifteen.value(7)).abs() < 1e-6, "{}", fifteen.value(7));
        assert!((fifteen.value(0) + 14.0 / 15.0).abs() < 1e-5);
        assert!((fifteen.value(14) - 14.0 / 15.0).abs() < 1e-5);
        let bytes = [0b0010_1000u8, 0];
        let mut bits = Bits {
            bytes: &bytes,
            at: 0,
        };
        // 2 + 3·1 + 9·0 = 5 → samples 2, 1, 0 → 2/3, 0, -2/3.
        let out = three.three(&mut bits);
        assert!((out[0] - 2.0 / 3.0).abs() < 1e-6);
        assert!(out[1].abs() < 1e-6);
        assert!((out[2] + 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(allocation_bits(LISTS[0]), 4);
        assert_eq!(allocation_bits(LISTS[2]), 3);
        assert_eq!(allocation_bits(LISTS[3]), 2);
        assert_eq!(allocation_bits(LOW), 2);
    }

    #[test]
    fn silence_decodes_to_silence_and_a_subband_is_a_tone() {
        let mut frame = vec![0u8; 768];
        frame[..4].copy_from_slice(&[0xff, 0xfd, 0xc4, 0x00]);
        let mut decoder = Mpeg::new();
        decoder
            .decode(Header::read(&frame).unwrap().format(), &frame)
            .expect("decodes");
        let sound = decoder.take().expect("a run");
        assert_eq!(sound.rate, 48_000);
        assert_eq!(sound.channels.len(), 2);
        assert_eq!(sound.len(), 1152);
        assert!(sound.channels[0].iter().all(|s| *s == 0.0));
        let mut synthesis = Synthesis::new();
        let mut out = vec![0.0f32; 32 * 64];
        let mut subbands = [0.0f32; 32];
        for t in 0..64 {
            subbands[1] = if t % 2 == 0 { 1.0 } else { -1.0 };
            synthesis.run(&subbands, &mut out[t * 32..t * 32 + 32]);
        }
        let peak = out[1024..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.5 && peak < 1.5, "{peak}");
    }

    #[test]
    fn frames_are_cut_by_their_headers() {
        let mut stream = Vec::new();
        for _ in 0..3 {
            let mut frame = vec![0u8; 768];
            frame[..4].copy_from_slice(&[0xff, 0xfd, 0xc4, 0x00]);
            stream.extend_from_slice(&frame);
        }
        stream.extend_from_slice(&[0xff, 0xfd, 0xc4, 0x00, 1, 2, 3]);
        let mut frames = Frames::new(&stream);
        assert_eq!(frames.by_ref().count(), 3);
        assert_eq!(frames.rest().len(), 7);
        assert_eq!(frames.skipped(), 0);
        let mut junk = vec![0xffu8, 0xfd, 0x00];
        junk.extend_from_slice(&stream);
        let mut frames = Frames::new(&junk);
        assert_eq!(frames.by_ref().count(), 3);
        assert!(frames.skipped() >= 3);
    }
}
