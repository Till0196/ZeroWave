// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! データ放送の文書を映像の位置に表示する。

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, Rect, TextureHandle, pos2, vec2};

use zwcore::data::Shown;

/// 名前とバージョンが同じなら中身も同じなので、世代が変わっても使い回す。
#[derive(Default)]
pub struct Textures {
    held: BTreeMap<String, (usize, TextureHandle)>,
}

impl std::fmt::Debug for Textures {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Textures({})", self.held.len())
    }
}

impl Textures {
    fn get(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        picture: &Arc<zwcore::data::Picture>,
        nearest: bool,
    ) -> TextureHandle {
        let key = Arc::as_ptr(picture) as usize ^ usize::from(nearest);
        if let Some((held_key, handle)) = self.held.get(name)
            && *held_key == key
        {
            return handle.clone();
        }
        let options = if nearest {
            egui::TextureOptions::NEAREST
        } else {
            egui::TextureOptions::LINEAR
        };
        let image = crate::ui::rgba_image(picture);
        if let Some((held_key, handle)) = self.held.get_mut(name)
            && handle.size() == picture.size()
        {
            handle.set(image, options);
            *held_key = key;
            return handle.clone();
        }
        let handle = ctx.load_texture(format!("データ放送 {name}"), image, options);
        self.held.insert(name.to_owned(), (key, handle.clone()));
        handle
    }

    pub fn keep_only(&mut self, _shown: &Shown) {
        self.held.retain(|name, _| name == PAGE);
    }
}

const PAGE: &str = "\u{0}page";

pub fn show(ui: &mut egui::Ui, area: Rect, shown: &Shown, textures: &mut Textures) {
    if shown.invisible || shown.size.0 <= 0.0 || shown.size.1 <= 0.0 {
        return;
    }
    let ctx = ui.ctx().clone();
    let base = ui.painter().with_clip_rect(area);
    // 置く矩形も画面の画素に揃える。半画素ずれると映像の穴の縁が黒い線になる。
    let Some(page) = &shown.page else {
        return;
    };
    let ppp = ctx.pixels_per_point();
    let snapped = Rect::from_min_max(
        pos2(
            (area.left() * ppp).round() / ppp,
            (area.top() * ppp).round() / ppp,
        ),
        pos2(
            (area.left() * ppp).round() / ppp + page.width as f32 / ppp,
            (area.top() * ppp).round() / ppp + page.height as f32 / ppp,
        ),
    );
    // 画素数が合っていれば補間しない（合わなければ再レンダリングが届くまで引き伸ばす）。
    let exact = ((snapped.width() - area.width()) * ppp).abs() <= 1.5
        && ((snapped.height() - area.height()) * ppp).abs() <= 1.5;
    let handle = textures.get(&ctx, PAGE, page, exact);
    base.image(
        handle.id(),
        if exact { snapped } else { area },
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// C プロファイルの文書を別ウィンドウいっぱいに 240×480 の比で表示する。
pub fn popup(ui: &mut egui::Ui, area: Rect, shown: &Shown, textures: &mut Textures) {
    ui.painter().rect_filled(area, 0.0, Color32::BLACK);
    if shown.size.0 <= 0.0 || shown.size.1 <= 0.0 {
        return;
    }
    let wanted = shown.size.0 / shown.size.1;
    let have = area.width() / area.height().max(1e-3);
    let (width, height) = if have > wanted {
        (area.height() * wanted, area.height())
    } else {
        (area.width(), area.width() / wanted)
    };
    let page = Rect::from_min_size(
        pos2(
            area.left() + (area.width() - width) / 2.0,
            area.top() + (area.height() - height) / 2.0,
        ),
        vec2(width, height),
    );
    show(ui, page, shown, textures);
    if shown.waiting {
        loading(ui, page, (9.0 * page.width() / 240.0).clamp(10.0, 20.0));
    }
}

/// 受像機の「データ取得中です。」を `page` の右下に出す（d を押して文書がまだ出ない間）。
pub fn loading(ui: &egui::Ui, page: Rect, size: f32) {
    let text = "データ取得中です。";
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(size),
        Color32::WHITE,
    );
    let pad = galley.size().y * 0.35;
    let band_size = galley.size() + vec2(pad * 2.0, pad * 2.0);
    let margin = band_size.y * 0.5;
    let band = Rect::from_min_size(page.max - band_size - vec2(margin, margin), band_size);
    ui.painter()
        .rect_filled(band, 0.0, Color32::from_black_alpha(200));
    ui.painter()
        .galley(band.min + vec2(pad, pad), galley, Color32::WHITE);
}

/// 色・矢印・決定・数字は文書がその組を取るときだけ文書のもの（TR-B14 第二分冊 2.1.10）。
/// 修飾キー付きは文書へ送らない（割り当ての方で使う）。
pub fn remote_key(
    key: egui::Key,
    modifiers: egui::Modifiers,
    takes: &dyn Fn(&str) -> bool,
) -> Option<zwsdk::layer::Key> {
    if modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd {
        return None;
    }
    let basic = takes("basic");
    let numeric = takes("numeric-tuning");
    // 色のボタンは見えている文書があるときだけ（無ければ F1〜F4 は割り当ての方へ）。
    let shown = basic || numeric || takes("data-button");
    Some(match key {
        egui::Key::D => zwsdk::layer::Key::DataButton,
        egui::Key::F1 if shown => zwsdk::layer::Key::Blue,
        egui::Key::F2 if shown => zwsdk::layer::Key::Red,
        egui::Key::F3 if shown => zwsdk::layer::Key::Green,
        egui::Key::F4 if shown => zwsdk::layer::Key::Yellow,
        egui::Key::ArrowUp if basic => zwsdk::layer::Key::Up,
        egui::Key::ArrowDown if basic => zwsdk::layer::Key::Down,
        egui::Key::ArrowLeft if basic => zwsdk::layer::Key::Left,
        egui::Key::ArrowRight if basic => zwsdk::layer::Key::Right,
        egui::Key::Enter if basic => zwsdk::layer::Key::Enter,
        egui::Key::Backspace | egui::Key::Escape if basic => zwsdk::layer::Key::Back,
        egui::Key::Num0 if numeric => zwsdk::layer::Key::Digit(0),
        egui::Key::Num1 if numeric => zwsdk::layer::Key::Digit(1),
        egui::Key::Num2 if numeric => zwsdk::layer::Key::Digit(2),
        egui::Key::Num3 if numeric => zwsdk::layer::Key::Digit(3),
        egui::Key::Num4 if numeric => zwsdk::layer::Key::Digit(4),
        egui::Key::Num5 if numeric => zwsdk::layer::Key::Digit(5),
        egui::Key::Num6 if numeric => zwsdk::layer::Key::Digit(6),
        egui::Key::Num7 if numeric => zwsdk::layer::Key::Digit(7),
        egui::Key::Num8 if numeric => zwsdk::layer::Key::Digit(8),
        egui::Key::Num9 if numeric => zwsdk::layer::Key::Digit(9),
        _ => return None,
    })
}
