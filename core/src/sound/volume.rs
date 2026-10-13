// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 音量（0〜100）をサンプルに掛ける倍率にする。曲線は三乗。

pub const MAX: u32 = 100;

pub fn gain(volume: u32, muted: bool) -> f32 {
    if muted {
        return 0.0;
    }
    let v = volume.min(MAX) as f32 / MAX as f32;
    v * v * v
}

pub fn stepped(volume: u32, by: i32) -> u32 {
    (volume as i32 + by).clamp(0, MAX as i32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_is_unity_and_nought_is_silence() {
        assert_eq!(gain(100, false), 1.0);
        assert_eq!(gain(0, false), 0.0);
        assert_eq!(gain(150, false), 1.0);
    }

    #[test]
    fn half_way_is_an_eighth_which_is_the_cube() {
        assert!((gain(50, false) - 0.125).abs() < 1e-6);
    }

    #[test]
    fn muted_is_silence_whatever_the_volume() {
        assert_eq!(gain(80, true), 0.0);
    }

    #[test]
    fn stepping_stays_inside_the_range() {
        assert_eq!(stepped(98, 5), 100);
        assert_eq!(stepped(3, -5), 0);
        assert_eq!(stepped(50, 5), 55);
    }
}
