// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::{Class, Sbr, Spectrum, pointer_bits};
use crate::bits::Reader;

/// The tables a real one-segment broadcast asks for (checked against FFmpeg).
#[test]
fn the_band_tables_of_a_one_segment_broadcast() {
    let mut sbr = Sbr::new();
    sbr.rate = 48000;
    sbr.limiter_bands = 2;
    sbr.spectrum = Spectrum {
        start_freq: 12,
        stop_freq: 9,
        crossover: 0,
        freq_scale: 2,
        alter_scale: true,
        noise_bands: 2,
    };
    sbr.rebuild().expect("the tables of a stream that decodes");
    assert_eq!(sbr.k, [22, 45, 45]);
    assert_eq!(sbr.kx[1], 22);
    assert_eq!(sbr.m[1], 23);
    assert_eq!(sbr.master, 10);
    assert_eq!(sbr.n, [5, 10]);
    assert_eq!(
        &sbr.f_high[..=sbr.n[1]],
        &[22, 23, 25, 27, 29, 31, 33, 36, 39, 42, 45]
    );
    assert_eq!(&sbr.f_low[..=sbr.n[0]], &[22, 25, 29, 33, 39, 45]);
    assert_eq!(sbr.n_noise, 2);
    assert_eq!(&sbr.f_noise[..=sbr.n_noise], &[22, 29, 45]);
    assert_eq!(sbr.patches, 2);
    assert_eq!(&sbr.patch_bands[..2], &[20, 3]);
    assert_eq!(&sbr.patch_start[..2], &[2, 18]);
    assert_eq!(sbr.n_limit, 3);
    assert_eq!(&sbr.f_limit[..=sbr.n_limit], &[22, 29, 42, 45]);
}

#[test]
fn a_rate_the_tables_do_not_cover_is_refused() {
    let mut sbr = Sbr::new();
    sbr.rate = 12345;
    sbr.spectrum = Spectrum {
        start_freq: 0,
        stop_freq: 0,
        crossover: 0,
        freq_scale: 2,
        alter_scale: true,
        noise_bands: 2,
    };
    assert!(sbr.rebuild().is_err());
}

#[test]
fn the_noise_border_pointer_is_as_wide_as_the_envelope_count_needs() {
    assert_eq!(pointer_bits(1), 1);
    assert_eq!(pointer_bits(2), 2);
    assert_eq!(pointer_bits(3), 2);
    assert_eq!(pointer_bits(4), 3);
    assert_eq!(pointer_bits(5), 3);
}

/// A payload that is not one leaves the decoder upsampling (what follows a loss is foreign bits).
#[test]
fn rubbish_in_a_fill_element_turns_the_replication_off_rather_than_reading_it() {
    let mut state = 987_654_321u32;
    let mut sbr = Sbr::new();
    for round in 0..300 {
        let length = 1 + (round * 13) % 200;
        let bytes: Vec<u8> = (0..length)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 16) as u8
            })
            .collect();
        let mut reader = Reader::new(&bytes);
        let _ = sbr.payload(&mut reader, bytes.len() * 8, false, true, 24000);
        assert_eq!(sbr.rate(), 48000);
        let mut channels = vec![vec![0.0f32; 1024], vec![0.0f32; 1024]];
        sbr.apply(&mut channels);
        assert_eq!(channels[0].len(), 2048);
        assert!(channels[0].iter().all(|sample| sample.is_finite()));
    }
}

#[test]
fn only_the_classes_with_a_variable_end_read_the_pointer_that_way() {
    assert!(!Class::of_index(0).trailing());
    assert!(Class::of_index(1).trailing());
    assert!(!Class::of_index(2).trailing());
    assert!(Class::of_index(3).trailing());
}

#[test]
fn a_replication_that_has_seen_nothing_still_doubles_the_rate() {
    let mut sbr = Sbr::new();
    sbr.enable(24000);
    assert_eq!(sbr.rate(), 48000);
    assert!(!sbr.parametric_stereo());
    let mut channels = vec![vec![0.0f32; 1024]];
    sbr.apply(&mut channels);
    assert_eq!(channels[0].len(), 2048);
    assert!(channels[0].iter().all(|sample| *sample == 0.0));
}
