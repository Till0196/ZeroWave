// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Reading fields that are not a whole number of bytes.

/// A run of bytes read a bit at a time. Reading past the end gives zeroes rather than failing.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub fn empty(&self) -> bool {
        self.at >= self.bytes.len() * 8
    }

    pub fn left(&self) -> usize {
        (self.bytes.len() * 8).saturating_sub(self.at)
    }

    pub fn at(&self) -> usize {
        self.at
    }

    pub fn bit(&mut self) -> u32 {
        let byte = self.at / 8;
        let shift = 7 - (self.at % 8);
        self.at += 1;
        match self.bytes.get(byte) {
            Some(value) => u32::from(value >> shift & 1),
            None => 0,
        }
    }

    /// The next `count` bits, most significant first. At most 32.
    pub fn bits(&mut self, count: usize) -> u32 {
        let mut value = 0;
        for _ in 0..count.min(32) {
            value = value << 1 | self.bit();
        }
        value
    }

    /// An unsigned exponential-Golomb code; over 32 leading zeroes stops reading.
    pub fn ue(&mut self) -> u32 {
        let mut zeroes = 0;
        while self.bit() == 0 && zeroes < 32 && !self.empty() {
            zeroes += 1;
        }
        if zeroes >= 32 {
            return 0;
        }
        (1u32 << zeroes) - 1 + self.bits(zeroes)
    }

    pub fn se(&mut self) -> i32 {
        let value = self.ue();
        let magnitude = value.div_ceil(2) as i32;
        if value.is_multiple_of(2) {
            -magnitude
        } else {
            magnitude
        }
    }

    pub fn skip(&mut self, count: usize) {
        self.at += count;
    }

    pub fn align(&mut self) {
        self.at = self.at.div_ceil(8) * 8;
    }
}

/// Takes out emulation prevention bytes (0x03 after two zeroes), only up to `most`.
pub fn unescape(bytes: &[u8], most: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len().min(most));
    let mut zeroes = 0;
    for &byte in bytes {
        if out.len() >= most {
            break;
        }
        if zeroes == 2 && byte == 0x03 {
            zeroes = 0;
            continue;
        }
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
        out.push(byte);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Reader, unescape};

    #[test]
    fn bits_come_out_most_significant_first() {
        let mut reader = Reader::new(&[0b1011_0010, 0xff]);
        assert_eq!(reader.bit(), 1);
        assert_eq!(reader.bits(3), 0b011);
        assert_eq!(reader.bits(4), 0b0010);
        assert_eq!(reader.bits(8), 0xff);
        assert!(reader.empty());
    }

    #[test]
    fn reading_past_the_end_gives_zeroes() {
        let mut reader = Reader::new(&[0xff]);
        assert_eq!(reader.bits(8), 0xff);
        assert_eq!(reader.bits(8), 0);
    }

    #[test]
    fn golomb_codes_come_out_as_the_standard_tabulates_them() {
        let mut reader = Reader::new(&[0b1010_0110, 0b0100_0010, 0b1001_1000, 0b1110_0000]);
        assert_eq!(reader.ue(), 0);
        assert_eq!(reader.ue(), 1);
        assert_eq!(reader.ue(), 2);
        assert_eq!(reader.ue(), 3);
        assert_eq!(reader.ue(), 4);
        assert_eq!(reader.ue(), 5);
        assert_eq!(reader.ue(), 6);
    }

    #[test]
    fn signed_golomb_codes_alternate_around_nought() {
        let mut reader = Reader::new(&[0b1010_0110, 0b0100_0010, 0b1001_1000, 0b1110_0000]);
        assert_eq!(reader.se(), 0);
        assert_eq!(reader.se(), 1);
        assert_eq!(reader.se(), -1);
        assert_eq!(reader.se(), 2);
        assert_eq!(reader.se(), -2);
        assert_eq!(reader.se(), 3);
        assert_eq!(reader.se(), -3);
    }

    #[test]
    fn the_inserted_byte_comes_back_out() {
        assert_eq!(unescape(&[0, 0, 3, 1, 0, 0, 3, 2], 64), [0, 0, 1, 0, 0, 2]);
        assert_eq!(unescape(&[0, 3, 3], 64), [0, 3, 3]);
        assert_eq!(unescape(&[0, 0, 3, 3], 64), [0, 0, 3]);
    }

    #[test]
    fn unescaping_stops_where_it_was_told_to() {
        assert_eq!(unescape(&[1, 2, 3, 4, 5], 3), [1, 2, 3]);
    }
}
