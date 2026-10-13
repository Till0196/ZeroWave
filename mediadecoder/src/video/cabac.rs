// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 算術デコーダー。H.264 と HEVC で共通。コンテキストの初期状態は呼ぶ側が作って渡す
//! （H.264 clause 9.3.1.1、HEVC 9.3.2.2）。

/// LPS range table: H.264 Table 9-44, HEVC Table 9-52. 行が状態、列は範囲の上位二ビット。
pub const RANGE_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

/// State transition: H.264 Table 9-45, HEVC Table 9-53. 第一列が LPS の後、第二列が MPS の後。
pub const TRANSITION: [[u8; 2]; 64] = [
    [0, 1],
    [0, 2],
    [1, 3],
    [2, 4],
    [2, 5],
    [4, 6],
    [4, 7],
    [5, 8],
    [6, 9],
    [7, 10],
    [8, 11],
    [9, 12],
    [9, 13],
    [11, 14],
    [11, 15],
    [12, 16],
    [13, 17],
    [13, 18],
    [15, 19],
    [15, 20],
    [16, 21],
    [16, 22],
    [18, 23],
    [18, 24],
    [19, 25],
    [19, 26],
    [21, 27],
    [21, 28],
    [22, 29],
    [22, 30],
    [23, 31],
    [24, 32],
    [24, 33],
    [25, 34],
    [26, 35],
    [26, 36],
    [27, 37],
    [27, 38],
    [28, 39],
    [29, 40],
    [29, 41],
    [30, 42],
    [30, 43],
    [30, 44],
    [31, 45],
    [32, 46],
    [32, 47],
    [33, 48],
    [33, 49],
    [33, 50],
    [34, 51],
    [34, 52],
    [35, 53],
    [35, 54],
    [35, 55],
    [36, 56],
    [36, 57],
    [36, 58],
    [37, 59],
    [37, 60],
    [37, 61],
    [38, 62],
    [38, 62],
    [63, 63],
];

/// 事前状態を、状態と最確シンボルの一つのバイトに詰める。`pre` は 1 から 126 に丸めて渡す。
pub fn packed(pre: i32) -> u8 {
    if pre <= 63 {
        ((63 - pre) as u8) << 1
    } else {
        ((pre - 64) as u8) << 1 | 1
    }
}

#[derive(Debug, Clone)]
pub struct Cabac<'a, const CONTEXTS: usize> {
    bytes: &'a [u8],
    at: usize,
    range: u32,
    offset: u32,
    state: [u8; CONTEXTS],
    overrun: bool,
}

impl<'a, const CONTEXTS: usize> Cabac<'a, CONTEXTS> {
    /// `bytes` must start byte-aligned.
    pub fn over(bytes: &'a [u8], state: [u8; CONTEXTS]) -> Self {
        let mut cabac = Self {
            bytes,
            at: 0,
            range: 510,
            offset: 0,
            state,
            overrun: false,
        };
        cabac.offset = cabac.read(9);
        cabac
    }

    /// What a wavefront stores after the second CTB of a row and a dependent slice inherits.
    pub fn contexts(&self) -> [u8; CONTEXTS] {
        self.state
    }

    pub fn restart(&mut self, at: usize, state: [u8; CONTEXTS]) {
        self.at = at * 8;
        self.range = 510;
        self.state = state;
        self.offset = self.read(9);
    }

    pub fn byte(&self) -> usize {
        self.at.div_ceil(8)
    }

    /// Past the end reads as ones, so a damaged slice cannot decode an endless run.
    fn read(&mut self, count: u32) -> u32 {
        let mut value = 0;
        for _ in 0..count {
            let byte = self.at / 8;
            let bit = match self.bytes.get(byte) {
                Some(byte) => u32::from(byte >> (7 - self.at % 8) & 1),
                None => {
                    self.overrun = true;
                    1
                }
            };
            self.at += 1;
            value = value << 1 | bit;
        }
        value
    }

    pub fn overrun(&self) -> bool {
        self.overrun
    }

    /// H.264 9.3.3.2.1 and 9.3.3.2.2, HEVC 9.3.4.3.2 and 9.3.4.3.2.2.
    pub fn decision(&mut self, context: usize) -> u32 {
        let state = self.state[context];
        let probability = usize::from(state >> 1);
        let mps = u32::from(state & 1);
        let quarter = (self.range >> 6 & 3) as usize;
        let lps = u32::from(RANGE_LPS[probability][quarter]);
        self.range -= lps;

        let bin;
        if self.offset >= self.range {
            bin = 1 - mps;
            self.offset -= self.range;
            self.range = lps;
            let mps = if probability == 0 { 1 - mps } else { mps };
            self.state[context] = TRANSITION[probability][0] << 1 | mps as u8;
        } else {
            bin = mps;
            self.state[context] = TRANSITION[probability][1] << 1 | mps as u8;
        }
        while self.range < 256 {
            self.range <<= 1;
            self.offset = self.offset << 1 | self.read(1);
        }
        bin
    }

    pub fn bypass(&mut self) -> u32 {
        self.offset = self.offset << 1 | self.read(1);
        if self.offset >= self.range {
            self.offset -= self.range;
            1
        } else {
            0
        }
    }

    pub fn bypass_bits(&mut self, count: u32) -> u32 {
        let mut value = 0;
        for _ in 0..count {
            value = value << 1 | self.bypass();
        }
        value
    }

    /// No renormalisation on yes, so the position stays where the data ends.
    pub fn terminate(&mut self) -> bool {
        self.range -= 2;
        if self.offset >= self.range {
            return true;
        }
        while self.range < 256 {
            self.range <<= 1;
            self.offset = self.offset << 1 | self.read(1);
        }
        false
    }

    pub fn golomb(&mut self, mut k: u32) -> u32 {
        let mut value: u32 = 0;
        let mut guard = 0;
        // 壊れたストリームでは k が 32 を超えうる。シフトと足し算があふれないようにする。
        while self.bypass() == 1 && guard < 32 {
            value = value.saturating_add(1u32.checked_shl(k).unwrap_or(u32::MAX));
            k += 1;
            guard += 1;
        }
        while k > 0 {
            k -= 1;
            value = value.saturating_add(self.bypass().checked_shl(k).unwrap_or(0));
        }
        value
    }
}

#[cfg(test)]
pub(crate) struct Coder<const CONTEXTS: usize> {
    low: u32,
    range: u32,
    outstanding: u32,
    first: bool,
    bits: Vec<u8>,
    pub state: [u8; CONTEXTS],
}

#[cfg(test)]
impl<const CONTEXTS: usize> Coder<CONTEXTS> {
    pub fn new(state: [u8; CONTEXTS]) -> Self {
        Self {
            low: 0,
            range: 510,
            outstanding: 0,
            first: true,
            bits: Vec::new(),
            state,
        }
    }

    fn put(&mut self, bit: u32) {
        // The coder's first bit is dropped so its ten-bit low lines up with the nine-bit start.
        if self.first {
            self.first = false;
        } else {
            self.bits.push(bit as u8);
        }
        while self.outstanding > 0 {
            self.bits.push(1 - bit as u8);
            self.outstanding -= 1;
        }
    }

    fn renormalise(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }

    pub fn decision(&mut self, context: usize, bin: u32) {
        let state = self.state[context];
        let probability = usize::from(state >> 1);
        let mps = u32::from(state & 1);
        let quarter = (self.range >> 6 & 3) as usize;
        let lps = u32::from(RANGE_LPS[probability][quarter]);
        self.range -= lps;
        if bin != mps {
            self.low += self.range;
            self.range = lps;
            let mps = if probability == 0 { 1 - mps } else { mps };
            self.state[context] = TRANSITION[probability][0] << 1 | mps as u8;
        } else {
            self.state[context] = TRANSITION[probability][1] << 1 | mps as u8;
        }
        self.renormalise();
    }

    pub fn bypass(&mut self, bin: u32) {
        self.low <<= 1;
        if bin == 1 {
            self.low += self.range;
        }
        if self.low >= 1024 {
            self.put(1);
            self.low -= 1024;
        } else if self.low < 512 {
            self.put(0);
        } else {
            self.low -= 512;
            self.outstanding += 1;
        }
    }

    pub fn terminate(&mut self, bin: u32) {
        self.range -= 2;
        if bin == 1 {
            self.low += self.range;
            self.range = 2;
        }
        self.renormalise();
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.terminate(1);
        self.range = 2;
        self.renormalise();
        self.put((self.low >> 9) & 1);
        self.bits.push(((self.low >> 8) & 1) as u8);
        self.bits.push(1);
        let mut bytes = vec![0u8; self.bits.len().div_ceil(8) + 4];
        for (at, &bit) in self.bits.iter().enumerate() {
            bytes[at / 8] |= bit << (7 - at % 8);
        }
        bytes
    }
}

#[cfg(test)]
pub(crate) fn round_trip<const CONTEXTS: usize>(initial: [u8; CONTEXTS], length: usize) {
    let contexts: Vec<usize> = (0..length).map(|at| (at * 7 + 11) % CONTEXTS).collect();
    let bins: Vec<u32> = (0..length)
        .map(|at: usize| (at * at + at / 3).is_multiple_of(3) as u32)
        .collect();
    let mut coder = Coder::new(initial);
    for (&context, &bin) in contexts.iter().zip(&bins) {
        coder.decision(context, bin);
        coder.bypass(bin ^ 1);
        coder.terminate(0);
    }
    let bytes = coder.finish();
    let mut cabac = Cabac::over(&bytes, initial);
    for (at, (&context, &bin)) in contexts.iter().zip(&bins).enumerate() {
        assert_eq!(cabac.decision(context), bin, "decision {at}");
        assert_eq!(cabac.bypass(), bin ^ 1, "bypass {at}");
        assert!(!cabac.terminate(), "terminate {at}");
    }
    assert!(cabac.terminate());
    assert!(!cabac.overrun());
}
