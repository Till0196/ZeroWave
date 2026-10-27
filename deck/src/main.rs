// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ZeroWave のデスクトップ側。録画や放送を開いて映して鳴らす。
//! スレッドはメイン（ウィンドウ・入力・画面）、再生（`stage`。デコード・時計・選局・映像）、別ウィンドウ、番組表、音声の五本。
//! 再生はイベントループの上で回さない（Windows はメニューやウィンドウの移動の間ループを返さない）。

// Windows：ダブルクリックで端末を開かない（端末から起動されたら `log::attach` が繋ぎ直す）。
#![windows_subsystem = "windows"]

mod audio;
mod awake;
mod capture;
mod check;
mod choice;
mod coaxd;
mod config;
mod deck;
mod draw;
mod keys;
mod log;
mod machine;
mod pcsc;
mod plugin_ui;
mod plugins;
mod scan;
mod stage;
mod state;
mod threads;
mod ui;
mod window;

/// macOS: Info.plist を実行ファイルに埋め込む（リンカの `-sectcreate` は zig が対応していない）。
#[cfg(target_os = "macos")]
#[unsafe(link_section = "__TEXT,__info_plist")]
#[used]
static INFO_PLIST: [u8; include_bytes!("../mac/Info.plist").len()] =
    *include_bytes!("../mac/Info.plist");

use std::sync::Arc;
use std::sync::mpsc;

use winit::event_loop::{ControlFlow, EventLoop};
use zwcore::input::source::Source;

use audio::device as sound;
use choice::{Choices, Render};
use stage::Live;
use threads::link::Wake;
use zwcore::player::Player;
use zwcore::player::supply::Supply;
use zwcore::sound::{Device, Passthrough};

#[cfg(feature = "sound")]
fn open_pcm(named: Option<&str>) -> (Option<Device>, sound::Keep) {
    match sound::Pcm::open(48_000, None, named) {
        Ok((device, keep)) => (Some(Device::Pcm(Box::new(device))), keep),
        Err(error) => {
            config::notice(stage::said_with("音声デバイスを開けません。", &error));
            (None, sound::Keep::none())
        }
    }
}

#[cfg(not(feature = "sound"))]
fn open_pcm(_named: Option<&str>) -> (Option<Device>, sound::Keep) {
    (None, sound::Keep::none())
}

use stage::Pick;

/// X11 の画面の倍率を macOS・Windows と同じ決め方に（`Xft.dpi` が無ければ 1.0）。
#[cfg(target_os = "linux")]
fn settle_x11_scale() {
    if std::env::var_os("WINIT_X11_SCALE_FACTOR").is_some()
        || std::env::var_os("DISPLAY").is_none()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        return;
    }
    let xft_dpi = || -> Option<f64> {
        let (connection, _) = x11rb::connect(None).ok()?;
        let database = x11rb::resource_manager::new_from_default(&connection).ok()?;
        database
            .get_string("Xft.dpi", "")?
            .trim()
            .parse::<f64>()
            .ok()
    };
    match xft_dpi() {
        Some(dpi) => tracing::info!("画面の倍率 {:.2}（Xft.dpi {dpi}）", dpi / 96.0),
        None => {
            tracing::info!("画面の倍率 1.00（Xft.dpi が無い）");
            // SAFETY: main の先頭で、まだスレッドを一つも起動していない。
            unsafe { std::env::set_var("WINIT_X11_SCALE_FACTOR", "1.0") };
        }
    }
}

fn main() {
    zwcore::memory::few_arenas();
    // D3D12 のビデオキューを試すためだけに起動された子プロセスなら、試して終わる（親のログを上書きする前に）。
    #[cfg(windows)]
    if let Some(code) = zwcore::decode::d3d12::dx::probe_child() {
        std::process::exit(code);
    }
    let logged = log::attach();
    log::init();
    tracing::info!(
        "{}（ビルド日 {}）",
        format!("ZeroWave {}", env!("ZW_BUILD")).trim_end(),
        machine::local_time(env!("ZW_BUILT_AT").parse().unwrap_or(0))
    );
    #[cfg(target_os = "linux")]
    settle_x11_scale();
    #[cfg(windows)]
    zwcore::decode::d3d12::dx::enable_dred();
    #[cfg(windows)]
    zwcore::decode::d3d12::dx::enable_child_probe();
    if let Some(path) = &logged {
        tracing::info!("ログファイル {}", path.display());
    }
    coax::http::name_agent(concat!("ZeroWave/", env!("CARGO_PKG_VERSION")));
    let _finish = Finish;
    let mut path = None;
    let mut program: Option<u16> = None;
    let mut mirakurun: Option<String> = None;
    let mut mirakurun_by = zwcore::input::source::mirakurun::By::Auto;
    let mut coaxd: Option<(String, i32, Vec<String>, String)> = None;
    let mut scanning: Option<String> = None;
    let mut named = stage::Named::default();
    let mut want_sound = true;
    let mut passthrough = false;
    let mut sound_device: Option<String> = None;
    let mut checking = Vec::new();
    let mut dumping: Option<String> = None;
    let mut against: Option<String> = None;
    let mut sound_checking: Option<String> = None;
    let mut frames = 120usize;
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let mut sets = Vec::new();
    let mut profile_flag: Option<String> = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--program" => program = arguments.next().and_then(|id| id.parse().ok()),
            "--mirakurun" => mirakurun = arguments.next(),
            "--mirakurun-stream" => {
                let value = arguments.next().unwrap_or_default();
                match zwcore::input::source::mirakurun::By::parse(&value) {
                    Some(by) => mirakurun_by = by,
                    None => {
                        tracing::warn!("--mirakurun-stream の値が不正: {value}");
                        config::notice(
                            "コマンドライン引数 --mirakurun-stream の値が正しくありません。既定の値で起動しました。\n指定できる値：auto・channel・service",
                        );
                    }
                }
            }
            "--nid" | "--tsid" | "--sid" | "--service" => {
                let value = arguments.next().unwrap_or_default();
                let id = value
                    .strip_prefix("0x")
                    .map(|digits| u16::from_str_radix(digits, 16).ok())
                    .unwrap_or_else(|| value.parse().ok());
                if id.is_none() {
                    tracing::warn!("{argument} の値が不正: {value}");
                    config::notice(format!(
                        "コマンドライン引数 {argument} の値が正しくありません。\n十進か 0x で始まる十六進で指定してください。"
                    ));
                }
                match argument.as_str() {
                    "--nid" => named.network = id,
                    "--tsid" => named.transport = id,
                    _ => named.service = id,
                }
            }
            "--no-sound" => want_sound = false,
            // パススルーできない音声では止まる（黙って PCM に変換すると区別できない）。
            "--passthrough" => passthrough = true,
            "--fit-window" => flags.push((argument, None)),
            "--set" => sets.extend(arguments.next()),
            "--profile" => profile_flag = arguments.next(),
            "--sound-device" => sound_device = arguments.next(),
            "--check" => checking.push(String::new()),
            "--decoders" => {
                for (codec, name) in [
                    (mediadecoder::es::Codec::Mpeg2, "MPEG-2"),
                    (mediadecoder::es::Codec::H264, "H.264"),
                    (mediadecoder::es::Codec::Hevc, "H.265"),
                ] {
                    println!("{name}");
                    for offer in zwcore::decode::offers(codec) {
                        let ways: Vec<&str> = offer.ways.iter().map(|way| way.name()).collect();
                        println!(
                            "  {}  {}  {}{}",
                            if offer.device.is_empty() {
                                "（このビルドの経路）"
                            } else {
                                &offer.device
                            },
                            ways.join(" / "),
                            if offer.gpu { "GPU" } else { "CPU" },
                            if offer.draws {
                                "（レンダリングに使用）"
                            } else {
                                ""
                            }
                        );
                    }
                    if zwcore::decode::has_software(codec) {
                        println!("  ZeroWave  —  CPU");
                    }
                }
                for codec in [
                    zwcore::decode::sound::SoundCodec::Aac,
                    zwcore::decode::sound::SoundCodec::Mpeg,
                ] {
                    println!("{}", codec.name());
                    for name in zwcore::decode::sound::offers(codec) {
                        println!("  {name}  OS  CPU");
                    }
                    println!("  ZeroWave  —  CPU");
                }
                leave(0);
            }
            "--scan" => scanning = Some(arguments.next().unwrap_or_default()),
            "--decoder" | "--decoder-api" | "--deinterlace" | "--deinterlace-by" | "--render"
            | "--backend" | "--gpu" | "--audio" | "--audio-latency" | "--dual-mono" | "--video"
            | "--present" | "--hdr" | "--upscale" | "--downscale" | "--antiring" => {
                let value = arguments.next();
                flags.push((argument, value));
            }
            "--dump" => {
                dumping = arguments.next();
            }
            "--against" => {
                against = arguments.next();
            }
            "--sound-check" => {
                sound_checking = arguments.next();
            }
            "--frames" => {
                frames = arguments
                    .next()
                    .and_then(|count| count.parse().ok())
                    .unwrap_or(frames);
            }
            other if !checking.is_empty() => checking.push(other.to_owned()),
            other => path = Some(other.to_owned()),
        }
    }
    let config = config::Config::load_forgiving(&sets);
    let profile = config.profile_name(profile_flag.as_deref());
    let file = config.effective(&profile).unwrap_or_else(|why| {
        config::notice(stage::said_with(
            "設定を読み込めませんでした。既定の設定で起動しました。",
            &why,
        ));
        config::File::default()
    });
    // ロックはどの deck も取りに行く（設定が偽の deck が先に動いていても分かるように）。
    let sole = config::claim_single();
    // 録画のファイルだけを渡され、ほかの deck が動いていれば、そちらで開いて終わる（macOS と同じ）。
    #[cfg(any(windows, target_os = "linux"))]
    if !sole
        && mirakurun.is_none()
        && coaxd.is_none()
        && scanning.is_none()
        && checking.is_empty()
        && dumping.is_none()
        && against.is_none()
        && sound_checking.is_none()
        && let Some(file) = path.as_deref()
        && window::handoff::hand(file)
    {
        leave(0);
    }
    if file.general.single_instance && !sole {
        tracing::warn!("別の deck が起動しているため終了する（複数起動しない設定）");
        return;
    }
    let applied = match config::apply::choices(&file) {
        Ok(applied) => applied,
        Err(why) => {
            config::notice(stage::said_with(
                &format!(
                    "設定ファイル ({}) にエラーがあります。\n既定の設定で起動しました。",
                    config::FILE
                ),
                &why,
            ));
            match config::apply::choices(&config::File::default()) {
                Ok(applied) => applied,
                Err(why) => {
                    log::fatal(&why);
                    leave(2);
                }
            }
        }
    };
    zwcore::decode::set_plan(applied.plan.clone());
    zwcore::decode::sound::set_sound_plan(applied.sound_plan.clone());
    plugins::prefer(&file.data.plugins, &file.data.disabled);
    ui::theme::choose(&file.ui.theme);
    coaxd::install(file.general.launch_coaxd);
    pcsc::apply(&file);
    config::apply::mirakurun_asking(&file);
    zwcore::guide::set_shown_offset(file.guide.offset * 3600);
    let mut overridden: Vec<String> = flags
        .iter()
        .filter_map(|(flag, _)| config::apply::key_of_flag(flag))
        .map(str::to_owned)
        .collect();
    overridden
        .extend(sets.iter().filter_map(|set: &String| {
            set.split_once('=').map(|(path, _)| path.trim().to_owned())
        }));
    if passthrough {
        overridden.push(String::from("audio.passthrough"));
    }
    if sound_device.is_some() {
        overridden.push(String::from("audio.device"));
    }
    config::set_overridden(overridden);
    let mut choices = applied.choices;
    for (flag, value) in &flags {
        if let Err(why) = Choices::parse(flag, value.as_deref(), &mut choices) {
            config::notice(stage::said_with("コマンドライン引数を無視しました。", &why));
        }
    }
    if sound_device.is_none() && !applied.sound_device.is_empty() {
        sound_device = Some(applied.sound_device.clone());
    }
    let passthrough = if passthrough {
        Passthrough::On
    } else {
        applied.passthrough
    };
    choices.options.volume = applied
        .volume
        .or(config.state.volume)
        .unwrap_or(config::state::DEFAULT_VOLUME);
    choices.options.muted = applied.restore_mute && config.state.mute;
    let mut pick = if named.is_empty() {
        Pick::First
    } else {
        Pick::Named(named)
    };
    let mut idle_because: Option<String> = None;
    if path.is_none() && mirakurun.is_none() && checking.is_empty() {
        if !config.found {
            config::set_first_run();
            let placed = config::write_template();
            idle_because = Some(match placed {
                Some(placed) => format!(
                    "設定ファイルを作成しました。\n{} にソースを設定すると放送を視聴できます。",
                    placed.display()
                ),
                None => {
                    String::from("設定ファイルを作成できません。\n保存できる場所がありません。")
                }
            });
        } else {
            let (entries, unknown) = file.entries_of(&file.profile(&profile));
            for label in unknown {
                tracing::info!("プロファイルで指定した source「{label}」が [[source]] に無い");
            }
            let mut chosen = None;
            for entry in entries {
                let source = entry.source;
                match source.kind.as_str() {
                    "mirakurun" if !source.url.is_empty() => {
                        chosen = Some((source, entry.tuner));
                        break;
                    }
                    "mirakurun" => tracing::info!("{}: url が無い", source.shown()),
                    "coaxd" => {
                        chosen = Some((source, entry.tuner));
                        break;
                    }
                    other => tracing::info!("{}: 不明な kind「{other}」", source.shown()),
                }
            }
            match chosen {
                Some((source, tuner)) => {
                    tracing::info!(
                        "source {}{}",
                        source.shown(),
                        if profile.is_empty() {
                            String::new()
                        } else {
                            format!("（プロファイル {profile}）")
                        }
                    );
                    if source.kind == "coaxd" {
                        coaxd = Some((
                            source.endpoint.clone(),
                            source.priority,
                            tuner.into_iter().collect(),
                            file.profile(&profile).name,
                        ));
                    } else {
                        mirakurun = Some(source.url.clone());
                    }
                    if !source.by.is_empty() {
                        match zwcore::input::source::mirakurun::By::parse(&source.by) {
                            Some(by) => mirakurun_by = by,
                            None => {
                                tracing::info!("{}: 不明な by「{}」", source.shown(), source.by)
                            }
                        }
                    }
                    if file.general.on_start != "nothing"
                        && pick == Pick::First
                        && let Some(last) = config.state.last
                    {
                        pick = Pick::Exact(last);
                    }
                }
                None => {
                    idle_because = Some(format!(
                        "放送ソースがありません。\n設定の［ソース］でソースを追加してください。（プロファイル：{}）",
                        if profile.is_empty() {
                            "既定"
                        } else {
                            &profile
                        }
                    ));
                }
            }
        }
        let remembered = profile.clone();
        config::update_state(move |state| state.profile = remembered);
    }
    if let Some(line) = scanning {
        leave(i32::from(!scan_only(&line)));
    }
    let dump_with = choices.options.decoder;
    if (dumping.is_some() || against.is_some() || !checking.is_empty())
        && (choices.options.deinterlace.is_some()
            || choices.options.deinterlace_by != zwcore::decode::DeinterlaceBy::Auto)
    {
        tracing::info!(
            "--dump・--against・--check では --deinterlace と --deinterlace-by は効かない"
        );
    }
    if !checking.is_empty() && choices.options.decoder.is_some() {
        tracing::info!("--check では --decoder は効かない");
    }
    if let (Some(path), Some(into)) = (path.as_deref(), dumping.as_deref()) {
        let made = match dump_with {
            Some(zwcore::decode::Prefer::Platform) => check::dump_hardware(path, into, frames),
            Some(zwcore::decode::Prefer::System) => check::dump_system(path, into, frames),
            _ => check::dump(path, into, frames),
        };
        leave(i32::from(!made));
    }
    if let Some(path) = sound_checking.as_deref() {
        leave(i32::from(!check::sound_check(path, frames)));
    }
    if let (Some(path), Some(raw)) = (path.as_deref(), against.as_deref()) {
        leave(i32::from(!check::against(path, raw, frames)));
    }
    if !checking.is_empty() {
        let paths: Vec<String> = checking
            .into_iter()
            .filter(|name| !name.is_empty())
            .collect();
        if paths.is_empty() {
            tracing::info!("usage: ZeroWave --check <録画> [<録画>…]");
            leave(2);
        }
        leave(i32::from(!check::run(&paths, frames)));
    }
    let live = mirakurun.is_some() || coaxd.is_some() || idle_because.is_some();
    let Some(path) = path.or_else(|| live.then(String::new)) else {
        tracing::info!(
            "usage: ZeroWave                                                （zerowave.toml の source を使う。ダブルクリック時もこれ）\n\
             \x20          --profile <名前>                                 （使うプロファイル。省略時は state.toml の前回の値）\n\
             \x20          --set セクション.キー=値                         （設定の一時上書き。例 --set audio.latency=120）\n\
             \x20 ZeroWave [--program <n>] [--no-sound] <録画>\n\
             \x20 ZeroWave --scan [<方式>]                                  （ウィンドウを開かずにスキャンして channels.toml を書く）\n\
             \x20 ZeroWave --mirakurun http://ホスト:40772 [--nid <n>] [--tsid <n>] [--sid <n>]\n\
             \x20          --nid・--tsid・--sid                            （起動時に選ぶ局。十進か 0x で始まる十六進。指定した番号が全部合う最初の局）\n\
             \x20          --mirakurun-stream auto|channel|service          （チャンネル単位で受けるか、サービス単位で受けるか）\n\
             \x20          --decoder        auto|hardware|software|system    （system は OS のソフトウェアデコーダー）\n\
             \x20          --deinterlace    auto|off|weave|bob|yadif|bwdif   （デインタレースの方式）\n\
             \x20          --deinterlace-by auto|software|hardware           （デインタレースする場所）\n\
             \x20          --video          <n>                              （映像の番号。降雨対応放送）\n\
             \x20          --audio          <n>                              （音声の番号）\n\
             \x20          --dual-mono      main|sub|both                    （デュアルモノの主/副）\n\
             \x20          --audio-latency  <ミリ秒>                         （音声出力の遅延。音が遅れて聞こえるとき）\n\
             \x20          --passthrough                                     （音声をデコードせずにそのまま出力する）\n\
             \x20          --sound-device   <名前>                           （音声の出力先。省略時は既定のデバイス）\n\
             \x20          --render         auto|wgpu|gles\n\
             \x20          --present        auto|vsync|immediate              （リフレッシュレートの低い環境向け）\n\
             \x20          --backend        vulkan|metal|dx12                （wgpu のバックエンド）\n\
             \x20          --gpu            <名前>                           （GPU が複数あるとき、レンダリングに使う GPU）\n\
             \x20          --hdr            auto|off|on                      （サーフェスを HDR で開くか。auto は HDR の映像のときだけ）\n\
             \x20 ZeroWave --check [--frames <n>] <録画>…\n\
             \x20 ZeroWave <録画> --dump out.yuv | --against ffmpeg.yuv"
        );
        leave(2);
    };

    // The sound device is opened before anything is decoded: its rate is what the clock counts in.
    let passthrough = if passthrough != Passthrough::Off && !want_sound {
        config::notice(
            "コマンドライン引数 --passthrough と --no-sound は同時に指定できません。\n音声を出さずに起動しました。",
        );
        Passthrough::Off
    } else {
        passthrough
    };
    let (sound, keep) = if !want_sound {
        (None, sound::Keep::none())
    } else if passthrough == Passthrough::Auto {
        let (pcm, keep) = open_pcm(sound_device.as_deref());
        match (pcm, Device::passthrough(48_000, sound_device.as_deref())) {
            (Some(Device::Pcm(pcm)), Ok(link)) => {
                tracing::info!(
                    "音声: 自動（マルチチャンネルは {} へパススルー、ほかはデコード）",
                    link.name()
                );
                (Some(Device::either(pcm, link)), keep)
            }
            (pcm, Err(error)) => {
                tracing::warn!("パススルーの出力を開けないため、デコードのみ: {error}");
                (pcm, keep)
            }
            (pcm, Ok(_)) => (pcm, keep),
        }
    } else if passthrough == Passthrough::On {
        match Device::passthrough(48_000, sound_device.as_deref()) {
            Ok(device) => {
                tracing::info!("音声: パススルー（{}）", device.name());
                (Some(device), sound::Keep::none())
            }
            Err(error) => {
                config::notice(stage::said_with("パススルー出力が利用できません。", &error));
                open_pcm(sound_device.as_deref())
            }
        }
    } else {
        open_pcm(sound_device.as_deref())
    };
    if passthrough == Passthrough::On
        && choices.options.dual_mono != zwcore::engine::DualMono::default()
    {
        tracing::info!("パススルーでは --dual-mono は効かない");
    }

    if let Some((endpoint, priority, tuners, profile)) = coaxd {
        return from_coaxd(
            &endpoint, priority, &tuners, &profile, pick, sound, keep, choices,
        );
    }
    if let Some(url) = mirakurun {
        return from_mirakurun(&url, mirakurun_by, pick, sound, keep, choices);
    }
    if let Some(why) = idle_because {
        return idle(&why, sound, keep, choices);
    }

    let mut player = match Player::open(&path, sound, choices.options, Some(plugins::loaded())) {
        Ok(player) => player,
        Err(error) => {
            let (sound, keep) = open_pcm(sound_device.as_deref());
            return idle(&error, sound, keep, choices);
        }
    };
    if !player.look(program) {
        drop(player);
        let (sound, keep) = open_pcm(sound_device.as_deref());
        let why = match program {
            Some(program) => format!(
                "指定された番組が見つかりません。\n{path}（サービス {}）",
                stage::id_text(program)
            ),
            None => format!("再生できる番組がありません。\n{path}"),
        };
        return idle(&why, sound, keep, choices);
    }
    run(player, choices, None, keep);
}

struct Finish;

/// 自分で終了する。`std::process::exit` は `Drop` を呼ばないので [`Finish`] と同じ後始末をする。
pub fn leave(code: i32) -> ! {
    window::ending::exit_now(code)
}

impl Drop for Finish {
    fn drop(&mut self) {
        log::finish();
    }
}

fn idle(why: &str, sound: Option<Device>, keep: sound::Keep, choices: Choices) {
    config::notice(why);
    let mut player = Player::from_source(
        zwcore::input::source::pending(),
        zwcore::filters::Container::Ts,
        sound,
        choices.options,
        Some(plugins::loaded()),
    );
    if !player.look_live(None) {
        leave(1);
    }
    let live = Live {
        supply: Supply::new(Box::new(Nothing {
            why: why.to_owned(),
        })),
        channels: Vec::new(),
        fetched: None,
        url: None,
        starting: None,
    };
    run(player, choices, Some(live), keep);
}

#[derive(Debug)]
struct Nothing {
    why: String,
}

impl Source for Nothing {
    fn name(&self) -> &str {
        "source なし"
    }

    fn channels(
        &mut self,
    ) -> Result<Vec<zwcore::input::source::Channel>, zwcore::input::source::Error> {
        Ok(Vec::new())
    }

    fn open(
        &mut self,
        _channel: &zwcore::input::source::Channel,
    ) -> Result<
        (zwcore::input::source::Bytes, zwcore::filters::Container),
        zwcore::input::source::Error,
    > {
        Err(zwcore::input::source::Error::Refused(self.why.clone()))
    }
}

fn from_mirakurun(
    url: &str,
    by: zwcore::input::source::mirakurun::By,
    pick: Pick,
    sound: Option<Device>,
    keep: sound::Keep,
    choices: Choices,
) {
    let mut source = match zwcore::input::source::mirakurun::Mirakurun::at(url) {
        Ok(source) => source,
        Err(why) => {
            return idle(
                &stage::said_with("Mirakurun に接続できません。", &why),
                sound,
                keep,
                choices,
            );
        }
    };
    source.set_by(by);
    let mut player = Player::from_source(
        zwcore::input::source::pending(),
        zwcore::filters::Container::Ts,
        sound,
        choices.options,
        Some(plugins::loaded()),
    );
    if !player.look_live(None) {
        leave(1);
    }
    let live = Live {
        supply: Supply::new(Box::new(source)),
        channels: Vec::new(),
        fetched: None,
        url: Some(url.to_owned()),
        starting: Some(stage::Starting::Mirakurun {
            url: url.to_owned(),
            by,
            pick,
        }),
    };
    run(player, choices, Some(live), keep);
}

fn scan_only(line: &str) -> bool {
    let file = config::load_plain();
    let Some(source) = file.sources.iter().find(|source| source.kind == "coaxd") else {
        log::fatal(
            "チャンネルスキャンには coaxd のソースが必要です（[[source]] kind = \"coaxd\"）",
        );
        return false;
    };
    zwcore::input::source::coaxd::set_tuner_settings(
        file.sources
            .iter()
            .filter(|source| source.kind == "coaxd")
            .flat_map(|source| &source.tuner_settings)
            .filter(|(_, setting)| !setting.is_empty())
            .map(|(id, setting)| (id.clone(), setting.to_core()))
            .collect(),
    );
    let legs: Vec<scan::Leg> = ui::panes::settings::PLANS
        .iter()
        .filter(|plan| line.is_empty() || **plan == line)
        .filter_map(|plan| {
            let (_, system) = ui::panes::settings::offered(plan);
            let tuner =
                zwcore::scan::serving_tuner(&source.endpoint, source.priority, system, &[])?;
            let had = ui::panes::settings::line_of(&file, plan);
            Some(scan::Leg {
                label: (*plan).to_owned(),
                endpoint: source.endpoint.clone(),
                priority: source.priority.saturating_sub(1),
                spec: zwcore::scan::Spec {
                    plan: had.plan.clone(),
                    bands: had.bands.clone(),
                    only: had.only.clone(),
                    satellites: had.satellites.clone(),
                    modulations: had.modulations.clone(),
                },
                tuner: Some(tuner),
            })
        })
        .collect();
    if legs.is_empty() {
        log::fatal(&if line.is_empty() {
            String::from("受信方式の分かるチューナーがありません（チューナー設定の受信方式）")
        } else {
            format!("受信方式「{line}」を受けるチューナーがありません")
        });
        return false;
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let job = scan::Job {
        legs,
        profile: config::channels_profile("").to_owned(),
        options: zwcore::scan::Options::default(),
        stop,
        keep: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    let into: scan::Progress =
        std::sync::Arc::new(std::sync::Mutex::new(scan::Scanning::default()));
    let said = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let telling = (into.clone(), said.clone());
    let tell = move || {
        let (into, said) = &telling;
        let (Ok(scanning), Ok(mut said)) = (into.lock(), said.lock()) else {
            return;
        };
        for line in scanning.log.iter().skip(*said) {
            tracing::info!("{line}");
        }
        *said = scanning.log.len();
    };
    scan::run(
        job,
        &into,
        &scan::Hooks {
            wake: &tell,
            yield_tuner: &|_| {},
            finished: &|_| {},
        },
    );
    match into
        .lock()
        .ok()
        .and_then(|scanning| scanning.done_with.clone())
    {
        Some(Ok(summary)) => {
            tracing::info!(
                "多重化ストリーム数 {}・サービス数 {}",
                summary.streams,
                summary.services
            );
            true
        }
        Some(Err(why)) => {
            log::fatal(&why);
            false
        }
        None => false,
    }
}

/// coaxd から受ける。ウィンドウを先に開け、coaxd の起動と接続は裏で（起動まで数秒かかる）。
#[allow(clippy::too_many_arguments)]
fn from_coaxd(
    endpoint: &str,
    priority: i32,
    tuners: &[String],
    profile: &str,
    pick: Pick,
    sound: Option<Device>,
    keep: sound::Keep,
    choices: Choices,
) {
    let source = zwcore::input::source::coaxd::Coaxd::written(endpoint, priority)
        .with_tuners(tuners.to_vec());
    let came = stage::tuning::came_of_coaxd(endpoint, tuners, profile);
    let mut player = Player::from_source(
        zwcore::input::source::pending(),
        zwcore::filters::Container::Ts,
        sound,
        choices.options,
        Some(plugins::loaded()),
    );
    if !player.look_live(None) {
        leave(1);
    }
    let live = Live {
        supply: Supply::new(Box::new(source)),
        channels: Vec::new(),
        fetched: None,
        url: Some(came),
        starting: Some(stage::Starting::Coaxd {
            endpoint: endpoint.to_owned(),
            priority,
            tuners: tuners.to_vec(),
            profile: config::channels_profile(profile).to_owned(),
            pick,
        }),
    };
    run(player, choices, Some(live), keep);
}

fn run(mut player: Player, choices: Choices, live: Option<Live>, keep: sound::Keep) {
    tracing::info!("{}", choices.line());
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let choices = if choices.render == Render::Gles {
        config::notice(
            "この環境では GLES 2.0 のレンダリングを利用できません。\nwgpu でレンダリングします。",
        );
        Choices {
            render: Render::Auto,
            ..choices
        }
    } else {
        choices
    };
    // wgpu の Instance はここで作る。別の Instance で作ったサーフェスはアダプターから見えない。
    let instance = (choices.render != Render::Gles).then(|| draw::instance_of(choices.backend));
    // auto で wgpu にハードウェアの GPU が無い（Vulkan がソフトウェアの llvmpipe だけなど）なら GLES 2.0 で描く。
    // GLES も開けなければ、ウィンドウを開いた後で wgpu に戻る（[`window::shell`]）。
    #[cfg(all(unix, not(target_os = "macos")))]
    let (choices, instance) = if choices.render == Render::Auto
        && choices.gpu.is_none()
        && instance
            .as_ref()
            .is_some_and(|instance| !zwcore::output::video::gpu::Gpu::draws_on_hardware(instance))
    {
        tracing::info!("wgpu にハードウェアの GPU が無いので、GLES 2.0 でレンダリングする");
        let choices = Choices {
            auto_gles: true,
            ..choices
        };
        (choices, None)
    } else {
        (choices, instance)
    };
    let event_loop = match EventLoop::<Wake>::with_user_event().build() {
        Ok(loop_) => loop_,
        Err(error) => {
            tracing::warn!("ウィンドウを開けない: {error}");
            leave(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let (to_stage, events) = mpsc::channel();
    let (from_stage_to, from_stage) = mpsc::channel();
    let (guide_to, guide_asked) = mpsc::channel();
    let (guide_merged, guide_back) = mpsc::channel();
    let (listed_to, listed) = mpsc::channel();
    let _ = guide_to.send(zwcore::guide::keeper::ToGuide::Logos(
        config::load_state().logos,
    ));
    let keep_guide = config::load_plain().guide.cache;
    let guide = window::ending::Thread::spawn("guide", "番組表", move |_told| {
        threads::background::lower();
        zwcore::guide::keeper::keeping(
            &guide_asked,
            &guide_merged,
            &listed_to,
            state::epg_cache(),
            keep_guide,
        )
    })
    .inspect_err(|error| tracing::warn!("番組表のスレッドを起動できない: {error}"))
    .ok();
    // 別ウィンドウのチャネルはメインスレッドも持つ（再生スレッドが詰まっても番組表は動くように）。
    let (requests, from_panes) = mpsc::channel();
    let proxy = event_loop.create_proxy();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    window::shell::watch_system_theme(requests.clone(), proxy.clone());
    #[cfg(any(windows, target_os = "linux"))]
    if config::holds_single() {
        window::handoff::listen(to_stage.clone(), requests.clone(), proxy.clone());
    }
    let waker: zwcore::wake::Waker = {
        let proxy = proxy.clone();
        Arc::new(move || {
            let _ = proxy.send_event(Wake);
        })
    };
    let _ = std::thread::Builder::new()
        .name(String::from("plugins"))
        .spawn(|| {
            let _ = plugins::loaded();
        });
    let zwcore::data::Handles {
        to: to_broadcast,
        broadcast: shown_broadcast,
        captions: shown_captions,
        layers: shown_layers,
        present,
    } = zwcore::data::spawn(zwcore::data::Config {
        wake: waker.clone(),
        sound: player.sound_sender(),
        plugins: Some(plugins::loaded()),
        nvram: state::nvram(),
        cfg: state::plugin_cfg(),
    });
    player.broadcast_to(to_broadcast.clone());
    let (panes, to_panes) = threads::panes::Panes::new(
        to_broadcast.clone(),
        Arc::clone(&shown_broadcast),
        config::load_plain(),
    );
    let deck = deck::Deck::new(
        to_stage.clone(),
        from_stage,
        to_panes.clone(),
        to_broadcast.clone(),
        shown_broadcast,
        shown_captions,
        shown_layers,
        present.clone(),
        choices.fit_window,
        choices.knobs,
    );
    let boot = window::shell::Boot {
        stage: stage::Boot {
            player,
            choices,
            live,
            painter: None,
            surface: None,
            #[cfg(target_os = "windows")]
            overlay: None,
            instance: instance.clone(),
            panes,
            requests,
            proxy: proxy.clone(),
            guide_to,
            guide_back,
            listed,
            size: (0, 0),
            refresh: 0,
            events,
            out: from_stage_to,
            presented: std::sync::Arc::default(),
            broadcast: to_broadcast,
            present,
        },
        instance,
    };
    #[cfg(target_os = "macos")]
    window::mac_open::accept(to_stage.clone());
    let mut shell =
        window::shell::Shell::new(boot, deck, to_stage, to_panes, from_panes, keep, guide);
    if let Err(error) = event_loop.run_app(&mut shell) {
        tracing::warn!("{error}");
        leave(1);
    }
    shell.close();
    if config::restart_requested() {
        restart();
    }
    // Linux は C の後始末を通らずに終える（[`window::ending::exit_now`]）。
    #[cfg(target_os = "linux")]
    window::ending::exit_now(0);
}

pub(crate) fn restart() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    tracing::info!("再起動する: {}", exe.display());
    // このプロセスを新しい ZeroWave に置き換える（別に起こすと二つ分のメモリを使う）。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let why = std::process::Command::new(exe).args(args).exec();
        tracing::warn!("再起動できない: {why}");
    }
    // Windows は置き換えられないので、ロックを放してから新しく起動する。
    #[cfg(not(unix))]
    config::release_single();
    #[cfg(not(unix))]
    match std::process::Command::new(exe).args(args).spawn() {
        Ok(_) => {}
        Err(why) => tracing::warn!("再起動できない: {why}"),
    }
}
