// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画像キャプチャ。どのレイヤーを含めるかを選べる（`[capture] include`）。トーストと全画面表示のバーは含めない。

mod encode;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug)]
pub struct Job {
    pub drawn: crate::draw::overlay::Drawn,
    pub size: (u32, u32),
    pub video: (u32, u32, u32, u32),
    pub with_video: bool,
    pub filtered: bool,
    pub copy: bool,
    pub path: PathBuf,
    pub encoding: Encoding,
    pub hdr: bool,
}

#[derive(Debug, Clone)]
pub struct Encoding {
    pub format: String,
    pub jpeg_quality: u8,
    pub png_level: u8,
}

#[derive(Debug)]
pub struct Readback {
    pub source: Source,
    pub stride: u32,
    pub size: (u32, u32),
    pub layout: Layout,
}

#[derive(Debug)]
pub enum Source {
    Gpu(wgpu::Buffer),
    #[cfg(all(unix, not(target_os = "macos")))]
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Rgba8,
    /// 10 ビットの RGB のまま、BT.2020。`transfer` は ITU-T H.273 の番号（16 が PQ、18 が HLG）。
    Rgb10a2 {
        transfer: u8,
    },
    Half,
}

pub fn folder_of(written: &str, kind: Folder) -> PathBuf {
    if !written.trim().is_empty() {
        return PathBuf::from(written.trim());
    }
    default_folder(kind)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Pictures,
    Videos,
}

pub fn default_folder(kind: Folder) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "linux") {
        let key = match kind {
            Folder::Pictures => "XDG_PICTURES_DIR",
            Folder::Videos => "XDG_VIDEOS_DIR",
        };
        if let Ok(text) = std::fs::read_to_string(home.join(".config/user-dirs.dirs")) {
            for line in text.lines() {
                if let Some(value) = line
                    .strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
                {
                    let value = value.trim().trim_matches('"');
                    let path = value.replace("$HOME", &home.to_string_lossy());
                    return PathBuf::from(path);
                }
            }
        }
    }
    home.join(match (kind, cfg!(target_os = "macos")) {
        (Folder::Pictures, _) => "Pictures",
        (Folder::Videos, true) => "Movies",
        (Folder::Videos, false) => "Videos",
    })
}

pub struct Composer {
    context: egui::Context,
    fonts_seen: u64,
    broadcast: crate::ui::panes::databroadcast::Textures,
    layers: BTreeMap<String, (usize, egui::TextureHandle)>,
}

impl std::fmt::Debug for Composer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Composer").finish_non_exhaustive()
    }
}

pub struct Shown<'a> {
    pub size: (u32, u32),
    pub ratio: f32,
    pub layers: &'a [Arc<zwcore::layers::Layer>],
    pub broadcast: Option<Arc<zwcore::data::Shown>>,
    pub captions: Option<Arc<zwcore::captions::Captions>>,
    pub osd: Option<(&'a crate::ui::overlay::osd::Osd, f32)>,
    pub volume: Option<(&'a crate::draw::scene::Volume, f32)>,
    pub idle: Option<&'a crate::draw::scene::Idle>,
    pub hdr: bool,
    pub file: &'a crate::config::File,
}

impl Composer {
    pub fn new() -> Self {
        let context = egui::Context::default();
        crate::ui::text::font::install(&context);
        Self {
            fonts_seen: crate::ui::text::font::generation(),
            context,
            broadcast: crate::ui::panes::databroadcast::Textures::default(),
            layers: BTreeMap::new(),
        }
    }

    pub fn compose(&mut self, shown: &Shown<'_>) -> (crate::draw::overlay::Drawn, egui::Rect) {
        crate::ui::text::font::refresh(&self.context, &mut self.fonts_seen);
        let (width, height) = shown.size;
        let whole =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32));
        let frame = fit(whole, shown.ratio);
        let mut input = egui::RawInput {
            screen_rect: Some(whole),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(1.0);
        let capture = &shown.file.capture;
        let include = |key: &str| capture.includes(key);
        let broadcast = &mut self.broadcast;
        let textures = &mut self.layers;
        let mut used = std::collections::BTreeSet::new();
        let output = self.context.run_ui(input, |ui| {
            use zwsdk::layer::Slot;
            let ctx = ui.ctx().clone();
            let painter = ui.painter().clone();
            let mut layer = |from: (Slot, u8), to: (Slot, u8)| {
                for layer in shown
                    .layers
                    .iter()
                    .filter(|layer| (from..to).contains(&layer.place.order()))
                    .filter(|layer| include(&plugin_key(&layer.name)))
                {
                    let key = Arc::as_ptr(&layer.image) as usize;
                    used.insert(layer.name.clone());
                    let handle = match textures.get_mut(&layer.name) {
                        Some((had, handle)) if *had == key => handle.clone(),
                        _ => {
                            let handle = ctx.load_texture(
                                format!("キャプチャのレイヤー {}", layer.name),
                                crate::ui::rgba_image(&layer.image),
                                egui::TextureOptions::LINEAR,
                            );
                            textures.insert(layer.name.clone(), (key, handle.clone()));
                            handle
                        }
                    };
                    painter.image(
                        handle.id(),
                        frame,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            };
            layer((Slot::Video, 0), (Slot::Databroadcast, 0));
            if include("databroadcast")
                && let Some(page) = &shown.broadcast
            {
                broadcast.keep_only(page);
                crate::ui::panes::databroadcast::show(ui, frame, page, broadcast);
            }
            layer((Slot::Databroadcast, 2), (Slot::Captions, 1));
            if let Some(captions) = &shown.captions {
                let caption = crate::ui::overlay::caption::Decor::of(&shown.file.caption, false);
                let superimpose = crate::ui::overlay::caption::Decor::of(&shown.file.caption, true);
                if include("caption")
                    && let Some((layout, since)) = &captions.caption
                {
                    crate::ui::overlay::caption::show(ui, frame, layout, since.elapsed(), &caption);
                }
                if include("caption")
                    && let Some(document) = &captions.ttml_caption
                {
                    crate::ui::overlay::ttml::show(ui, frame, document, &caption);
                }
                if include("superimpose") {
                    if let Some((layout, since)) = &captions.superimpose {
                        crate::ui::overlay::caption::show(
                            ui,
                            frame,
                            layout,
                            since.elapsed(),
                            &superimpose,
                        );
                    }
                    if let Some(document) = &captions.ttml_superimpose {
                        crate::ui::overlay::ttml::show(ui, frame, document, &superimpose);
                    }
                }
            }
            layer((Slot::Captions, 1), (Slot::Osd, 1));
            if let Some(idle) = shown.idle {
                crate::ui::overlay::idle::show(ui, frame, idle);
            }
            if include("osd") {
                if let Some((osd, opacity)) = shown.osd {
                    crate::ui::overlay::osd::show(ui, frame, osd, opacity, shown.hdr);
                }
                if let Some((volume, opacity)) = shown.volume {
                    crate::ui::overlay::volume::show(ui, frame, volume, opacity, 0.0, shown.hdr);
                }
            }
            layer((Slot::Osd, 1), (Slot::Osd, 3));
        });
        self.layers.retain(|name, _| used.contains(name));
        let jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        (
            crate::draw::overlay::Drawn::new(
                jobs,
                output.textures_delta,
                output.pixels_per_point,
                shown.size,
            ),
            frame,
        )
    }
}

fn fit(area: egui::Rect, ratio: f32) -> egui::Rect {
    if ratio <= 0.0 || !ratio.is_finite() {
        return area;
    }
    let (width, height) = if area.width() / area.height() > ratio {
        (area.height() * ratio, area.height())
    } else {
        (area.width(), area.width() / ratio)
    };
    egui::Rect::from_center_size(area.center(), egui::vec2(width, height))
}

pub fn size_of(spelled: &str, source: Option<(u32, u32)>, display: (u32, u32)) -> (u32, u32) {
    let even = |(width, height): (u32, u32)| (width.max(2) & !1, height.max(2) & !1);
    match spelled {
        "display" => even(display),
        "source" => even(source.unwrap_or((1920, 1080))),
        other if let Some((top, bottom)) = other.split_once('/') => {
            let (width, height) = source.unwrap_or((1920, 1080));
            let (Ok(top), Ok(bottom)) = (top.parse::<u32>(), bottom.parse::<u32>()) else {
                return even((width, height));
            };
            let bottom = bottom.max(1);
            even((width * top / bottom, height * top / bottom))
        }
        other => other
            .split_once('x')
            .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
            .map(even)
            .unwrap_or_else(|| even(source.unwrap_or((1920, 1080)))),
    }
}

static WRITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn writing() -> usize {
    WRITING.load(std::sync::atomic::Ordering::Acquire)
}

pub fn finish(
    copy: bool,
    path: PathBuf,
    encoding: Encoding,
    readback: Readback,
    said: std::sync::mpsc::Sender<String>,
) {
    WRITING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let spawned = std::thread::Builder::new()
        .name(String::from("capture"))
        .spawn(move || {
            let started = std::time::Instant::now();
            let message = match save(copy, &path, &encoding, &readback) {
                Ok(Some(used)) => {
                    let name = used
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if readback.layout == Layout::Rgba8 {
                        format!("画像を保存しました。\n{name}")
                    } else {
                        format!("画像を保存しました（HDR）。\n{name}")
                    }
                }
                Ok(None) => String::from("画像をクリップボードにコピーしました。"),
                Err(why) if copy => {
                    crate::stage::said_with("クリップボードにコピーできません。", &why)
                }
                Err(why) => crate::stage::said_with("画像を保存できません。", &why),
            };
            tracing::debug!("キャプチャ：保存スレッドの所要時間 {:?}", started.elapsed());
            WRITING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            let _ = said.send(message);
        });
    if let Err(error) = spawned {
        WRITING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        tracing::warn!("キャプチャを保存するスレッドを起動できない: {error}");
    }
}

fn save(
    copy: bool,
    path: &std::path::Path,
    encoding: &Encoding,
    readback: &Readback,
) -> Result<Option<PathBuf>, String> {
    let started = std::time::Instant::now();
    let (width, height) = readback.size;
    let stride = readback.stride as usize;
    let size = (width, height);
    let arrange = |data: &[u8]| match readback.layout {
        Layout::Rgba8 if copy => pixels(data, stride, size, 4, rgba_row),
        Layout::Rgba8 => pixels(data, stride, size, 3, rgb_row),
        Layout::Rgb10a2 { .. } => pixels(data, stride, size, 6, rgb10a2_row),
        Layout::Half => pixels(data, stride, size, 6, half_row),
    };
    let pixels = match &readback.source {
        Source::Gpu(buffer) => {
            let mapped = buffer
                .get_mapped_range(..)
                .map_err(|error| error.to_string())?;
            let pixels = arrange(&mapped);
            drop(mapped);
            buffer.unmap();
            pixels
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        Source::Bytes(bytes) => arrange(bytes),
    };
    let arranged = started.elapsed();
    if copy {
        copy_image(&pixels, (width, height))?;
        tracing::debug!(
            "キャプチャ：並べ替え {arranged:?}、コピー {:?}",
            started.elapsed() - arranged
        );
        return Ok(None);
    }
    let started = std::time::Instant::now();
    let (encoded, kind) = match readback.layout {
        Layout::Rgb10a2 { transfer } => (
            encode::png(
                &pixels,
                (width, height),
                true,
                encoding.png_level,
                Some([9, transfer, 0, 1]),
            ),
            "PNG 16 ビット",
        ),
        Layout::Half => (
            encode::png(
                &pixels,
                (width, height),
                true,
                encoding.png_level,
                Some([9, 16, 0, 1]),
            ),
            "PNG 16 ビット",
        ),
        Layout::Rgba8 => match encoding.format.as_str() {
            "jpg" | "jpeg" | "bmp" => {
                (encode_image(encoding, pixels, (width, height))?, "JPEG/BMP")
            }
            _ => (
                encode::png(&pixels, (width, height), false, encoding.png_level, None),
                "PNG",
            ),
        },
    };
    let encoded_in = started.elapsed();
    let used = store(path, &encoded)?;
    tracing::info!(
        "キャプチャ {kind}：並べ替え {arranged:?}、エンコード {encoded_in:?}、書き込み {:?}、{:.1} MB",
        started.elapsed() - encoded_in,
        encoded.len() as f64 / 1e6
    );
    Ok(Some(used))
}

fn pixels(
    mapped: &[u8],
    stride: usize,
    (width, height): (u32, u32),
    bytes: usize,
    convert: fn(&[u8], &mut [u8]),
) -> Vec<u8> {
    let (width, height) = (width as usize, height as usize);
    let row = width * bytes;
    let mut out = vec![0u8; row * height];
    encode::rows_in_parallel(&mut out, row, height, |rows, chunk| {
        for (y, line) in rows.zip(chunk.chunks_exact_mut(row)) {
            convert(&mapped[y * stride..], line);
        }
    });
    out
}

fn rgba_row(from: &[u8], to: &mut [u8]) {
    to.copy_from_slice(&from[..to.len()]);
}

fn rgb_row(from: &[u8], to: &mut [u8]) {
    for (pixel, out) in from
        .as_chunks::<4>()
        .0
        .iter()
        .zip(to.as_chunks_mut::<3>().0)
    {
        *out = [pixel[0], pixel[1], pixel[2]];
    }
}

fn rgb10a2_row(from: &[u8], to: &mut [u8]) {
    for (word, out) in from
        .as_chunks::<4>()
        .0
        .iter()
        .zip(to.as_chunks_mut::<6>().0)
    {
        let packed = u32::from_le_bytes(*word);
        for (channel, shift) in [0, 10, 20].into_iter().enumerate() {
            let ten = ((packed >> shift) & 0x3ff) as u16;
            let wide = (ten << 6) | (ten >> 4);
            out[channel * 2..channel * 2 + 2].copy_from_slice(&wide.to_be_bytes());
        }
    }
}

fn half_row(from: &[u8], to: &mut [u8]) {
    for (pixel, out) in from
        .as_chunks::<8>()
        .0
        .iter()
        .zip(to.as_chunks_mut::<6>().0)
    {
        for channel in 0..3 {
            let bits = u16::from_le_bytes([pixel[channel * 2], pixel[channel * 2 + 1]]);
            let value = half_to_f32(bits).clamp(0.0, 1.0);
            let wide = (value * 65535.0 + 0.5) as u16;
            out[channel * 2..channel * 2 + 2].copy_from_slice(&wide.to_be_bytes());
        }
    }
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * fraction * 2f32.powi(-24),
        31 if fraction == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15),
    }
}

fn store(path: &std::path::Path, encoded: &[u8]) -> Result<PathBuf, String> {
    let (used, mut file) = create(path)?;
    std::io::Write::write_all(&mut file, encoded).map_err(|error| error.to_string())?;
    Ok(used)
}

fn create(path: &std::path::Path) -> Result<(PathBuf, std::fs::File), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    zwcore::record::create_unused(path).map_err(|error| error.to_string())
}

fn encode_image(
    encoding: &Encoding,
    rgb: Vec<u8>,
    (width, height): (u32, u32),
) -> Result<Vec<u8>, String> {
    let image = image::RgbImage::from_raw(width, height, rgb)
        .ok_or_else(|| String::from("画像のサイズが合わない"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    let written = match encoding.format.as_str() {
        "bmp" => image.write_to(&mut out, image::ImageFormat::Bmp),
        _ => image.write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out,
            encoding.jpeg_quality.clamp(1, 100),
        )),
    };
    written.map_err(|error| error.to_string())?;
    Ok(out.into_inner())
}

/// クリップボードへ。所有権はプロセスが終わるまで保持する（X11 では手放すと消える）。
fn copy_image(rgba: &[u8], (width, height): (u32, u32)) -> Result<(), String> {
    static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);
    let mut slot = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
    }
    let clipboard = slot
        .as_mut()
        .ok_or_else(|| String::from("クリップボードが無い"))?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Borrowed(rgba),
        })
        .map_err(|error| error.to_string())
}

pub fn extension(format: &str) -> &'static str {
    match format {
        "jpg" | "jpeg" => "jpg",
        "bmp" => "bmp",
        _ => "png",
    }
}

pub const BUILT_IN: &[(&str, &str)] = &[
    ("superimpose", "文字スーパー"),
    ("osd", "OSD"),
    ("caption", "字幕"),
    ("databroadcast", "データ放送"),
];

pub const VIDEO_FILTER: &str = "video_filter";

pub fn video_filter_row(names: &[String]) -> Option<(String, String)> {
    const SHOWN: &str = "映像フィルター";
    (!names.is_empty()).then(|| {
        let others: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| *name != SHOWN)
            .collect();
        let shown = if others.is_empty() {
            SHOWN.to_owned()
        } else {
            format!("{SHOWN}（{}）", others.join("、"))
        };
        (VIDEO_FILTER.to_owned(), shown)
    })
}

pub fn plugin_key(layer: &str) -> String {
    format!("plugin:{layer}")
}

pub fn layer_rows(plugins: &[(String, String)]) -> Vec<(String, String)> {
    let (filters, layers): (Vec<_>, Vec<_>) =
        plugins.iter().partition(|(key, _)| key == VIDEO_FILTER);
    let mut rows = Vec::with_capacity(BUILT_IN.len() + plugins.len());
    rows.push((BUILT_IN[0].0.to_owned(), BUILT_IN[0].1.to_owned()));
    rows.extend(
        layers
            .iter()
            .map(|(key, name)| (key.clone(), format!("{name}（プラグイン）"))),
    );
    rows.extend(
        BUILT_IN[1..]
            .iter()
            .map(|(key, name)| ((*key).to_owned(), (*name).to_owned())),
    );
    rows.extend(filters.into_iter().cloned());
    rows
}

#[cfg(test)]
mod tests {

    #[test]
    fn read_back_rows_are_rearranged() {
        let mut rgb = [0u8; 6];
        rgb_row(&[1, 2, 3, 255, 4, 5, 6, 255], &mut rgb);
        assert_eq!(rgb, [1, 2, 3, 4, 5, 6]);
        let white = 0x3ffu32 | (0x3ff << 10) | (0x3ff << 20);
        let mut wide = [0u8; 6];
        rgb10a2_row(&white.to_le_bytes(), &mut wide);
        assert_eq!(wide, [0xff; 6]);
        let red = 0x200u32;
        rgb10a2_row(&red.to_le_bytes(), &mut wide);
        assert_eq!(&wide[..2], &((0x200u16 << 6) | (0x200 >> 4)).to_be_bytes());
        let one = 0x3c00u16.to_le_bytes();
        let mut halves = [0u8; 6];
        half_row(&[one[0], one[1], 0, 0, one[0], one[1], 0, 0], &mut halves);
        assert_eq!(halves, [0xff, 0xff, 0, 0, 0xff, 0xff]);
        assert_eq!(half_to_f32(0x4000), 2.0);
    }

    use super::*;

    #[test]
    fn plugin_layers_sit_between_superimpose_and_osd() {
        let filter = video_filter_row(&[String::from("映像フィルター")]);
        assert_eq!(filter.as_ref().unwrap().1, "映像フィルター");
        let rows = layer_rows(
            &std::iter::once((plugin_key("clock"), String::from("clock")))
                .chain(filter)
                .collect::<Vec<_>>(),
        );
        let keys: Vec<&str> = rows.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "superimpose",
                "plugin:clock",
                "osd",
                "caption",
                "databroadcast",
                VIDEO_FILTER
            ]
        );
        assert_eq!(rows[5].1, "映像フィルター");
        assert!(video_filter_row(&[]).is_none());
        assert!(crate::config::file::Capture::default().includes(VIDEO_FILTER));
    }

    #[test]
    fn sizes_are_read_and_kept_even() {
        assert_eq!(
            size_of("display", Some((1920, 1080)), (1281, 721)),
            (1280, 720)
        );
        assert_eq!(
            size_of("source", Some((1440, 1080)), (800, 450)),
            (1440, 1080)
        );
        assert_eq!(size_of("source", None, (800, 450)), (1920, 1080));
        assert_eq!(size_of("1280x720", None, (800, 450)), (1280, 720));
        assert_eq!(size_of("1/2", Some((1920, 1080)), (800, 450)), (960, 540));
        assert_eq!(size_of("2/3", Some((1920, 1080)), (800, 450)), (1280, 720));
    }

    #[test]
    fn a_picture_is_fitted_by_its_shape() {
        let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1440.0));
        let fitted = fit(area, 16.0 / 9.0);
        assert_eq!(fitted.width(), 1920.0);
        assert_eq!(fitted.height(), 1080.0);
    }

    #[test]
    fn new_plugins_are_left_out_by_default() {
        let capture = crate::config::file::Capture::default();
        assert!(capture.includes("caption"));
        assert!(capture.includes("superimpose"));
        assert!(capture.includes("databroadcast"));
        assert!(!capture.includes("osd"));
        assert!(!capture.includes(&plugin_key("clock")));
    }
}
