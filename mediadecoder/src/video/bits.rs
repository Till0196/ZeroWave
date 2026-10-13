// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Bit reader for slices (MPEG-2 and H.264): peek without consuming, consume without looking.
//! Past the end reads as zeroes and [`Reader::overrun`] reports it.

#[derive(Debug, Clone)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    #[cfg(test)]
    pub fn at(&self) -> usize {
        self.at
    }

    pub fn overrun(&self) -> bool {
        self.at > self.bytes.len() * 8
    }

    /// The next `count` bits (at most 32) without consuming them.
    pub fn peek(&self, count: u32) -> u32 {
        debug_assert!(count <= 32);
        let byte = self.at / 8;
        let mut window = 0u64;
        for offset in 0..5 {
            window = window << 8 | u64::from(self.bytes.get(byte + offset).copied().unwrap_or(0));
        }
        if count == 0 {
            return 0;
        }
        let shift = 40 - (self.at % 8) - count as usize;
        ((window >> shift) & (u64::MAX >> (64 - count))) as u32
    }

    pub fn skip(&mut self, count: u32) {
        self.at += count as usize;
    }

    pub fn bits(&mut self, count: u32) -> u32 {
        let value = self.peek(count);
        self.skip(count);
        value
    }

    pub fn bit(&mut self) -> u32 {
        self.bits(1)
    }

    pub fn signed(&mut self, count: u32) -> i32 {
        let value = self.bits(count) as i32;
        let sign = 1 << (count - 1);
        (value ^ sign) - sign
    }

    /// Zeroes before the next one. Stops at 32 so a damaged slice cannot hang.
    pub fn leading_zeroes(&mut self) -> u32 {
        let window = self.peek(32);
        if window != 0 {
            let zeroes = window.leading_zeros();
            self.skip(zeroes + 1);
            return zeroes;
        }
        self.skip(32);
        32
    }

    pub fn ue(&mut self) -> u32 {
        let zeroes = self.leading_zeroes();
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

    /// Truncated code (`te(v)`): one inverted bit when the range is 1, else `ue`.
    pub fn te(&mut self, range: u32) -> u32 {
        if range == 1 {
            1 - self.bit()
        } else {
            self.ue()
        }
    }

    /// `more_rbsp_data`.
    pub fn more_data(&self) -> bool {
        let total = self.bytes.len() * 8;
        if self.at >= total {
            return false;
        }
        let mut last = total;
        while last > self.at {
            last -= 1;
            if self.bytes[last / 8] >> (7 - last % 8) & 1 == 1 {
                return last > self.at;
            }
        }
        false
    }

    pub fn align(&mut self) {
        self.at = self.at.div_ceil(8) * 8;
    }

    /// The rest from the current position, which must be byte-aligned.
    pub fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.at / 8..).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::Reader;

    #[test]
    fn bits_come_out_most_significant_first() {
        let mut reader = Reader::new(&[0b1011_0010, 0x0f, 0xff]);
        assert_eq!(reader.peek(4), 0b1011);
        assert_eq!(reader.bits(4), 0b1011);
        assert_eq!(reader.bits(8), 0b0010_0000);
        assert_eq!(reader.bits(12), 0xfff);
        assert_eq!(reader.at(), 24);
    }

    #[test]
    fn a_window_of_thirty_two_bits_comes_out_whole_however_it_straddles() {
        let bytes = [0xff, 0x00, 0xff, 0x00, 0xff, 0x00];
        let mut reader = Reader::new(&bytes);
        reader.skip(7);
        assert_eq!(reader.peek(32), 0b1000_0000_0111_1111_1000_0000_0111_1111);
    }

    #[test]
    fn golomb_codes_come_out_as_the_standard_tabulates_them() {
        let mut reader = Reader::new(&[0b1010_0110, 0b0100_0010, 0b1001_1000, 0b1110_0000]);
        for wanted in 0..7 {
            assert_eq!(reader.ue(), wanted);
        }
        let mut reader = Reader::new(&[0b1010_0110, 0b0100_0010, 0b1001_1000, 0b1110_0000]);
        for wanted in [0, 1, -1, 2, -2, 3, -3] {
            assert_eq!(reader.se(), wanted);
        }
    }

    #[test]
    fn a_run_of_zeroes_longer_than_a_word_stops_rather_than_running_on() {
        let mut reader = Reader::new(&[0; 64]);
        assert_eq!(reader.leading_zeroes(), 32);
        assert_eq!(reader.ue(), 0);
    }

    #[test]
    fn the_stop_bit_and_its_padding_are_not_more_data() {
        let mut reader = Reader::new(&[0b1010_1000]);
        assert!(reader.more_data());
        reader.skip(4);
        assert!(!reader.more_data());
        reader.skip(1);
        assert!(!reader.more_data());
    }

    #[test]
    fn a_signed_field_comes_out_as_two_s_complement() {
        let mut reader = Reader::new(&[0b1000_0000, 0b0111_1111, 0xff]);
        assert_eq!(reader.signed(8), -128);
        assert_eq!(reader.signed(8), 127);
        assert_eq!(reader.signed(4), -1);
    }

    #[test]
    fn reading_past_the_end_gives_zeroes_and_says_so() {
        let mut reader = Reader::new(&[0xff]);
        assert_eq!(reader.bits(8), 0xff);
        assert!(!reader.overrun());
        assert_eq!(reader.bits(8), 0);
        assert!(reader.overrun());
    }
}
