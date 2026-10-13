// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Annex B の可変長符号のテーブルを、九ビットと七ビットの二段のルックアップテーブルにする。

use super::tables;
use crate::video::bits::Reader;

/// Bits indexing the first level; short codes resolve in one lookup.
const ROOT: u32 = 9;

pub const END_OF_BLOCK: u16 = 0xffff;
/// Escape: run and level follow in plain bits.
pub const ESCAPE: u16 = 0xfffe;

#[derive(Debug)]
pub struct Table {
    /// Indexed by the next [`ROOT`] bits; zero where no code begins that way.
    root: Box<[u32]>,
    subs: Box<[u32]>,
}

fn leaf(length: u8, value: u16) -> u32 {
    u32::from(length) << 16 | u32::from(value)
}

fn branch(at: usize, bits: u32) -> u32 {
    1 << 31 | bits << 24 | at as u32
}

impl Table {
    /// Builds the table for `(code, length, value)` rows, which must form a prefix code.
    pub fn new(rows: &[(u16, u8, u16)]) -> Self {
        let longest = rows
            .iter()
            .map(|&(_, length, _)| u32::from(length))
            .max()
            .unwrap_or(ROOT);
        let mut root = vec![0u32; 1 << ROOT];
        let mut subs = Vec::new();

        for &(code, length, value) in rows {
            if u32::from(length) <= ROOT {
                let spare = ROOT - u32::from(length);
                let first = usize::from(code) << spare;
                for entry in &mut root[first..first + (1 << spare)] {
                    *entry = leaf(length, value);
                }
            }
        }

        let deep = longest.saturating_sub(ROOT);
        for &(code, length, value) in rows {
            if u32::from(length) <= ROOT {
                continue;
            }
            let prefix = usize::from(code) >> (u32::from(length) - ROOT);
            if root[prefix] == 0 {
                root[prefix] = branch(subs.len(), deep);
                subs.resize(subs.len() + (1 << deep), 0);
            }
            let at = (root[prefix] & 0x00ff_ffff) as usize;
            let rest = u32::from(length) - ROOT;
            let spare = deep - rest;
            let first = at + ((usize::from(code) & ((1 << rest) - 1)) << spare);
            for entry in &mut subs[first..first + (1 << spare)] {
                *entry = leaf(length, value);
            }
        }

        Self {
            root: root.into_boxed_slice(),
            subs: subs.into_boxed_slice(),
        }
    }

    /// Builds a coefficient table, with run and level packed into one value.
    pub fn coefficients(rows: &[(u16, u8, u8, u8)], end: (u16, u8)) -> Self {
        let mut all: Vec<(u16, u8, u16)> = rows
            .iter()
            .map(|&(code, length, run, level)| (code, length, packed(run, level)))
            .collect();
        all.push((end.0, end.1, END_OF_BLOCK));
        all.push((
            tables::COEFFICIENT_ESCAPE.0,
            tables::COEFFICIENT_ESCAPE.1,
            ESCAPE,
        ));
        Self::new(&all)
    }

    /// Reads one code, or `None` (consuming nothing) if the bits are no code.
    pub fn read(&self, reader: &mut Reader<'_>) -> Option<u16> {
        let entry = self.root[reader.peek(ROOT) as usize];
        if entry == 0 {
            return None;
        }
        if entry >> 31 == 0 {
            reader.skip((entry >> 16) & 0x1f);
            return Some(entry as u16);
        }
        let deep = entry >> 24 & 0x1f;
        let at = (entry & 0x00ff_ffff) as usize;
        let index = reader.peek(ROOT + deep) & ((1 << deep) - 1);
        let entry = self.subs[at + index as usize];
        if entry == 0 {
            return None;
        }
        reader.skip((entry >> 16) & 0x1f);
        Some(entry as u16)
    }
}

/// Run and level as one value; level is never zero, so zero means nothing.
pub fn packed(run: u8, level: u8) -> u16 {
    u16::from(run) << 6 | u16::from(level)
}

pub fn run(packed: u16) -> u32 {
    u32::from(packed >> 6)
}

pub fn level(packed: u16) -> i32 {
    i32::from(packed & 63)
}

#[derive(Debug)]
pub struct Tables {
    pub address_increment: Table,
    pub coded_block_pattern: Table,
    pub motion_code: Table,
    pub dc_size_luma: Table,
    pub dc_size_chroma: Table,
    pub macroblock_type_i: Table,
    pub macroblock_type_p: Table,
    pub macroblock_type_b: Table,
    pub coefficients_zero: Table,
    pub coefficients_one: Table,
}

pub fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| Tables {
        address_increment: Table::new(tables::ADDRESS_INCREMENT),
        coded_block_pattern: Table::new(tables::CODED_BLOCK_PATTERN),
        motion_code: Table::new(tables::MOTION_CODE),
        dc_size_luma: Table::new(tables::DC_SIZE_LUMA),
        dc_size_chroma: Table::new(tables::DC_SIZE_CHROMA),
        macroblock_type_i: Table::new(tables::MACROBLOCK_TYPE_I),
        macroblock_type_p: Table::new(tables::MACROBLOCK_TYPE_P),
        macroblock_type_b: Table::new(tables::MACROBLOCK_TYPE_B),
        coefficients_zero: Table::coefficients(
            tables::COEFFICIENTS_ZERO,
            tables::END_OF_BLOCK_ZERO,
        ),
        coefficients_one: Table::coefficients(tables::COEFFICIENTS_ONE, tables::END_OF_BLOCK_ONE),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::bits::Reader;

    fn stream(codes: &[(u16, u8)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut at = 0;
        for &(code, length) in codes {
            for shift in (0..length).rev() {
                if at % 8 == 0 {
                    bytes.push(0);
                }
                let bit = code >> shift & 1;
                if bit == 1 {
                    let last = bytes.len() - 1;
                    bytes[last] |= 1 << (7 - at % 8);
                }
                at += 1;
            }
        }
        bytes
    }

    #[test]
    fn every_code_of_every_table_reads_back_as_itself() {
        for rows in [
            tables::ADDRESS_INCREMENT,
            tables::CODED_BLOCK_PATTERN,
            tables::MOTION_CODE,
            tables::DC_SIZE_LUMA,
            tables::DC_SIZE_CHROMA,
            tables::MACROBLOCK_TYPE_I,
            tables::MACROBLOCK_TYPE_P,
            tables::MACROBLOCK_TYPE_B,
        ] {
            let table = Table::new(rows);
            for &(code, length, value) in rows {
                // Trailing ones check the table stops at the end of the code.
                let bytes = stream(&[(code, length), (0xffff, 16)]);
                let mut reader = Reader::new(&bytes);
                assert_eq!(table.read(&mut reader), Some(value));
                assert_eq!(reader.at(), usize::from(length));
            }
        }
    }

    #[test]
    fn every_coefficient_of_both_tables_reads_back_as_itself() {
        for (rows, end) in [
            (tables::COEFFICIENTS_ZERO, tables::END_OF_BLOCK_ZERO),
            (tables::COEFFICIENTS_ONE, tables::END_OF_BLOCK_ONE),
        ] {
            let table = Table::coefficients(rows, end);
            for &(code, length, run, level) in rows {
                let bytes = stream(&[(code, length), (0, 16)]);
                let mut reader = Reader::new(&bytes);
                let value = table
                    .read(&mut reader)
                    .expect("a code that is in the table");
                assert_eq!(
                    (super::run(value), super::level(value)),
                    (u32::from(run), i32::from(level))
                );
                assert_eq!(reader.at(), usize::from(length));
            }
            let bytes = stream(&[end, (0, 16)]);
            assert_eq!(table.read(&mut Reader::new(&bytes)), Some(END_OF_BLOCK));
            let bytes = stream(&[tables::COEFFICIENT_ESCAPE, (0, 16)]);
            assert_eq!(table.read(&mut Reader::new(&bytes)), Some(ESCAPE));
        }
    }

    #[test]
    fn bits_that_are_not_a_code_give_nothing_and_consume_nothing() {
        // No code in Table B.1 begins with twelve zeroes.
        let table = Table::new(tables::ADDRESS_INCREMENT);
        let mut reader = Reader::new(&[0, 0, 0]);
        assert_eq!(table.read(&mut reader), None);
        assert_eq!(reader.at(), 0);
    }
}
