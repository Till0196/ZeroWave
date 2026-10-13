// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! コンテキストの初期化（clause 9.3.2.2）。算術は [`crate::video::cabac`] にある。
//! H.264 と違い、傾きと切片をテーブルの一バイトから導く。

use super::contexts::{CONTEXTS, INIT};
use crate::video::cabac;

pub type Cabac<'a> = cabac::Cabac<'a, CONTEXTS>;

/// Clause 9.3.2.2: the state every context starts a slice segment in.
pub fn initial(init_type: usize, qp: i32) -> [u8; CONTEXTS] {
    let qp = qp.clamp(0, 51);
    let mut state = [0; CONTEXTS];
    for (state, &init) in state.iter_mut().zip(&INIT[init_type.min(2)]) {
        let slope = (i32::from(init) >> 4) * 5 - 45;
        let offset = ((i32::from(init) & 15) << 3) - 16;
        let pre = (((slope * qp) >> 4) + offset).clamp(1, 126);
        *state = cabac::packed(pre);
    }
    state
}

impl<'a> Cabac<'a> {
    /// Starts at byte-aligned `bytes` with every context initialised for this type and quantiser.
    pub fn new(bytes: &'a [u8], init_type: usize, qp: i32) -> Self {
        Self::over(bytes, initial(init_type, qp))
    }
}

#[cfg(test)]
mod tests {
    use super::{Cabac, initial};
    use crate::video::cabac;

    #[test]
    fn what_the_coder_of_the_standard_writes_is_what_this_reads_back() {
        for qp in [0, 12, 26, 40, 51] {
            for init_type in 0..3 {
                cabac::round_trip(initial(init_type, qp), 400);
            }
        }
    }

    #[test]
    fn the_initialisation_is_the_formula_of_clause_9_3_2_2() {
        for &qp in &[0, 26, 51] {
            for init_type in 0..3 {
                let state = initial(init_type, qp);
                for (context, &init) in super::INIT[init_type].iter().enumerate() {
                    let slope = (i32::from(init) >> 4) * 5 - 45;
                    let offset = ((i32::from(init) & 15) << 3) - 16;
                    let pre = (((slope * qp) >> 4) + offset).clamp(1, 126);
                    assert_eq!(
                        state[context],
                        cabac::packed(pre),
                        "context {context}, type {init_type}, qp {qp}"
                    );
                }
            }
        }
    }

    /// 154 has to land on an even probability so a context reached by mistake does not panic.
    #[test]
    fn the_value_that_stands_for_a_blank_is_the_middle_of_the_range() {
        let slope = (154 >> 4) * 5 - 45;
        let offset = ((154 & 15) << 3) - 16;
        let pre: i32 = ((slope * 26) >> 4) + offset;
        assert_eq!(pre, 64);
        assert!(initial(0, 26).iter().all(|&state| state >> 1 <= 63));
    }

    #[test]
    fn a_slice_that_runs_out_says_so_rather_than_reading_on() {
        let mut cabac = Cabac::new(&[0x00], 0, 26);
        for _ in 0..64 {
            cabac.decision(11);
        }
        assert!(cabac.overrun());
    }

    #[test]
    fn a_substream_starts_again_over_contexts_it_is_handed() {
        let mut first = cabac::Coder::new(initial(0, 26));
        for at in 0..40 {
            first.decision(at % 30, (at % 3 == 0) as u32);
        }
        let saved = first.state;
        let bytes = first.finish();
        let mut second = cabac::Coder::new(saved);
        for at in 0..40 {
            second.decision(at % 30, (at % 5 == 0) as u32);
        }
        let mut stream = bytes.clone();
        let at = stream.len();
        stream.extend(second.finish());

        let mut cabac = Cabac::new(&stream, 0, 26);
        for at in 0..40 {
            assert_eq!(cabac.decision(at % 30), (at % 3 == 0) as u32);
        }
        cabac.restart(at, saved);
        for at in 0..40 {
            assert_eq!(cabac.decision(at % 30), (at % 5 == 0) as u32, "second {at}");
        }
    }
}
