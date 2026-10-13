// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! コンテキストの初期化（clause 9.3.1.1）。算術は [`crate::video::cabac`] と共通。

use super::tables::CABAC_INIT;
use crate::video::cabac;

/// Covers 4:2:0 frame and field with the 8x8 transform; the rest are for 4:2:2 and 4:4:4.
pub const CONTEXTS: usize = 460;

pub type Cabac<'a> = cabac::Cabac<'a, CONTEXTS>;

/// Clause 9.3.1.1. `model` is 0 for an intra slice, else `cabac_init_idc + 1`.
pub fn initial(model: usize, qp: i32) -> [u8; CONTEXTS] {
    let qp = qp.clamp(0, 51);
    let mut state = [0; CONTEXTS];
    for (state, init) in state.iter_mut().zip(&CABAC_INIT) {
        let [m, n] = init[model.min(3)];
        let pre = (((i32::from(m) * qp) >> 4) + i32::from(n)).clamp(1, 126);
        *state = cabac::packed(pre);
    }
    state
}

impl<'a> Cabac<'a> {
    pub fn new(bytes: &'a [u8], model: usize, qp: i32) -> Self {
        Self::over(bytes, initial(model, qp))
    }
}

#[cfg(test)]
mod tests {
    use super::{CONTEXTS, Cabac, initial};
    use crate::video::cabac;

    #[test]
    fn what_the_coder_of_the_standard_writes_is_what_this_reads_back() {
        for qp in [0, 12, 26, 40, 51] {
            for model in 0..4 {
                cabac::round_trip(initial(model, qp), 300);
            }
        }
    }

    #[test]
    fn the_initialisation_is_the_formula_of_clause_9_3_1_1() {
        let cabac = Cabac::new(&[0xff; 8], 0, 26);
        let [m, n] = super::CABAC_INIT[70][0];
        let pre = (((i32::from(m) * 26) >> 4) + i32::from(n)).clamp(1, 126);
        assert_eq!(cabac.contexts()[70], cabac::packed(pre));
    }

    #[test]
    fn a_context_a_slice_cannot_reach_still_lands_on_a_state_that_behaves() {
        for model in 0..4 {
            assert!(
                initial(model, 26).iter().all(|&state| state >> 1 <= 63),
                "model {model}"
            );
        }
        assert_eq!(initial(0, 26).len(), CONTEXTS);
    }

    #[test]
    fn a_slice_that_runs_out_says_so_rather_than_reading_on() {
        let mut cabac = Cabac::new(&[0x00], 0, 26);
        for _ in 0..64 {
            cabac.decision(11);
        }
        assert!(cabac.overrun());
    }
}
