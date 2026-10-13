// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Two-level (8 + 8 bit) lookup tables for the variable length codes of clause 9.2.

use super::tables::{self, Row};
use crate::video::bits::Reader;

/// First-level index width; every short code fits, so the common case is one lookup.
const ROOT: u32 = 8;

#[derive(Debug)]
pub struct Table {
    root: Box<[u32]>,
    subs: Box<[u32]>,
}

/// The length is never 0, so 0 means no code.
fn leaf(length: u8, value: u8) -> u32 {
    u32::from(length) << 16 | u32::from(value)
}

fn branch(at: usize, bits: u32) -> u32 {
    1 << 31 | bits << 24 | at as u32
}

impl Table {
    pub fn new(rows: &[Row]) -> Self {
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
            let prefix = usize::from(code >> (u32::from(length) - ROOT));
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

    /// `None` if the bits are not a code; then nothing is consumed.
    pub fn read(&self, reader: &mut Reader<'_>) -> Option<u8> {
        let entry = self.root[reader.peek(ROOT) as usize];
        if entry == 0 {
            return None;
        }
        if entry >> 31 == 0 {
            reader.skip(entry >> 16 & 0x1f);
            return Some(entry as u8);
        }
        let deep = entry >> 24 & 0x1f;
        let at = (entry & 0x00ff_ffff) as usize;
        let index = reader.peek(ROOT + deep) & ((1 << deep) - 1);
        let entry = self.subs[at + index as usize];
        if entry == 0 {
            return None;
        }
        reader.skip(entry >> 16 & 0x1f);
        Some(entry as u8)
    }
}

#[derive(Debug)]
pub struct Tables {
    /// By the column `nC` chooses.
    pub coeff_token: [Table; 4],
    pub coeff_token_chroma_dc: Table,
    pub total_zeros: [Table; 15],
    pub total_zeros_chroma_dc: [Table; 3],
    /// By the number of zeroes left, less one, up to seven.
    pub run_before: [Table; 7],
}

pub fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| Tables {
        coeff_token: std::array::from_fn(|at| Table::new(tables::COEFF_TOKEN[at])),
        coeff_token_chroma_dc: Table::new(tables::COEFF_TOKEN_CHROMA_DC),
        total_zeros: std::array::from_fn(|at| Table::new(tables::TOTAL_ZEROS[at])),
        total_zeros_chroma_dc: std::array::from_fn(|at| {
            Table::new(tables::TOTAL_ZEROS_CHROMA_DC[at])
        }),
        run_before: std::array::from_fn(|at| Table::new(tables::RUN_BEFORE[at])),
    })
}

#[cfg(test)]
mod tests {
    use super::{Table, tables};
    use crate::video::bits::Reader;
    use crate::video::h264::tables::Row;

    fn stream(code: u16, length: u8, pad: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (at, shift) in (0..length).rev().enumerate() {
            if at % 8 == 0 {
                bytes.push(0);
            }
            if code >> shift & 1 == 1 {
                let last = bytes.len() - 1;
                bytes[last] |= 1 << (7 - at % 8);
            }
        }
        bytes.extend_from_slice(&[pad; 4]);
        bytes
    }

    fn every_table() -> Vec<&'static [Row]> {
        let mut all: Vec<&[Row]> = tables::COEFF_TOKEN.to_vec();
        all.push(tables::COEFF_TOKEN_CHROMA_DC);
        all.extend_from_slice(&tables::TOTAL_ZEROS);
        all.extend_from_slice(&tables::TOTAL_ZEROS_CHROMA_DC);
        all.extend_from_slice(&tables::RUN_BEFORE);
        all
    }

    #[test]
    fn every_code_of_every_table_reads_back_as_itself() {
        for rows in every_table() {
            let table = Table::new(rows);
            for &(code, length, value) in rows {
                for pad in [0x00, 0xff] {
                    let bytes = stream(code, length, pad);
                    let mut reader = Reader::new(&bytes);
                    assert_eq!(table.read(&mut reader), Some(value));
                    assert_eq!(reader.at(), usize::from(length));
                }
            }
        }
    }

    #[test]
    fn no_code_of_any_table_is_the_start_of_another() {
        for rows in every_table() {
            for &(code, length, _) in rows {
                for &(other, other_length, _) in rows {
                    if other_length <= length || (code, length) == (other, other_length) {
                        continue;
                    }
                    assert_ne!(
                        other >> (other_length - length),
                        code,
                        "{code:b} of {length} bits begins {other:b} of {other_length}"
                    );
                }
            }
        }
    }

    #[test]
    fn bits_that_are_not_a_code_give_nothing_and_consume_nothing() {
        let table = Table::new(tables::COEFF_TOKEN[0]);
        let mut reader = Reader::new(&[0, 0, 0]);
        assert_eq!(table.read(&mut reader), None);
        assert_eq!(reader.at(), 0);
    }
}
