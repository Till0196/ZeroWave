//! 比率（パン&スキャン）。矩形はクリップ空間（−1〜1、y は上向き）の `[左, 下, 右, 上]`。

/// 映像の形と表示する領域の形が違うときの合わせ方（全画面表示で使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stretch {
    /// 映像の形のまま、領域に収める。
    #[default]
    Fit,
    /// 映像の形のまま、領域いっぱいに広げてはみ出す所を切る。
    Fill,
    /// 領域の形に伸ばす。
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PanScan {
    /// 左、上、右、下。0〜1。
    pub crop: [f32; 4],
    /// 見える所がこの形（横÷縦）になるよう、余る向きを両側から等しく切る。
    pub cut_to: Option<f32>,
    /// 見える所をこの形で出す（伸び縮みする）。
    pub aspect: Option<f32>,
    pub mask: bool,
    pub stretch: Stretch,
}

impl PanScan {
    pub fn is_default(&self) -> bool {
        self.crop == [0.0; 4]
            && self.cut_to.is_none()
            && self.aspect.is_none()
            && self.stretch == Stretch::Fit
    }

    /// `source` は映像の縦横比。
    pub fn visible(&self, source: f32) -> [f32; 4] {
        let crop = self.crop.map(|f| f.clamp(0.0, 0.45));
        match self.cut_to.filter(|cut| *cut > 0.0) {
            Some(cut) => cut_to(crop, source, cut),
            None => crop,
        }
    }

    pub fn shape(&self, source: f32) -> f32 {
        if let Some(aspect) = self.aspect.filter(|aspect| *aspect > 0.0) {
            return aspect;
        }
        let [left, top, right, bottom] = self.visible(source);
        source * (1.0 - left - right) / (1.0 - top - bottom).max(1e-6)
    }

    /// 縦横比 `have` の領域に出すときに見える所（`Fill` は領域の形に切る）。
    pub fn visible_in(&self, source: f32, have: f32) -> [f32; 4] {
        let visible = self.visible(source);
        if self.stretch != Stretch::Fill || have <= 0.0 {
            return visible;
        }
        // 見える所の形を、出す形（`shape`）で測る。
        let [left, top, right, bottom] = visible;
        let shown = source * (1.0 - left - right) / (1.0 - top - bottom).max(1e-6);
        let scale = self.shape(source) / shown.max(1e-6);
        cut_to(visible, source * scale, have)
    }

    /// 縦横比 `have` の領域に出すときの形。
    pub fn shape_in(&self, source: f32, have: f32) -> f32 {
        match self.stretch {
            Stretch::Fit => self.shape(source),
            Stretch::Fill | Stretch::Stretch if have > 0.0 => have,
            Stretch::Fill | Stretch::Stretch => self.shape(source),
        }
    }

    /// 縦横比 `have` の画面での、表示枠と映像全体の矩形。
    pub fn fit(&self, source: f32, have: f32) -> ([f32; 4], [f32; 4]) {
        let wanted = self.shape_in(source, have);
        // 丸め誤差の 1 ピクセルで余白を作らない。
        let (x, y) = if (have - wanted).abs() / wanted < 0.01 {
            (1.0, 1.0)
        } else if have > wanted {
            (wanted / have, 1.0)
        } else {
            (1.0, have / wanted)
        };
        let frame = [-x, -y, x, y];
        let [left, top, right, bottom] = self.visible_in(source, have);
        let wide = (1.0 - left - right).max(1e-6);
        let tall = (1.0 - top - bottom).max(1e-6);
        let picture = [
            -x - 2.0 * x * left / wide,
            -y - 2.0 * y * bottom / tall,
            x + 2.0 * x * right / wide,
            y + 2.0 * y * top / tall,
        ];
        (frame, picture)
    }
}

/// 見える所 `crop`（左、上、右、下）を、形が `cut` になるよう余る向きを両側から等しく切る。
/// `source` は映像全体の形。
fn cut_to(crop: [f32; 4], source: f32, cut: f32) -> [f32; 4] {
    let [mut left, mut top, mut right, mut bottom] = crop;
    let wide = 1.0 - left - right;
    let tall = 1.0 - top - bottom;
    let shape = source * wide / tall.max(1e-6);
    if shape > cut {
        let extra = wide - wide * cut / shape;
        left += extra / 2.0;
        right += extra / 2.0;
    } else if shape < cut {
        let extra = tall - tall * shape / cut;
        top += extra / 2.0;
        bottom += extra / 2.0;
    }
    [left, top, right, bottom]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn the_default_is_the_picture_as_it_is() {
        let (frame, picture) = PanScan::default().fit(16.0 / 9.0, 16.0 / 9.0);
        assert!(close(frame, [-1.0, -1.0, 1.0, 1.0]));
        assert!(close(picture, frame));
    }

    #[test]
    fn a_side_cut_shows_the_middle_four_by_three() {
        let side = PanScan {
            cut_to: Some(4.0 / 3.0),
            mask: true,
            ..PanScan::default()
        };
        let visible = side.visible(16.0 / 9.0);
        assert!(close(visible, [0.125, 0.0, 0.125, 0.0]), "{visible:?}");
        assert!((side.shape(16.0 / 9.0) - 4.0 / 3.0).abs() < 1e-4);
        let (frame, picture) = side.fit(16.0 / 9.0, 4.0 / 3.0);
        assert!(close(frame, [-1.0, -1.0, 1.0, 1.0]));
        assert!(
            close(picture, [-4.0 / 3.0, -1.0, 4.0 / 3.0, 1.0]),
            "{picture:?}"
        );
    }

    #[test]
    fn a_letterbox_and_a_double_frame_are_cut_out() {
        let letterbox = PanScan {
            cut_to: Some(16.0 / 9.0),
            ..PanScan::default()
        };
        assert!(close(
            letterbox.visible(4.0 / 3.0),
            [0.0, 0.125, 0.0, 0.125]
        ));
        let double = PanScan {
            crop: [0.125; 4],
            ..PanScan::default()
        };
        assert!((double.shape(16.0 / 9.0) - 16.0 / 9.0).abs() < 1e-4);
        let (frame, picture) = double.fit(16.0 / 9.0, 16.0 / 9.0);
        assert!(close(frame, [-1.0, -1.0, 1.0, 1.0]));
        assert!(close(
            picture,
            [-4.0 / 3.0, -4.0 / 3.0, 4.0 / 3.0, 4.0 / 3.0]
        ));
    }

    #[test]
    fn a_forced_shape_is_pillarboxed() {
        let four = PanScan {
            aspect: Some(4.0 / 3.0),
            ..PanScan::default()
        };
        let (frame, picture) = four.fit(16.0 / 9.0, 16.0 / 9.0);
        assert!(close(frame, [-0.75, -1.0, 0.75, 1.0]));
        assert!(close(picture, frame));
    }

    #[test]
    fn a_full_screen_fill_cuts_to_the_screen() {
        let fill = PanScan {
            stretch: Stretch::Fill,
            ..PanScan::default()
        };
        assert!(!fill.is_default());
        // 16:9 の映像を 16:10 の画面いっぱいに：左右を切る。
        let have = 16.0 / 10.0;
        let visible = fill.visible_in(16.0 / 9.0, have);
        let cut = (1.0 - (16.0 / 10.0) / (16.0 / 9.0)) / 2.0;
        assert!(close(visible, [cut, 0.0, cut, 0.0]), "{visible:?}");
        let (frame, picture) = fill.fit(16.0 / 9.0, have);
        assert!(close(frame, [-1.0, -1.0, 1.0, 1.0]));
        let x = 1.0 + 2.0 * cut / (1.0 - 2.0 * cut);
        assert!(close(picture, [-x, -1.0, x, 1.0]), "{picture:?}");
        // 4:3 に切った絵を 16:9 の画面いっぱいに：上下を切る。
        let side = PanScan {
            cut_to: Some(4.0 / 3.0),
            stretch: Stretch::Fill,
            ..PanScan::default()
        };
        let [left, top, right, bottom] = side.visible_in(16.0 / 9.0, 16.0 / 9.0);
        assert!((left - 0.125).abs() < 1e-4 && (right - 0.125).abs() < 1e-4);
        assert!((top - 0.125).abs() < 1e-4 && (bottom - 0.125).abs() < 1e-4);
    }

    #[test]
    fn a_full_screen_stretch_fills_the_screen() {
        let stretch = PanScan {
            stretch: Stretch::Stretch,
            ..PanScan::default()
        };
        let (frame, picture) = stretch.fit(4.0 / 3.0, 16.0 / 9.0);
        assert!(close(frame, [-1.0, -1.0, 1.0, 1.0]));
        assert!(close(picture, frame));
        assert!((stretch.shape_in(4.0 / 3.0, 16.0 / 9.0) - 16.0 / 9.0).abs() < 1e-4);
        assert!((stretch.shape(4.0 / 3.0) - 4.0 / 3.0).abs() < 1e-4);
    }
}
