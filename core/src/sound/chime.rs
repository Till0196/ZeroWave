// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 受信機の内蔵音（TR-B14 第二分冊 第三編 第 2 部 3.3.5）。

use std::f32::consts::PI;

pub fn synthesize(number: u32, rate: u32) -> Option<Vec<f32>> {
    let rate = rate.max(8000) as usize;
    Some(match number {
        0..=4 => chime(rate, number),
        13 => alert(rate),
        5 => select(rate),
        6 => repeat(&select(rate), 3, -2000),
        7 => decide(rate),
        8 => repeat(&decide(rate), 3, -2000),
        9 => tick(rate),
        10 => repeat(&tick(rate), 3, 0),
        11 => back(rate),
        12 => repeat(&back(rate), 3, -2000),
        _ => return None,
    })
}

fn sine(rate: usize, at: usize, hz: f32) -> f32 {
    (at as f32 * hz * 2.0 * PI / rate as f32).sin()
}

fn chime(rate: usize, number: u32) -> Vec<f32> {
    let shift = 1.0 + number as f32 * 0.06;
    let (high, low) = (1318.5 * shift, 1046.5 * shift);
    let first = rate * 35 / 100;
    let second = rate * 90 / 100;
    let volume = 0.12;
    let bell = |i: usize, hz: f32, len: usize| -> f32 {
        let t = i as f32 / len as f32;
        let envelope = (1.0 - t).powi(2);
        (sine(rate, i, hz) + sine(rate, i, hz * 2.0) * 0.3 + sine(rate, i, hz * 3.0) * 0.1)
            * envelope
    };
    let mut out = vec![0.0; first + second];
    for (i, sample) in out.iter_mut().enumerate() {
        if i < first {
            *sample += bell(i, high, first) * volume;
        }
        if i >= first * 7 / 10 {
            let j = i - first * 7 / 10;
            if j < second {
                *sample += bell(j, low, second) * volume;
            }
        }
    }
    out
}

fn alert(rate: usize) -> Vec<f32> {
    let len = rate * 15 / 100;
    let volume = 0.12;
    let one: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / len as f32;
            let envelope = if t < 0.1 {
                t / 0.1
            } else {
                1.0 - (t - 0.1) / 0.9
            };
            (sine(rate, i, 880.0) + sine(rate, i, 1760.0) * 0.5) * volume * envelope
        })
        .collect();
    repeat(&one, 3, (rate / 10) as isize)
}

fn select(rate: usize) -> Vec<f32> {
    let len = rate / 5;
    let volume = 0.1;
    (0..len)
        .map(|i| {
            let envelope = 1.0 - i as f32 / len as f32;
            (sine(rate, i, 4680.0)
                + sine(rate, i, 4160.0)
                + sine(rate, i, 3640.0) * 0.25
                + sine(rate, i, 520.0))
                * volume
                * envelope
        })
        .collect()
}

fn decide(rate: usize) -> Vec<f32> {
    two_step(rate, 2100.0, 1400.0, 4200.0, 0.6)
}

fn tick(rate: usize) -> Vec<f32> {
    let len = rate * 9 / 100;
    let volume = 0.1;
    (0..len)
        .map(|i| {
            let envelope = 1.0 - i as f32 / len as f32;
            (sine(rate, i, 5200.0) + sine(rate, i, 3120.0) + sine(rate, i, 1040.0))
                * volume
                * envelope
        })
        .collect()
}

/// 規格は番号だけで音は受信機ごと（TR-B14 表 3-9）。
fn back(rate: usize) -> Vec<f32> {
    two_step(rate, 1400.0, 933.0, 2800.0, 0.4)
}

fn two_step(rate: usize, first: f32, second: f32, overtone: f32, weight: f32) -> Vec<f32> {
    let len = rate / 5;
    let volume = 0.1;
    (0..len)
        .map(|i| {
            let mut sample = 0.0;
            if i < rate / 20 {
                sample += sine(rate, i, first) * volume;
            }
            if i >= rate * 4 / 100 {
                sample +=
                    sine(rate, i, second) * volume + sine(rate, i, overtone) * volume * weight;
            }
            sample
        })
        .collect()
}

fn repeat(one: &[f32], times: usize, gap: isize) -> Vec<f32> {
    let step = (one.len() as isize + gap).max(1) as usize;
    let mut out = vec![0.0; step * (times - 1) + one.len()];
    for n in 0..times {
        for (i, sample) in one.iter().enumerate() {
            out[n * step + i] += sample;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_sounds_are_short_and_quiet() {
        for number in 5..=12 {
            let samples = synthesize(number, 48000).expect("a button sound");
            assert!(samples.len() <= 48000, "{number} は一秒に収まる");
            assert!(samples.iter().all(|s| s.abs() <= 1.0));
            assert!(samples.iter().any(|s| s.abs() > 0.01));
        }
        assert!(synthesize(14, 48000).is_none());
    }

    #[test]
    fn the_chimes_and_the_alert_are_there_and_differ() {
        let mut heard = Vec::new();
        for number in [0, 1, 2, 3, 4, 13] {
            let samples = synthesize(number, 48000).expect("a chime");
            assert!(samples.len() <= 96000, "{number} は二秒に収まる");
            assert!(samples.iter().all(|s| s.abs() <= 1.0), "{number}");
            assert!(samples.iter().any(|s| s.abs() > 0.05), "{number}");
            heard.push(samples);
        }
        assert_ne!(heard[0], heard[1]);
        assert_ne!(heard[0], heard[5]);
    }
}
