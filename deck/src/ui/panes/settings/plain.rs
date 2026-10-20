// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! キーを一行ずつ並べるだけのページと、その選択肢。ファイルでのキー名は画面に出さない。

use super::{File, Form, Pressed};

type Choices = &'static [(&'static str, &'static str)];

const ON_START: Choices = &[("last", "最後に見たチャンネル"), ("nothing", "なし")];
const ON_START_VOLUME: Choices = &[("last", "前回の音量"), ("fixed", "指定した音量")];
const WHEEL: Choices = &[
    ("volume", "音量"),
    ("channel", "チャンネル"),
    ("nothing", "何もしない"),
];
const CHANNEL_ORDER: Choices = &[
    ("remote", "チャンネル番号順"),
    ("service", "サービス ID 順"),
];
const PASSTHROUGH: Choices = &[
    ("off", "しない"),
    ("on", "常にする"),
    ("auto", "音声の形式に合わせる"),
];
const DUAL_MONO: Choices = &[
    ("main", "主音声"),
    ("sub", "副音声"),
    ("both", "主音声＋副音声"),
];
const DOWNMIX: Choices = &[
    ("auto", "自動"),
    ("stereo", "ステレオにする"),
    ("off", "5.1ch のまま出す"),
];
const DOWNMIX_22: Choices = &[
    ("auto", "自動"),
    ("stereo", "ステレオにする"),
    ("51", "5.1ch にする"),
    ("71", "7.1ch にする"),
    ("off", "22.2ch のまま出す"),
];
const DEINTERLACE: Choices = &[
    ("auto", "自動"),
    ("off", "Weave"),
    ("bob", "Bob"),
    ("yadif", "Yadif"),
    ("bwdif", "Bwdif"),
];
const SCALER: Choices = &[
    ("nearest", "最近傍 (Nearest neighbor)"),
    ("bilinear", "バイリニア (Bilinear)"),
    ("mitchell", "バイキュービック（Mitchell）"),
    ("catmull-rom", "バイキュービック（Catmull-Rom）"),
    ("spline36", "Spline36"),
    ("lanczos3", "Lanczos3"),
    ("ewa-lanczos", "EWA Lanczos"),
];
/// 縮小の選択肢。EWA は重い。
const SCALER_DOWN: Choices = &[
    ("nearest", "最近傍 (Nearest neighbor)"),
    ("bilinear", "バイリニア (Bilinear)"),
    ("mitchell", "バイキュービック（Mitchell）"),
    ("catmull-rom", "バイキュービック（Catmull-Rom）"),
    ("spline36", "Spline36"),
    ("lanczos3", "Lanczos3"),
    ("ewa-lanczos", "EWA Lanczos"),
];
/// GLES 2.0 の経路は EGL のある unix にしか無い。
const RENDER: Choices = &[
    ("auto", "自動"),
    ("wgpu", "wgpu"),
    #[cfg(all(unix, not(target_os = "macos")))]
    ("gles", "OpenGL ES 2.0"),
];
const BACKEND: Choices = &[
    ("auto", "自動"),
    #[cfg(target_os = "windows")]
    ("dx12", "Direct3D 12"),
    #[cfg(target_os = "macos")]
    ("metal", "Metal"),
    #[cfg(not(target_os = "macos"))]
    ("vulkan", "Vulkan"),
];
const PRESENT: Choices = &[
    ("auto", "自動"),
    ("vsync", "待つ"),
    ("immediate", "待たない"),
];
const HDR: Choices = &[
    ("auto", "自動"),
    ("off", "SDR に変換して表示"),
    ("on", "HDR で出力"),
];
const THEME: Choices = &[
    ("auto", "システムの設定に合わせる"),
    ("dark", "ダーク"),
    ("light", "ライト"),
];
const FULLSCREEN: Choices = &[
    ("fit", "画面に収める"),
    ("fill", "画面いっぱいに切り抜く"),
    ("stretch", "引き伸ばす"),
];
const OSD_CHANNEL: Choices = &[
    ("logo+name", "ロゴと名前"),
    ("name", "名前のみ"),
    ("number", "番号のみ"),
    ("off", "表示しない"),
];
const CAPTION_SHOWN: Choices = &[
    ("off", "表示しない"),
    ("first", "第1言語"),
    ("second", "第2言語"),
];
const CAPTION_BACKGROUND: Choices = &[
    ("broadcast", "字幕データに従う"),
    ("always", "常に表示"),
    ("never", "常に透明"),
];
const CAPTION_OUTLINE: Choices = &[
    ("broadcast", "字幕データに従う"),
    ("always", "常に縁取る"),
    ("never", "常に透明"),
];
const LOG_LEVEL: Choices = &[("info", "Info"), ("debug", "Debug"), ("trace", "Trace")];
const CAPTURE_FORMAT: Choices = &[("bmp", "BMP"), ("jpg", "JPEG"), ("png", "PNG")];
const CAPTURE_SIZE: Choices = crate::window::menu::CAPTURE_SIZES;
const ASPECT: Choices = &[
    ("default", "デフォルト"),
    ("16:9", "16:9"),
    ("letterbox", "16:9 レターボックス"),
    ("double", "16:9 超額縁"),
    ("side_cut", "4:3 サイドカット"),
    ("4:3", "4:3"),
];
const PREVIEW_CHANNEL: &str = "ゼロウェーブテレビ";
const PREVIEW_EVENT: &str = "[新][字]湖と森の一年　第１回";
const ONESEG: Choices = &[
    ("fallback", "受信できないときのみ切り替える"),
    ("never", "切り替えない"),
    ("always", "常にワンセグ"),
];

pub(super) fn general(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) {
    form.section("起動");
    form.key("general.on_start");
    form.select(
        "起動時のチャンネル",
        &mut edited.general.on_start,
        &loaded.general.on_start,
        ON_START,
    );
    form.key("general.on_start_volume");
    form.select(
        "起動時の音量",
        &mut edited.general.on_start_volume,
        &loaded.general.on_start_volume,
        ON_START_VOLUME,
    );
    form.key("general.volume");
    form.slider(
        "音量",
        &mut edited.general.volume,
        loaded.general.volume,
        0..=100,
        "%",
    );
    form.key("general.single_instance");
    form.check(
        "",
        "複数起動を禁止する",
        &mut edited.general.single_instance,
        loaded.general.single_instance,
    );
    form.section("画面");
    form.key("general.no_sleep");
    form.check(
        "",
        "再生中はスクリーンセーバーとスリープを止める",
        &mut edited.general.no_sleep,
        loaded.general.no_sleep,
    );
    form.key("general.always_on_top");
    form.check(
        "",
        "常に前面に表示",
        &mut edited.general.always_on_top,
        loaded.general.always_on_top,
    );
    form.section("地域設定");
    form.key("general.zipcode");
    form.text_sized(
        "郵便番号",
        &mut edited.general.zipcode,
        &loaded.general.zipcode,
        "100-0001",
        120.0,
    );
    let zipcode = edited.general.zipcode.trim();
    if !zipcode.is_empty() && zwcore::region::digits(zipcode).is_none() {
        form.hint("7桁の数字で入力してください");
    }
    form.section("受信機の識別");
    form.key("data.maker_id");
    form.text_sized(
        "メーカーID",
        &mut edited.data.maker_id,
        &loaded.data.maker_id,
        "",
        120.0,
    );
    if !is_hex(&edited.data.maker_id, 2) {
        form.hint("16 進 2 桁で入力してください");
    }
    form.key("data.maker_oui");
    form.text_sized(
        "メーカー OUI",
        &mut edited.data.maker_oui,
        &loaded.data.maker_oui,
        "",
        120.0,
    );
    if !is_hex(&edited.data.maker_oui, 6) {
        form.hint("16 進 6 桁で入力してください");
    }
    form.key("data.model_id");
    form.text_sized(
        "機種名",
        &mut edited.data.model_id,
        &loaded.data.model_id,
        "",
        200.0,
    );
    let model = edited.data.model_id.trim();
    if model.chars().count() > 10 || !model.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        form.hint("英数字と _ で 10 文字までで入力してください");
    }
}

fn is_hex(value: &str, digits: usize) -> bool {
    let value = value.trim();
    value.is_empty() || (value.len() == digits && value.chars().all(|c| c.is_ascii_hexdigit()))
}

pub(super) fn operation(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) {
    form.section("マウスホイール");
    form.key("operation.wheel");
    form.select(
        "ホイール",
        &mut edited.operation.wheel,
        &loaded.operation.wheel,
        WHEEL,
    );
    form.key("operation.wheel_shift");
    form.select(
        "Shift ＋ ホイール",
        &mut edited.operation.wheel_shift,
        &loaded.operation.wheel_shift,
        WHEEL,
    );
    form.key("operation.wheel_ctrl");
    form.select(
        "Ctrl ＋ ホイール",
        &mut edited.operation.wheel_ctrl,
        &loaded.operation.wheel_ctrl,
        WHEEL,
    );
    form.section("マウスのクリック");
    let named: Vec<(String, String)> = [
        (String::from("nothing"), String::from("何もしない")),
        (String::from("menu"), String::from("メニュー")),
    ]
    .into_iter()
    .chain(
        crate::keys::Action::all()
            .into_iter()
            .map(|action| (action.name(), action.label())),
    )
    .collect();
    let clicks: Vec<(&str, &str)> = named
        .iter()
        .map(|(name, label)| (name.as_str(), label.as_str()))
        .collect();
    form.key("operation.double_click");
    form.select(
        "ダブルクリック",
        &mut edited.operation.double_click,
        &loaded.operation.double_click,
        &clicks,
    );
    form.key("operation.middle_click");
    form.select(
        "中クリック",
        &mut edited.operation.middle_click,
        &loaded.operation.middle_click,
        &clicks,
    );
    form.key("operation.right_click");
    form.select(
        "右クリック",
        &mut edited.operation.right_click,
        &loaded.operation.right_click,
        &clicks,
    );
    form.section("チャンネル送り");
    form.key("operation.channel_order");
    form.select(
        "送る順",
        &mut edited.operation.channel_order,
        &loaded.operation.channel_order,
        CHANNEL_ORDER,
    );
    form.key("operation.channel_step_skips_sub");
    form.check(
        "",
        "サブチャンネルを飛ばす",
        &mut edited.operation.channel_step_skips_sub,
        loaded.operation.channel_step_skips_sub,
    );
    form.section("音量");
    form.key("audio.volume_step");
    form.slider(
        "1 回で変える量",
        &mut edited.audio.volume_step,
        loaded.audio.volume_step,
        1..=100,
        "%",
    );
}

/// デインターレースはチャンネルを開き直し、レンダリングの方式とバックエンドは再起動して反映する。
pub(super) fn video(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File, gpus: &[String]) {
    form.section("デインターレース（ZeroWave）");
    form.key("video.deinterlace.method");
    form.select(
        "方式",
        &mut edited.video.deinterlace.method,
        &loaded.video.deinterlace.method,
        DEINTERLACE,
    );
    form.key("video.deinterlace.adapt_progressive");
    form.check(
        "",
        "インターレースのみ処理",
        &mut edited.video.deinterlace.adapt_progressive,
        loaded.video.deinterlace.adapt_progressive,
    );
    form.key("video.deinterlace.telecine");
    form.check(
        "",
        "24p の映像を検出したら逆テレシネする",
        &mut edited.video.deinterlace.telecine,
        loaded.video.deinterlace.telecine,
    );
    form.section("色調補正");
    let picture = &mut edited.video.picture;
    let was = &loaded.video.picture;
    form.key("video.picture.brightness");
    form.slider_reset(
        "明るさ",
        &mut picture.brightness,
        was.brightness,
        -100..=100,
        "",
        0,
    );
    form.key("video.picture.contrast");
    form.slider_reset(
        "コントラスト",
        &mut picture.contrast,
        was.contrast,
        -100..=100,
        "",
        0,
    );
    form.key("video.picture.hue");
    form.slider_reset("色相", &mut picture.hue, was.hue, -180..=180, "", 0);
    form.key("video.picture.saturation");
    form.slider_reset(
        "彩度",
        &mut picture.saturation,
        was.saturation,
        -100..=100,
        "",
        0,
    );
    form.section("拡大縮小");
    form.key("video.scale.upscale");
    form.select(
        "拡大",
        &mut edited.video.scale.upscale,
        &loaded.video.scale.upscale,
        SCALER,
    );
    form.key("video.scale.downscale");
    form.select(
        "縮小",
        &mut edited.video.scale.downscale,
        &loaded.video.scale.downscale,
        SCALER_DOWN,
    );
    form.key("video.scale.antiring");
    form.check(
        "",
        "アンチエイリアシングを有効にする（Lanczos3・Spline36・Catmull-Rom）",
        &mut edited.video.scale.antiring,
        loaded.video.scale.antiring,
    );
    form.section("レンダリング");
    form.key("video.render.render");
    form.select(
        "レンダラー",
        &mut edited.video.render.render,
        &loaded.video.render.render,
        RENDER,
    );
    form.key("video.render.backend");
    form.select(
        "グラフィックス API",
        &mut edited.video.render.backend,
        &loaded.video.render.backend,
        BACKEND,
    );
    // GPU は名前で選ぶ。無いときも選んであった名を残して見せる。
    let mut choices: Vec<(&str, &str)> = vec![("auto", "自動（外部GPU優先）")];
    choices.extend(gpus.iter().map(|name| (name.as_str(), name.as_str())));
    form.key("video.render.gpu");
    form.select(
        "GPU",
        &mut edited.video.render.gpu,
        &loaded.video.render.gpu,
        &choices,
    );
    form.key("video.render.present");
    form.select(
        "垂直同期",
        &mut edited.video.render.present,
        &loaded.video.render.present,
        PRESENT,
    );
    form.key("video.render.hdr");
    form.select(
        "HDR",
        &mut edited.video.render.hdr,
        &loaded.video.render.hdr,
        HDR,
    );
    form.key("video.render.queue");
    form.slider(
        "遅延フレーム数",
        &mut edited.video.render.queue,
        loaded.video.render.queue,
        1..=10,
        "フレーム",
    );
}

pub(super) fn audio(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    loaded: &File,
    devices: &[String],
) -> Option<Pressed> {
    form.section("出力");
    let mut choices: Vec<(&str, &str)> = vec![("", "既定のデバイス")];
    choices.extend(devices.iter().map(|name| (name.as_str(), name.as_str())));
    form.key("audio.device");
    form.select(
        "出力先",
        &mut edited.audio.device,
        &loaded.audio.device,
        &choices,
    );
    if edited.audio.device != loaded.audio.device {
        form.hint("出力先は次の起動から使います");
    }
    form.key("audio.passthrough");
    form.select(
        "パススルー出力",
        &mut edited.audio.passthrough,
        &loaded.audio.passthrough,
        PASSTHROUGH,
    );
    form.key("audio.latency");
    form.slider(
        "音ずれ補正",
        &mut edited.audio.latency,
        loaded.audio.latency,
        -1000..=1000,
        "ms",
    );
    form.key("audio.reset_delay_on_change");
    form.check(
        "",
        "チャンネルを変えたら音ずれ補正を 0 に戻す",
        &mut edited.audio.reset_delay_on_change,
        loaded.audio.reset_delay_on_change,
    );
    form.section("モノラル・デュアルモノラル");
    form.key("audio.mono_to_stereo");
    form.check(
        "",
        "モノラル音声をステレオ化する",
        &mut edited.audio.mono_to_stereo,
        loaded.audio.mono_to_stereo,
    );
    form.key("audio.dual_mono");
    form.select(
        "デュアルモノラル",
        &mut edited.audio.dual_mono,
        &loaded.audio.dual_mono,
        DUAL_MONO,
    );
    form.section("サラウンド");
    form.key("audio.downmix");
    form.select(
        "5.1ch 音声",
        &mut edited.audio.downmix,
        &loaded.audio.downmix,
        DOWNMIX,
    );
    form.key("audio.downmix_22");
    form.select(
        "22.2ch 音声",
        &mut edited.audio.downmix_22,
        &loaded.audio.downmix_22,
        DOWNMIX_22,
    );
    let said = |custom: bool| if custom { "カスタム" } else { "既定" };
    let mut pressed = None;
    let custom =
        edited.audio.matrix.contains_key("downmix") || edited.audio.matrix.contains_key("surround");
    if form.told_with_button(
        "5.1ch の係数",
        said(custom),
        ("audio.five_one", 0),
        "変更…",
        72.0,
    ) {
        pressed = Some(Pressed::OpenFiveOne);
    }
    if form.told_with_button(
        "22.2ch の係数",
        said(edited.audio.matrix.contains_key("seven_one")),
        ("audio.twenty_two", 0),
        "変更…",
        72.0,
    ) {
        pressed = Some(Pressed::OpenTwentyTwo);
    }
    pressed
}

pub(super) fn display(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    loaded: &File,
) -> Option<Pressed> {
    let mut pressed = None;
    form.section("表示");
    form.key("ui.theme");
    form.select("テーマ", &mut edited.ui.theme, &loaded.ui.theme, THEME);
    form.key("general.hide_cursor");
    form.slider(
        "カーソルを隠すまで",
        &mut edited.general.hide_cursor,
        loaded.general.hide_cursor,
        0..=60,
        "秒",
    );
    form.key("general.title");
    form.text(
        "タイトルバー",
        &mut edited.general.title,
        &loaded.general.title,
        "{channel} {event}",
    );
    form.hint("{channel} はチャンネル名、{event} は番組名に置き換わります");
    // macOS のメニューは OS の物で、隠せない。
    #[cfg(not(target_os = "macos"))]
    {
        form.section("メニューバー");
        form.key("ui.menu_bar");
        form.check(
            "",
            "メニューバーを表示する",
            &mut edited.ui.menu_bar,
            loaded.ui.menu_bar,
        );
    }
    form.section("ステータスバー");
    form.key("ui.status_bar");
    form.check(
        "",
        "ステータスバーを表示する",
        &mut edited.ui.status_bar,
        loaded.ui.status_bar,
    );
    form.key("ui.status_font");
    let auto = auto_font(&ui_font_now(edited));
    if let Some(open) = font_row(
        form,
        "フォント",
        &mut edited.ui.status_font,
        &loaded.ui.status_font,
        &auto,
        "ui.status_font",
    ) {
        pressed = Some(open);
    }
    form.key("ui.status_font_size");
    form.slider(
        "大きさ",
        &mut edited.ui.status_font_size,
        loaded.ui.status_font_size,
        0..=24,
        "pt",
    );
    form.key("ui.status_height");
    form.slider(
        "高さ",
        &mut edited.ui.status_height,
        loaded.ui.status_height,
        0..=64,
        "px",
    );
    form.hint(
        "大きさ 0 pt で既定の文字サイズ、高さ 0 px で文字サイズに合わせた高さ（二行のときは一行の高さ）",
    );
    super::status_items::items(form, edited, loaded);
    form.hint("ウィンドウの幅に入りきらない項目は、二行目に表示しないときは表示しません");
    form.section("フォント");
    form.key("ui.font");
    let auto = auto_font(&body_font());
    if let Some(open) = font_row(
        form,
        "UI",
        &mut edited.ui.font,
        &loaded.ui.font,
        &auto,
        "ui.font",
    ) {
        pressed = Some(open);
    }
    form.key("ui.font_size");
    form.slider(
        "大きさ",
        &mut edited.ui.font_size,
        loaded.ui.font_size,
        0..=24,
        "pt",
    );
    form.hint("0 pt で既定の大きさ");
    form.section("ウィンドウ");
    form.key("video.frame.keep_aspect");
    form.check(
        "",
        "ウィンドウの縦横比を映像に合わせる",
        &mut edited.video.frame.keep_aspect,
        loaded.video.frame.keep_aspect,
    );
    form.hint("Ctrl を押しながら大きさを変えると、この設定の逆になります");
    form.key("video.frame.fullscreen");
    form.select(
        "全画面表示",
        &mut edited.video.frame.fullscreen,
        &loaded.video.frame.fullscreen,
        FULLSCREEN,
    );
    form.key("video.frame.aspect");
    let aspects = super::pans::aspect_choices(ASPECT, &edited.video.pans);
    let aspects: Vec<(&str, &str)> = aspects
        .iter()
        .map(|(spelled, name)| (spelled.as_str(), name.as_str()))
        .collect();
    form.select(
        "比率",
        &mut edited.video.frame.aspect,
        &loaded.video.frame.aspect,
        &aspects,
    );
    form.key("video.frame.side_cut");
    form.check(
        "",
        "切り取った所を黒く隠さない",
        &mut edited.video.frame.side_cut,
        loaded.video.frame.side_cut,
    );
    super::pans::pans(form, edited, loaded);
    pressed
}

pub(super) fn osd(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) -> Option<Pressed> {
    let mut pressed = None;
    form.section("OSD");
    form.key("osd.enabled");
    form.check(
        "",
        "OSD を表示する",
        &mut edited.osd.enabled,
        loaded.osd.enabled,
    );
    form.key("osd.opacity");
    form.slider(
        "不透明度",
        &mut edited.osd.opacity,
        loaded.osd.opacity,
        0..=100,
        "%",
    );
    form.key("osd.fade");
    form.slider(
        "表示時間",
        &mut edited.osd.fade,
        loaded.osd.fade,
        1..=30,
        "秒",
    );
    form.key("osd.font");
    let auto = auto_font(&ui_font_now(edited));
    if let Some(open) = font_row(
        form,
        "フォント",
        &mut edited.osd.font,
        &loaded.osd.font,
        &auto,
        "osd.font",
    ) {
        pressed = Some(open);
    }
    form.key("osd.channel");
    form.select(
        "チャンネル切替時",
        &mut edited.osd.channel,
        &loaded.osd.channel,
        OSD_CHANNEL,
    );
    form.section("番組情報");
    form.key("osd.event");
    form.slider(
        "表示時間",
        &mut edited.osd.event,
        loaded.osd.event,
        0..=60,
        "秒",
    );
    for (flag, said) in [
        ("channel", "チャンネル切替時に表示する"),
        ("event", "番組変更時に表示する"),
    ] {
        let mut on = edited.osd.event_on.iter().any(|s| s == flag);
        let was = loaded.osd.event_on.iter().any(|s| s == flag);
        form.key("osd.event_on");
        form.check("", said, &mut on, was);
        let has = edited.osd.event_on.iter().any(|s| s == flag);
        if on != has {
            if on {
                edited.osd.event_on.push(flag.to_owned());
            } else {
                edited.osd.event_on.retain(|s| s != flag);
            }
        }
    }
    pressed
}

pub(super) fn caption(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    loaded: &File,
) -> Option<Pressed> {
    let mut pressed = None;
    form.section("表示");
    form.key("caption.shown");
    form.select(
        "字幕",
        &mut edited.caption.shown,
        &loaded.caption.shown,
        CAPTION_SHOWN,
    );
    form.key("caption.forced");
    form.check(
        "",
        "強制表示の字幕を表示する",
        &mut edited.caption.forced,
        loaded.caption.forced,
    );
    form.key("caption.superimpose");
    form.select(
        "文字スーパー",
        &mut edited.caption.superimpose,
        &loaded.caption.superimpose,
        CAPTION_SHOWN,
    );
    form.key("caption.superimpose_forced");
    form.check(
        "",
        "強制表示の文字スーパーを表示する",
        &mut edited.caption.superimpose_forced,
        loaded.caption.superimpose_forced,
    );
    form.hint("第2言語が無い番組では、第1言語を表示します");
    // 字幕データは「背景は黒、縁取りは黒」と指定しているものとしてレンダリングする。
    form.section("サンプル");
    let area = form.line(76.0);
    let area = egui::Rect::from_min_max(
        egui::pos2(area.left() + 6.0, area.top()),
        egui::pos2(area.right(), area.bottom()),
    );
    let look = form.look;
    let painter = form.ui.painter().with_clip_rect(area);
    painter.rect_filled(area, 6.0, look.sample_ground);
    painter.rect_stroke(
        area,
        6.0,
        egui::Stroke::new(1.0, look.sample_edge),
        egui::StrokeKind::Inside,
    );
    let decor = crate::ui::overlay::caption::Decor::of(&edited.caption, false);
    crate::ui::overlay::caption::sample(&painter, area, &decor, 22.0);
    form.section("フォント");
    form.key("caption.font");
    let auto_ui = ui_font_now(edited);
    let auto = auto_font(&auto_ui);
    if let Some(open) = font_row(
        form,
        "字幕",
        &mut edited.caption.font,
        &loaded.caption.font,
        &auto,
        "caption.font",
    ) {
        pressed = Some(open);
    }
    form.hint("大きさは映像に合わせて決まるので、選んだフォントの名前だけを使います");
    form.key("caption.superimpose_font");
    let auto = auto_font(if edited.caption.font.is_empty() {
        &auto_ui
    } else {
        &edited.caption.font
    });
    if let Some(open) = font_row(
        form,
        "文字スーパー",
        &mut edited.caption.superimpose_font,
        &loaded.caption.superimpose_font,
        &auto,
        "caption.superimpose_font",
    ) {
        pressed = Some(open);
    }
    form.key("caption.fallback");
    form.check(
        "",
        "フォントに含まれない文字は既定のフォントで表示する",
        &mut edited.caption.fallback,
        loaded.caption.fallback,
    );
    form.section("背景");
    form.key("caption.background");
    form.select(
        "背景",
        &mut edited.caption.background,
        &loaded.caption.background,
        CAPTION_BACKGROUND,
    );
    form.key("caption.opacity");
    form.slider_reset(
        "不透明度",
        &mut edited.caption.opacity,
        loaded.caption.opacity,
        0..=100,
        "%",
        crate::config::file::CAPTION_OPACITY,
    );
    form.hint("100 % で不透明（黒なら真っ黒）、0 % で透明");
    form.section("縁取り");
    form.key("caption.outline");
    form.select(
        "縁取り",
        &mut edited.caption.outline,
        &loaded.caption.outline,
        CAPTION_OUTLINE,
    );
    form.key("caption.outline_width");
    form.slider(
        "太さ",
        &mut edited.caption.outline_width,
        loaded.caption.outline_width,
        0..=20,
        "%",
    );
    form.hint("0 % で字幕データの太さのまま");
    pressed
}

pub(super) fn epg(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) -> Option<Pressed> {
    let mut pressed = None;
    form.section("番組表");
    form.key("guide.hours");
    form.slider(
        "1 画面の時間幅",
        &mut edited.guide.hours,
        loaded.guide.hours,
        1..=24,
        "時間",
    );
    form.key("guide.offset");
    form.slider(
        "時差（UTC から）",
        &mut edited.guide.offset,
        loaded.guide.offset,
        -12..=14,
        "時間",
    );
    form.section("保存");
    form.key("guide.cache");
    form.check(
        "",
        "番組表データを保存する",
        &mut edited.guide.cache,
        loaded.guide.cache,
    );
    form.key("guide.logo");
    form.check(
        "",
        "局ロゴデータを保存する",
        &mut edited.guide.logo,
        loaded.guide.logo,
    );
    form.section("フォント");
    form.key("guide.font");
    let auto = auto_font(&ui_font_now(edited));
    if let Some(open) = font_row(
        form,
        "番組表",
        &mut edited.guide.font,
        &loaded.guide.font,
        &auto,
        "guide.font",
    ) {
        pressed = Some(open);
    }
    form.key("guide.font_size");
    form.slider(
        "大きさ",
        &mut edited.guide.font_size,
        loaded.guide.font_size,
        0..=24,
        "pt",
    );
    form.hint("0 pt で既定の大きさ。番組説明のウィンドウも同じフォントです");
    form.section("番組の詳細");
    extended_networks(form, edited, loaded);
    pressed
}

/// 残すのはネットワーク識別子（名前は書き換えられるため）。全部入っていれば `["*"]` にまとめる。
fn extended_networks(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) {
    use zwcore::guide::Band;

    let bands: Vec<(String, Vec<(u16, u16)>)> = Band::all()
        .into_iter()
        .filter(|band| *band != Band::OTHER)
        .map(|band| (band.name(), band.ranges()))
        .collect();
    let now = crate::config::apply::extended(edited);
    let was = crate::config::apply::extended(loaded);
    let items: Vec<(String, bool, bool)> = bands
        .iter()
        .map(|(name, ranges)| {
            (
                name.clone(),
                covered(now.as_deref(), ranges),
                covered(was.as_deref(), ranges),
            )
        })
        .collect();
    form.key("guide.extended");
    let hit = form.chips("番組詳細情報の取得", &items);
    if let Some(hit) = hit {
        let mut on: Vec<bool> = items.iter().map(|(_, on, _)| *on).collect();
        on[hit] = !on[hit];
        edited.guide.extended = if on.iter().all(|on| *on) {
            vec![toml::Value::String(String::from("*"))]
        } else {
            bands
                .iter()
                .zip(&on)
                .filter(|(_, on)| **on)
                .flat_map(|((_, ranges), _)| ranges.iter().map(|range| nid_value(*range)))
                .collect()
        };
    }
}

fn covered(reading: Option<&[(u16, u16)]>, ranges: &[(u16, u16)]) -> bool {
    let Some(reading) = reading else {
        return true;
    };
    !ranges.is_empty()
        && ranges.iter().all(|(from, to)| {
            reading
                .iter()
                .any(|(start, end)| start <= from && to <= end)
        })
}

fn nid_value(range: (u16, u16)) -> toml::Value {
    let (from, to) = range;
    if from == 0 && to == u16::MAX {
        toml::Value::String(String::from("*"))
    } else if from == to {
        toml::Value::Integer(i64::from(from))
    } else {
        toml::Value::String(format!("0x{from:04x}-0x{to:04x}"))
    }
}

pub(super) fn recording(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    loaded: &File,
) -> Option<Pressed> {
    let mut pressed = None;
    form.section("保存先");
    form.key("record.folder");
    let (_, browse) = form.program(
        "保存先フォルダ",
        &mut edited.record.folder,
        &loaded.record.folder,
        "（空欄ならビデオフォルダ）",
        "record.folder",
    );
    if browse {
        pressed = Some(Pressed::BrowseFolder(
            "record.folder",
            edited.record.folder.clone(),
        ));
    }
    form.key("record.name");
    form.text(
        "ファイル名",
        &mut edited.record.name,
        &loaded.record.name,
        "{date}_{channel}_{event}",
    );
    let preview = zwcore::record::file_name(
        zwcore::record::without_extension(&edited.record.name),
        PREVIEW_CHANNEL,
        PREVIEW_EVENT,
        edited.guide.offset,
    );
    form.told("プレビュー", &format!("{preview}.ts"));
    form.hint(
        "{date} 日付、{time} 時刻、{channel} チャンネル名、{event} 番組名。拡張子は自動で付きます（TS は .ts、MMT/TLV は .mmts）。同じ名前のファイルがあるときは、末尾に (1) などを付けて保存します",
    );
    form.section("保存するデータ");
    form.key("record.service_only");
    form.check(
        "",
        "現在のサービスのみ保存する",
        &mut edited.record.service_only,
        loaded.record.service_only,
    );
    form.key("record.caption");
    form.check(
        "",
        "字幕データを保存する",
        &mut edited.record.caption,
        loaded.record.caption,
    );
    form.key("record.carousel");
    form.check(
        "",
        "データ放送を保存する",
        &mut edited.record.carousel,
        loaded.record.carousel,
    );
    form.key("record.eit");
    form.check(
        "",
        "番組情報（EIT）を保存する",
        &mut edited.record.eit,
        loaded.record.eit,
    );
    form.section("確認");
    form.key("record.confirm_channel_change");
    form.check(
        "",
        "録画中のチャンネル変更は確認を取る",
        &mut edited.record.confirm_channel_change,
        loaded.record.confirm_channel_change,
    );
    form.key("record.confirm_exit");
    form.check(
        "",
        "録画中の終了は確認を取る",
        &mut edited.record.confirm_exit,
        loaded.record.confirm_exit,
    );
    form.key("record.confirm_stop");
    form.check(
        "",
        "録画停止は確認を取る",
        &mut edited.record.confirm_stop,
        loaded.record.confirm_stop,
    );
    if edited.record.confirm_stop {
        form.key("record.confirm_stop_status_bar_only");
        form.check(
            "",
            "ステータスバーでの録画停止のみ確認を取る",
            &mut edited.record.confirm_stop_status_bar_only,
            loaded.record.confirm_stop_status_bar_only,
        );
    }
    form.section("書き出し");
    form.key("record.write_unit");
    form.slider_reset(
        "ファイル書き出し単位",
        &mut edited.record.write_unit,
        loaded.record.write_unit,
        1..=32_768,
        "KiB",
        crate::config::file::RECORD_WRITE_UNIT,
    );
    form.key("record.max_pending");
    form.slider_reset(
        "書き出し待ちのサイズ制限",
        &mut edited.record.max_pending,
        loaded.record.max_pending,
        32..=1024,
        "MiB",
        crate::config::file::RECORD_MAX_PENDING,
    );
    form.hint("書き込みが追いつかずに書き出し待ちがこの大きさを超えると、古いデータから破棄します");
    pressed
}

pub(super) fn capture(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    loaded: &File,
    layers: &[(String, String)],
) -> Option<Pressed> {
    let mut pressed = None;
    form.section("保存先");
    form.key("capture.folder");
    let (_, browse) = form.program(
        "保存先フォルダ",
        &mut edited.capture.folder,
        &loaded.capture.folder,
        "（空欄ならピクチャフォルダ）",
        "capture.folder",
    );
    if browse {
        pressed = Some(Pressed::BrowseFolder(
            "capture.folder",
            edited.capture.folder.clone(),
        ));
    }
    form.key("capture.name");
    form.text(
        "ファイル名",
        &mut edited.capture.name,
        &loaded.capture.name,
        "{date}_{time}_{channel}",
    );
    let preview = zwcore::record::file_name(
        &edited.capture.name,
        PREVIEW_CHANNEL,
        PREVIEW_EVENT,
        edited.guide.offset,
    );
    form.told(
        "プレビュー",
        &format!(
            "{preview}.{}",
            crate::capture::extension(&edited.capture.format)
        ),
    );
    form.hint("{date} 日付、{time} 時刻、{channel} チャンネル名、{event} 番組名");
    form.section("画像");
    form.key("capture.size");
    form.select(
        "大きさ",
        &mut edited.capture.size,
        &loaded.capture.size,
        CAPTURE_SIZE,
    );
    form.key("capture.format");
    form.select(
        "保存形式",
        &mut edited.capture.format,
        &loaded.capture.format,
        CAPTURE_FORMAT,
    );
    form.key("capture.jpeg_quality");
    form.slider_reset(
        "JPEG 品質",
        &mut edited.capture.jpeg_quality,
        loaded.capture.jpeg_quality,
        0..=100,
        "",
        crate::config::file::CAPTURE_JPEG_QUALITY,
    );
    form.key("capture.png_level");
    form.slider_reset(
        "PNG 圧縮レベル",
        &mut edited.capture.png_level,
        loaded.capture.png_level,
        0..=9,
        "",
        crate::config::file::CAPTURE_PNG_LEVEL,
    );
    form.key("capture.icon_saves");
    form.check(
        "",
        "キャプチャボタンでファイルに保存する",
        &mut edited.capture.icon_saves,
        loaded.capture.icon_saves,
    );
    form.hint("オフにすると、ステータスバーのキャプチャボタンでクリップボードにコピーします");
    form.key("capture.hdr");
    form.check(
        "",
        "HDR 映像は HDR のまま保存する",
        &mut edited.capture.hdr,
        loaded.capture.hdr,
    );
    form.hint(
        "保存形式が PNG で、HDR 出力しているときに有効です。16 ビットの PNG に、PQ か HLG を示す cICP チャンクを付けて保存します",
    );
    form.section("レイヤー設定");
    form.note("上にあるものほど手前に重なります。プラグインが表示するものは、読み込まれているものがここに並びます");
    form.key("capture.include");
    let mut rows: Vec<super::form::Layer> = crate::capture::layer_rows(layers)
        .into_iter()
        .map(|(key, name)| {
            let plugin = key.starts_with(&crate::capture::plugin_key(""))
                || key == crate::capture::VIDEO_FILTER;
            let name = name
                .strip_suffix("（プラグイン）")
                .map_or(name.clone(), str::to_owned);
            super::form::Layer {
                on: edited.capture.includes(&key),
                was: loaded.capture.includes(&key),
                key,
                name,
                plugin,
                fixed: false,
            }
        })
        .collect();
    rows.push(super::form::Layer {
        key: String::from("video"),
        name: String::from("映像"),
        plugin: false,
        on: true,
        was: true,
        fixed: true,
    });
    form.layers(&mut rows);
    for row in rows.iter().filter(|row| !row.fixed) {
        if row.on != edited.capture.includes(&row.key) {
            edited.capture.include.insert(row.key.clone(), row.on);
        }
    }
    pressed
}

pub(super) fn playback(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) {
    form.section("バッファ");
    form.key("playback.buffer");
    form.slider(
        "最大バッファ",
        &mut edited.playback.buffer,
        loaded.playback.buffer,
        1..=30,
        "秒",
    );
    form.section("エラーカウント");
    form.key("playback.reset_errors_on_tune");
    form.check(
        "",
        "チャンネル切り替え時のエラーカウントをリセットする",
        &mut edited.playback.reset_errors_on_tune,
        loaded.playback.reset_errors_on_tune,
    );
    form.section("ワンセグ");
    form.key("playback.oneseg");
    form.select(
        "ワンセグ",
        &mut edited.playback.oneseg,
        &loaded.playback.oneseg,
        ONESEG,
    );
    form.section("起動時");
    form.key("playback.restore_mute");
    form.check(
        "",
        "消音の状態を次の起動に引き継ぐ",
        &mut edited.playback.restore_mute,
        loaded.playback.restore_mute,
    );
}

pub(super) fn log(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) -> Option<Pressed> {
    form.section("ログ");
    form.key("log.file");
    form.check(
        "",
        "ログをファイルに出力する",
        &mut edited.log.file,
        loaded.log.file,
    );
    let path = crate::config::path_of("zerowave.log");
    let shown = path
        .as_ref()
        .map_or(String::from("（書き込めません）"), |path| {
            path.display().to_string()
        });
    form.told("出力場所", &shown);
    let mut pressed = None;
    let row = form.line(super::skin::SELECT_HEIGHT);
    let folder = path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf);
    let button = egui::Rect::from_min_size(row.min, egui::vec2(120.0, row.height()));
    if form.button_enabled(
        button,
        ("log.folder", 0),
        "フォルダを開く",
        folder.is_some(),
    ) && let Some(folder) = folder
    {
        pressed = Some(Pressed::Reveal(folder));
    }
    form.key("log.keep");
    form.check(
        "",
        "同じログファイルに追記する",
        &mut edited.log.keep,
        loaded.log.keep,
    );
    form.key("log.level");
    form.select(
        "ログレベル",
        &mut edited.log.level,
        &loaded.log.level,
        LOG_LEVEL,
    );
    form.note("ログの設定は次の起動から有効になります。");
    pressed
}

fn body_font() -> String {
    crate::ui::text::font::body_name().unwrap_or_else(|| String::from("OS のフォント"))
}

fn ui_font_now(edited: &File) -> String {
    if edited.ui.font.is_empty() {
        body_font()
    } else {
        edited.ui.font.clone()
    }
}

fn auto_font(now: &str) -> String {
    format!("自動（{now}）")
}

/// 本体のフォントは名前で探すだけなので、ファイルは選ばせない。
fn font_row(
    form: &mut Form<'_, '_>,
    label: &str,
    value: &mut String,
    was: &str,
    auto: &str,
    key: &'static str,
) -> Option<Pressed> {
    let field = form.font_field(label, value, value != was, auto, false, key);
    if field.auto {
        value.clear();
    }
    field.open.then(|| Pressed::PickFont {
        target: super::font_pick::Target::Setting(key),
        place: super::font_pick::place_of(key).to_owned(),
        current: value.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_band_is_ticked_only_when_all_its_nids_are_read() {
        assert!(covered(None, &[(0x0006, 0x0007)]));
        assert!(covered(Some(&[(0x0000, u16::MAX)]), &[(0x0006, 0x0007)]));
        assert!(covered(Some(&[(0x0006, 0x0007)]), &[(0x0006, 0x0007)]));
        // 半分だけ読む設定ではオンにしない（読まない側が知らないうちに含まれる）。
        assert!(!covered(Some(&[(0x0006, 0x0006)]), &[(0x0006, 0x0007)]));
        assert!(!covered(Some(&[]), &[(0x0004, 0x0004)]));
        assert!(!covered(Some(&[(0x0000, u16::MAX)]), &[]));
    }

    #[test]
    fn a_range_is_written_the_way_nids_are_read() {
        let back = |range| crate::config::key::pattern_of(&nid_value(range)).expect("読み直せる");
        assert_eq!(nid_value((4, 4)), toml::Value::Integer(4));
        assert_eq!(back((4, 4)), crate::config::key::Pattern::One(4));
        assert_eq!(
            nid_value((0x0006, 0x0007)),
            toml::Value::String(String::from("0x0006-0x0007"))
        );
        assert_eq!(
            back((0x0006, 0x0007)),
            crate::config::key::Pattern::Range(6, 7)
        );
        assert_eq!(
            nid_value((0, u16::MAX)),
            toml::Value::String(String::from("*"))
        );
        assert_eq!(back((0, u16::MAX)), crate::config::key::Pattern::Any);
    }

    #[test]
    fn every_default_spelling_is_among_the_choices() {
        let file = File::default();
        let has = |choices: Choices, value: &str| choices.iter().any(|(s, _)| *s == value);
        assert!(has(ON_START, &file.general.on_start));
        assert!(has(ON_START_VOLUME, &file.general.on_start_volume));
        assert!(has(WHEEL, &file.operation.wheel));
        assert!(has(WHEEL, &file.operation.wheel_shift));
        assert!(has(WHEEL, &file.operation.wheel_ctrl));
        assert!(has(CHANNEL_ORDER, &file.operation.channel_order));
        assert!(has(PASSTHROUGH, &file.audio.passthrough));
        assert!(has(DUAL_MONO, &file.audio.dual_mono));
        assert!(has(DOWNMIX, &file.audio.downmix));
        assert!(has(DOWNMIX_22, &file.audio.downmix_22));
        assert!(has(DEINTERLACE, &file.video.deinterlace.method));
        assert!(has(RENDER, &file.video.render.render));
        assert!(has(BACKEND, &file.video.render.backend));
        assert!(has(PRESENT, &file.video.render.present));
        assert!(has(HDR, &file.video.render.hdr));
        assert!(has(THEME, &file.ui.theme));
        assert!(has(OSD_CHANNEL, &file.osd.channel));
        assert!(has(CAPTION_SHOWN, &file.caption.shown));
        assert!(has(CAPTION_SHOWN, &file.caption.superimpose));
        assert!(has(CAPTION_BACKGROUND, &file.caption.background));
        assert!(has(CAPTION_OUTLINE, &file.caption.outline));
        assert!(has(LOG_LEVEL, &file.log.level));
        assert!(has(CAPTURE_FORMAT, &file.capture.format));
        assert!(has(CAPTURE_SIZE, &file.capture.size));
        assert!(has(ONESEG, &file.playback.oneseg));
    }
}
