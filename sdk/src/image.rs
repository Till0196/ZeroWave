// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画像の処理。プラグインからは [`crate::plug::host`] を通して本体が native で行う。

/// RGBA（アルファで割った値）を面積平均で縮める。アルファを重みに混ぜる。`out` は `to` の大きさ。
pub fn shrink_into(rgba: &[u8], from: (u32, u32), to: (u32, u32), out: &mut [u8]) {
    let (fw, fh) = (from.0 as usize, from.1 as usize);
    let (tw, th) = (to.0 as usize, to.1 as usize);
    out.fill(0);
    if fw == 0 || fh == 0 || tw == 0 || th == 0 {
        return;
    }
    let xs = spans(tw, fw as f64 / tw as f64, fw);
    let ys = spans(th, fh as f64 / th as f64, fh);
    let rows = |first: usize, out: &mut [u8]| {
        for (y, ytaps) in ys.iter().enumerate().skip(first).take(out.len() / (tw * 4)) {
            for (x, xtaps) in xs.iter().enumerate() {
                let (mut r, mut g, mut b, mut a, mut area) = (0.0, 0.0, 0.0, 0.0, 0.0);
                for &(sy, wy) in ytaps {
                    for &(sx, wx) in xtaps {
                        let w = wx * wy;
                        let at = (sy * fw + sx) * 4;
                        let alpha = f64::from(rgba[at + 3]) * w;
                        r += f64::from(rgba[at]) * alpha;
                        g += f64::from(rgba[at + 1]) * alpha;
                        b += f64::from(rgba[at + 2]) * alpha;
                        a += alpha;
                        area += w;
                    }
                }
                let at = ((y - first) * tw + x) * 4;
                if a > 0.0 {
                    out[at] = (r / a).round() as u8;
                    out[at + 1] = (g / a).round() as u8;
                    out[at + 2] = (b / a).round() as u8;
                }
                out[at + 3] = (a / area).round().min(255.0) as u8;
            }
        }
    };
    #[cfg(target_arch = "wasm32")]
    rows(0, out);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8);
        let per = th.div_ceil(threads);
        std::thread::scope(|scope| {
            for (n, chunk) in out.chunks_mut(per * tw * 4).enumerate() {
                let rows = &rows;
                scope.spawn(move || rows(n * per, chunk));
            }
        });
    }
}

pub fn shrink(rgba: &[u8], from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    let mut out = vec![0u8; to.0 as usize * to.1 as usize * 4];
    shrink_into(rgba, from, to, &mut out);
    out
}

/// 出力の画素ごとに、重なる入力の画素とその重なりの幅。
fn spans(n: usize, step: f64, limit: usize) -> Vec<Vec<(usize, f64)>> {
    (0..n)
        .map(|i| {
            let (a, b) = (i as f64 * step, (i as f64 + 1.0) * step);
            let mut taps = Vec::new();
            let mut k = a.floor() as usize;
            while (k as f64) < b && k < limit {
                let w = (b.min(k as f64 + 1.0) - a.max(k as f64)).max(0.0);
                if w > 0.0 {
                    taps.push((k, w));
                }
                k += 1;
            }
            taps
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halving_averages_by_area_and_alpha() {
        // 2×2 を 1×1 に：不透明の赤 1 画素と透明 3 画素なら、色は赤のまま、アルファは 1/4。
        let mut rgba = vec![0u8; 16];
        rgba[..4].copy_from_slice(&[255, 0, 0, 255]);
        assert_eq!(shrink(&rgba, (2, 2), (1, 1)), [255, 0, 0, 64]);
        // 端数の倍率でも、全面が同じ色なら同じ色。
        let grey: Vec<u8> = [90u8, 90, 90, 255].repeat(20 * 12);
        assert!(
            shrink(&grey, (20, 12), (12, 7))
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| *px == [90, 90, 90, 255])
        );
    }
}
