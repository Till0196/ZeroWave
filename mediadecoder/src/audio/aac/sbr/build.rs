// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The frequency tables a header builds.

use super::*;

impl Sbr {
    /// Builds the master frequency table and everything derived (only when the spectrum changed).
    pub(super) fn rebuild(&mut self) -> Result<(), ()> {
        self.master_table()?;
        self.derived_tables()?;
        for channel in &mut self.channels {
            channel.noise_at = 0;
        }
        Ok(())
    }

    /// The master frequency band table, 14496-3 clause 4.6.18.3.2.1.
    pub(super) fn master_table(&mut self) -> Result<(), ()> {
        let rate = self.rate;
        let offsets = match rate {
            16000 => &tables::OFFSETS[0],
            22050 => &tables::OFFSETS[1],
            24000 => &tables::OFFSETS[2],
            32000 => &tables::OFFSETS[3],
            44100 | 48000 | 64000 => &tables::OFFSETS[4],
            88200 | 96000 | 128000 | 176400 | 192000 => &tables::OFFSETS[5],
            _ => return Err(()),
        };
        let corner: u32 = if rate < 32000 {
            3000
        } else if rate < 64000 {
            4000
        } else {
            5000
        };
        let start_min = ((corner << 7) + rate / 2) / rate;
        let stop_min = ((corner << 8) + rate / 2) / rate;
        let spectrum = self.spectrum;
        let start =
            i64::from(start_min) + i64::from(offsets[usize::from(spectrum.start_freq).min(15)]);
        if start <= 0 {
            return Err(());
        }
        self.k[0] = start as usize;
        self.k[2] = match spectrum.stop_freq {
            0..=13 => {
                let mut widths = [0i32; 13];
                make_bands(&mut widths, stop_min as usize, 64, 13);
                widths.sort_unstable();
                let mut stop = stop_min as i64;
                for width in widths.iter().take(usize::from(spectrum.stop_freq)) {
                    stop += i64::from(*width);
                }
                stop.clamp(0, 64) as usize
            }
            14 => (2 * self.k[0]).min(64),
            15 => (3 * self.k[0]).min(64),
            _ => return Err(()),
        };
        if self.k[2] <= self.k[0] {
            return Err(());
        }
        let most = if rate <= 32000 {
            48
        } else if rate == 44100 {
            35
        } else {
            32
        };
        if self.k[2] - self.k[0] > most {
            return Err(());
        }

        if spectrum.freq_scale == 0 {
            // Unwarped: bands of one or two, the odd one taken off the bottom or added to the top.
            let step = usize::from(spectrum.alter_scale) + 1;
            let master = ((self.k[2] - self.k[0] + (step & 2)) >> step) << 1;
            if master == 0 || master > 48 || spectrum.crossover >= master {
                return Err(());
            }
            self.master = master;
            for at in 1..=master {
                self.f_master[at] = step as u16;
            }
            let difference = (self.k[2] - self.k[0]) as i64 - (master * step) as i64;
            if difference < 0 {
                self.f_master[1] = self.f_master[1].saturating_sub(1);
                if difference < -1 {
                    self.f_master[2] = self.f_master[2].saturating_sub(1);
                }
            } else if difference > 0 {
                self.f_master[master] += 1;
            }
            self.f_master[0] = self.k[0] as u16;
            for at in 1..=master {
                self.f_master[at] += self.f_master[at - 1];
            }
        } else {
            let half = 7 - i32::from(spectrum.freq_scale);
            // Two regions where the replication spans more than about an octave.
            let two = 49 * self.k[2] > 110 * self.k[0];
            self.k[1] = if two { 2 * self.k[0] } else { self.k[2] };
            let lower = ((half as f32) * (self.k[1] as f32 / self.k[0] as f32).log2())
                .round_ties_even() as i32
                * 2;
            if lower <= 0 || lower > 48 {
                return Err(());
            }
            let lower = lower as usize;
            let mut low = [0i32; 49];
            make_bands(&mut low[1..], self.k[0], self.k[1], lower);
            low[1..=lower].sort_unstable();
            let widest = low[lower];
            low[0] = self.k[0] as i32;
            for at in 1..=lower {
                if low[at] <= 0 {
                    return Err(());
                }
                low[at] += low[at - 1];
            }
            if two {
                let warp = if spectrum.alter_scale {
                    0.769_230_8f32
                } else {
                    1.0
                };
                let upper = ((half as f32) * warp * (self.k[2] as f32 / self.k[1] as f32).log2())
                    .round_ties_even() as i32
                    * 2;
                if upper <= 0 || upper > 48 || lower + upper as usize > 48 {
                    return Err(());
                }
                let upper = upper as usize;
                let mut high = [0i32; 49];
                make_bands(&mut high[1..], self.k[1], self.k[2], upper);
                let narrowest = high[1..=upper].iter().copied().min().unwrap_or(0);
                if narrowest < widest {
                    // Sorted so that a wide band does not meet a narrow one.
                    high[1..=upper].sort_unstable();
                    let change = (widest - high[1]).min((high[upper] - high[1]) >> 1);
                    high[1] += change;
                    high[upper] -= change;
                }
                high[1..=upper].sort_unstable();
                high[0] = self.k[1] as i32;
                for at in 1..=upper {
                    if high[at] <= 0 {
                        return Err(());
                    }
                    high[at] += high[at - 1];
                }
                self.master = lower + upper;
                if spectrum.crossover >= self.master {
                    return Err(());
                }
                for (slot, &value) in self.f_master[..=lower].iter_mut().zip(&low[..=lower]) {
                    *slot = value as u16;
                }
                for (slot, &value) in self.f_master[lower + 1..=lower + upper]
                    .iter_mut()
                    .zip(&high[1..=upper])
                {
                    *slot = value as u16;
                }
            } else {
                self.master = lower;
                if spectrum.crossover >= self.master {
                    return Err(());
                }
                for (slot, &value) in self.f_master[..=lower].iter_mut().zip(&low[..=lower]) {
                    *slot = value as u16;
                }
            }
        }
        Ok(())
    }

    pub(super) fn derived_tables(&mut self) -> Result<(), ()> {
        let crossover = self.spectrum.crossover;
        self.n[1] = self.master - crossover;
        self.n[0] = (self.n[1] + 1) >> 1;
        if self.n[1] > 48 {
            return Err(());
        }
        for at in 0..=self.n[1] {
            self.f_high[at] = self.f_master[crossover + at];
        }
        self.m[1] = usize::from(self.f_high[self.n[1]] - self.f_high[0]);
        self.kx[1] = usize::from(self.f_high[0]);
        if self.kx[1] + self.m[1] > 64 || self.kx[1] > 32 {
            return Err(());
        }
        self.f_low[0] = self.f_high[0];
        let odd = self.n[1] & 1;
        for at in 1..=self.n[0] {
            self.f_low[at] = self.f_high[2 * at - odd];
        }
        let noise = (f32::from(self.spectrum.noise_bands)
            * (self.k[2] as f32 / self.kx[1] as f32).log2())
        .round_ties_even() as i32;
        self.n_noise = noise.max(1) as usize;
        if self.n_noise > 5 {
            return Err(());
        }
        self.f_noise[0] = self.f_low[0];
        let mut at = 0usize;
        for band in 1..=self.n_noise {
            at += (self.n[0] - at) / (self.n_noise + 1 - band);
            self.f_noise[band] = self.f_low[at];
        }
        self.calculate_patches()?;
        self.limits();
        Ok(())
    }

    /// 14496-3 clause 4.6.18.6.3. A copy must start on a band of the same parity or it is inverted.
    pub(super) fn calculate_patches(&mut self) -> Result<(), ()> {
        self.patches = 0;
        let mut msb = self.k[0];
        let mut usb = self.kx[1];
        let goal = ((1000u64 << 11) + u64::from(self.rate) / 2) / u64::from(self.rate).max(1);
        let goal = goal as usize;
        let mut at = if goal < self.kx[1] + self.m[1] {
            let mut at = 0;
            while at < self.master && usize::from(self.f_master[at]) < goal {
                at += 1;
            }
            at
        } else {
            self.master
        };
        let (mut last_at, mut last_msb) = (usize::MAX, usize::MAX);
        let mut sb;
        loop {
            if at == last_at && msb == last_msb {
                return Err(());
            }
            last_at = at;
            last_msb = msb;
            let mut index = at;
            let mut odd;
            loop {
                sb = usize::from(self.f_master[index]);
                odd = (sb + self.k[0]) & 1;
                if index == 0 {
                    break;
                }
                index -= 1;
                if sb + odd < self.k[0] + msb {
                    break;
                }
            }
            if self.patches > 5 {
                return Err(());
            }
            let width = sb.saturating_sub(usb);
            self.patch_bands[self.patches] = width;
            self.patch_start[self.patches] = self.k[0].saturating_sub(odd + width);
            if width > 0 {
                usb = sb;
                msb = sb;
                self.patches += 1;
            } else {
                msb = self.kx[1];
            }
            if usize::from(self.f_master[at]) < sb + 3 {
                at = self.master;
            }
            if sb == self.kx[1] + self.m[1] {
                break;
            }
        }
        if self.patches > 1 && self.patch_bands[self.patches - 1] < 3 {
            self.patches -= 1;
        }
        Ok(())
    }

    /// The limiter band table, 14496-3 clause 4.6.18.3.2.3.
    pub(super) fn limits(&mut self) {
        if self.limiter_bands == 0 {
            self.f_limit[0] = self.f_low[0];
            self.f_limit[1] = self.f_low[self.n[0]];
            self.n_limit = 1;
            return;
        }
        // 2^(0.49 / bands per octave): the narrowest limiter band allowed.
        let narrowest = match self.limiter_bands {
            1 => 1.327_151_7f32,
            2 => 1.185_092_8,
            _ => 1.119_871_6,
        };
        let mut borders = [0u16; 7];
        borders[0] = self.kx[1] as u16;
        for at in 1..=self.patches {
            borders[at] = borders[at - 1] + self.patch_bands[at - 1] as u16;
        }
        self.f_limit = [0; 30];
        self.f_limit[..=self.n[0]].copy_from_slice(&self.f_low[..=self.n[0]]);
        if self.patches > 1 {
            for (slot, &border) in self.f_limit[self.n[0] + 1..self.n[0] + self.patches]
                .iter_mut()
                .zip(&borders[1..self.patches])
            {
                *slot = border;
            }
        }
        let count = self.patches + self.n[0];
        self.f_limit[..count].sort_unstable();
        self.n_limit = self.n[0] + self.patches - 1;
        let is_border = |value: u16| borders[..=self.patches].contains(&value);
        let (mut from, mut to) = (1usize, 0usize);
        while to < self.n_limit && from < count {
            if f32::from(self.f_limit[from]) >= f32::from(self.f_limit[to]) * narrowest {
                to += 1;
                self.f_limit[to] = self.f_limit[from];
                from += 1;
            } else if self.f_limit[from] == self.f_limit[to] || !is_border(self.f_limit[from]) {
                from += 1;
                self.n_limit -= 1;
            } else if !is_border(self.f_limit[to]) {
                self.f_limit[to] = self.f_limit[from];
                from += 1;
                self.n_limit -= 1;
            } else {
                to += 1;
                self.f_limit[to] = self.f_limit[from];
                from += 1;
            }
        }
    }
}
