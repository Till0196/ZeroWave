// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! サーフェス。何で開くか、いつ用意し直すか、隠れているか。

use super::*;

/// ウィンドウ・サーフェス・ディスプレイの状態を別々に持つ（混ぜると戻れなくなる）。
#[derive(Debug)]
pub(super) struct Surface {
    pub(super) target: wgpu::TextureFormat,
    pub(super) space: Space,
    /// cd/m²。メインスレッドが取得して渡す。
    pub(super) sdr_white: f32,
    /// cd/m²。HLG をこれに相対で変換する。
    pub(super) peak: f32,
    /// 分からなければ真。偽なら映像が HDR でも SDR で開く（SDR の画面で PQ のまま合成されると白が潰れる）。
    monitor_hdr: bool,
    /// 位置を動かした間も持つ（SDR に戻すとシークのたびに HDR のサーフェスを二度作り直す）。
    last_transfer: Option<Transfer>,
    monitor_changed: Option<crate::draw::monitor::Monitor>,
    hdr_refused: bool,
    occluded: bool,
    /// ウィンドウの状態（`occluded`）とは別に持つ（サーフェスの状態は遅れることがあり、上書きすると戻れない）。
    unseen: Option<Instant>,
    pub(super) grabbed: bool,
    regrip: bool,
    held_until: Option<Instant>,
    said_ahead: Option<u32>,
}

impl Surface {
    pub(super) fn new() -> Self {
        Self {
            target: wgpu::TextureFormat::Bgra8UnormSrgb,
            space: Space::Srgb,
            sdr_white: zwcore::output::video::colour::SDR_WHITE,
            peak: zwcore::output::video::colour::HLG_PEAK,
            monitor_hdr: true,
            last_transfer: None,
            monitor_changed: None,
            hdr_refused: false,
            occluded: false,
            unseen: None,
            grabbed: false,
            regrip: false,
            held_until: None,
            said_ahead: None,
        }
    }

    pub(super) fn window_hidden(&mut self, hidden: bool) -> bool {
        let changed = self.occluded != hidden;
        self.occluded = hidden;
        if !hidden {
            self.unseen = None;
        }
        changed
    }

    pub(super) fn unseen_now(&mut self) -> bool {
        let first = self.unseen.is_none();
        self.unseen = Some(Instant::now() + UNSEEN_FOR);
        first
    }

    pub(super) fn seen(&mut self) -> bool {
        self.unseen.take().is_some()
    }

    pub(super) fn covered(&self) -> bool {
        self.occluded || self.unseen.is_some_and(|until| Instant::now() < until)
    }

    pub(super) fn hidden(&self) -> bool {
        self.occluded || self.unseen.is_some()
    }

    /// 離したときは [`GRIP_HOLD`] だけ待つ（「離した」は動きが 120 ms 止まっただけでも来る）。
    pub(super) fn grab(&mut self, grabbed: bool) -> bool {
        if self.grabbed == grabbed {
            return false;
        }
        self.grabbed = grabbed;
        if grabbed {
            self.held_until = None;
            if self.said_ahead != Some(2) {
                self.regrip = true;
            }
        } else {
            self.held_until = Some(Instant::now() + GRIP_HOLD);
        }
        true
    }

    pub(super) fn monitor(&mut self, monitor: crate::draw::monitor::Monitor) {
        if (monitor.hdr, monitor.sdr_white, monitor.peak)
            != (self.monitor_hdr, self.sdr_white, self.peak)
        {
            self.monitor_changed = Some(monitor);
        } else {
            self.monitor_changed = None;
        }
    }

    /// 境目を跨いで動かしている間は結果が何度も入れ替わるので、掴まれている間は載せない。
    pub(super) fn settle(&mut self) -> bool {
        if self.grabbed {
            return false;
        }
        let Some(monitor) = self.monitor_changed.take() else {
            return false;
        };
        tracing::info!(
            "ディスプレイ HDR {}  SDR の白 {:.0} cd/m²  ピーク {:.0} cd/m²",
            if monitor.hdr { "有効" } else { "無効" },
            monitor.sdr_white,
            monitor.peak
        );
        self.monitor_hdr = monitor.hdr;
        self.sdr_white = monitor.sdr_white;
        self.peak = monitor.peak;
        true
    }

    pub(super) fn due(&mut self, wanted: Option<Transfer>) -> bool {
        if !self.grabbed
            && let Some(until) = self.held_until
            && Instant::now() >= until
        {
            self.held_until = None;
            self.regrip = true;
        }
        let stale = !self.hdr_refused
            && !self.grabbed
            && self.for_hdr(wanted).is_some() != self.space.is_hdr();
        std::mem::take(&mut self.regrip) || stale
    }

    fn for_hdr(&self, wanted: Option<Transfer>) -> Option<Transfer> {
        if !self.monitor_hdr {
            return None;
        }
        wanted
    }

    pub(super) fn choose(
        &mut self,
        capabilities: &wgpu::SurfaceCapabilities,
        wanted: Option<Transfer>,
    ) -> (wgpu::TextureFormat, Space) {
        let wanted = self.for_hdr(wanted);
        if wanted.is_none() {
            self.hdr_refused = false;
        }
        let sdr = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb);
        let chosen = match wanted.and_then(|transfer| hdr_surface(capabilities, transfer)) {
            Some(chosen) => chosen,
            None => {
                if wanted.is_some() && !self.hdr_refused {
                    tracing::info!(
                        "ディスプレイが HDR 非対応のため、サーフェスは SDR（映像は SDR へ変換）"
                    );
                    self.hdr_refused = true;
                }
                (sdr, Space::Srgb)
            }
        };
        if chosen != (self.target, self.space) {
            tracing::info!(
                "サーフェス {:?}  {}",
                chosen.0,
                match chosen.1 {
                    Space::Srgb => "sRGB",
                    Space::ScRgb if cfg!(target_vendor = "apple") => {
                        "scRGB（リニア、1.0 = SDR の白）"
                    }
                    Space::ScRgb => "scRGB（リニア、1.0 = 80 cd/m²）",
                    Space::Pq => "BT.2100 PQ",
                    Space::Hlg => "BT.2100 HLG",
                }
            );
        }
        chosen
    }

    /// ループは放送のティックで回るので、画面が十分速い（90 Hz 以上）ときだけ一つ。60 Hz で一つだと
    /// 取るたびに垂直同期を丸ごと待って遅れが伸びる。分からない画面と掴まれている間は二つ。
    pub(super) fn ahead(&mut self, refresh: u32) -> u32 {
        const OUTPUTS: u32 = 60;
        let held = self.held_until.is_some_and(|until| Instant::now() < until);
        let forced = std::env::var("ZW_PRESENT_AHEAD")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|ahead| (1..=2).contains(ahead));
        let ahead = if self.grabbed || held {
            2
        } else {
            // ウィンドウには問い合わせない（macOS ではウィンドウに触ると `dispatch_sync` になる）。
            match (forced, refresh) {
                (Some(ahead), _) => ahead,
                (None, hz) if hz >= OUTPUTS * 1500 => 1,
                _ => 2,
            }
        };
        if self.said_ahead != Some(ahead) {
            self.said_ahead = Some(ahead);
            tracing::info!("最大フレーム遅延 {ahead}");
        }
        ahead
    }
}

impl Stage {
    pub(super) fn configure(&mut self, width: u32, height: u32) {
        // A resize drag sends one per pixel; setting up again for the same size stutters.
        if self.size == (width, height) {
            return;
        }

        if width == 0 || height == 0 {
            return;
        }
        let ahead = self.surface.ahead(self.refresh);
        self.say_video_latency();
        let wanted = self.hdr_wanted();
        let Some(painter) = self.painter.as_mut() else {
            return;
        };
        #[cfg_attr(not(unix), allow(irrefutable_let_patterns))]
        let Painter::Wgpu {
            gpu,
            target,
            space,
            white,
            peak,
        } = painter
        else {
            painter.resize(width, height);
            self.size = (width, height);
            return;
        };
        let Some(surface) = self.handed.as_ref() else {
            return;
        };
        let capabilities = surface.get_capabilities(&gpu.adapter);
        let (format, colour) = self.surface.choose(&capabilities, wanted);
        let rebuilt = (format, colour, self.surface.sdr_white, self.surface.peak)
            != (*target, *space, *white, *peak);
        *target = format;
        *space = colour;
        *white = self.surface.sdr_white;
        *peak = self.surface.peak;
        self.surface.target = format;
        self.surface.space = colour;
        self.size = (width, height);
        if rebuilt {
            self.player.rebuild_screen();
            if let Some(mixer) = self.mixer.as_mut() {
                mixer.retarget(&gpu.device, format, colour, self.surface.sdr_white);
            }
        }
        let alpha = capabilities
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        surface.configure(
            &gpu.device,
            &wgpu::SurfaceConfiguration {
                usage: surface_usage(&capabilities),
                format,
                width,
                height,
                present_mode: self.choices.present.mode(),
                // At one, back pressure holds the loop to the display's blanks.
                // See [`Surface::ahead`].
                desired_maximum_frame_latency: ahead,
                alpha_mode: alpha,
                // Auto never resolves to HDR, so the HDR spaces are named outright.
                color_space: match colour {
                    Space::Srgb => wgpu::SurfaceColorSpace::Auto,
                    Space::ScRgb => wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
                    Space::Pq => wgpu::SurfaceColorSpace::Bt2100Pq,
                    Space::Hlg => wgpu::SurfaceColorSpace::Bt2100Hlg,
                },
                view_formats: if format.is_srgb() {
                    vec![format.remove_srgb_suffix()]
                } else {
                    Vec::new()
                },
            },
        );
        self.say_video_latency();
    }

    pub(super) fn hdr_wanted(&mut self) -> Option<Transfer> {
        let transfer = match self.player.picture_format().map(Transfer::of_format) {
            Some(transfer) => {
                self.surface.last_transfer = Some(transfer);
                transfer
            }
            None => self.surface.last_transfer.unwrap_or(Transfer::Sdr),
        };
        match self.choices.hdr {
            Hdr::Off => None,
            Hdr::On => Some(transfer),
            Hdr::Auto => transfer.is_hdr().then_some(transfer),
        }
    }

    /// 指定しないと映像は常にその分遅れて見える。測れたら（`shown::Shown::known`）何もしない。
    pub(super) fn say_video_latency(&mut self) {
        self.player.set_display(self.refresh);
        if self.shown.known() || self.refresh == 0 {
            return;
        }
        let vsync = 1000.0 * 1000.0 / f64::from(self.refresh);
        let blanks = match self.choices.present {
            crate::choice::Present::Immediate => 1.0,
            _ => f64::from(self.surface.said_ahead.unwrap_or(2)),
        };
        self.player
            .set_video_latency(vsync * blanks + self.filter_latency);
    }
}

impl Stage {
    pub(super) fn hide(&mut self, hidden: bool) {
        if self.surface.window_hidden(hidden) {
            tracing::info!(
                "ウィンドウが{}",
                if hidden {
                    "非表示になった"
                } else {
                    "再表示された"
                }
            );
        }
        self.hidden();
    }

    pub(super) fn hidden(&mut self) {
        self.player.set_hidden(self.surface.hidden());
    }

    /// 離した後は実測を捨て直す（コンポジターが保持していた間の値を混ぜない）。
    pub(super) fn grab(&mut self, grabbed: bool) {
        if self.surface.grab(grabbed) && !grabbed {
            self.shown.forget();
        }
    }
}

/// HLG、PQ、scRGB の順で、`capabilities` が明示的に返しているものだけ選ぶ。
pub(super) fn hdr_surface(
    capabilities: &wgpu::SurfaceCapabilities,
    transfer: Transfer,
) -> Option<(wgpu::TextureFormat, Space)> {
    use wgpu::SurfaceColorSpaces as Spaces;
    let ten = wgpu::TextureFormat::Rgb10a2Unorm;
    let half = wgpu::TextureFormat::Rgba16Float;
    let offers = |format, space: Spaces| capabilities.color_spaces(format).contains(space);
    // macOS では scRGB だけ。wgpu-hal の Metal は BT.2100 の色空間名で SIGBUS で落ちる。
    // 上流が直ったら外す。
    let mut order = if cfg!(target_os = "macos") {
        vec![(half, Spaces::EXTENDED_SRGB_LINEAR, Space::ScRgb)]
    } else {
        vec![
            (ten, Spaces::BT2100_PQ, Space::Pq),
            (half, Spaces::EXTENDED_SRGB_LINEAR, Space::ScRgb),
        ]
    };
    if transfer == Transfer::Hlg && !cfg!(target_os = "macos") {
        order.insert(0, (ten, Spaces::BT2100_HLG, Space::Hlg));
    }
    order
        .into_iter()
        .find(|(format, flag, _)| offers(*format, *flag))
        .map(|(format, _, space)| (format, space))
}

/// PQ と HLG のサーフェスに画面を載せるとき `Sheet` がコピーを読むので `COPY_SRC` も。
pub(super) fn surface_usage(capabilities: &wgpu::SurfaceCapabilities) -> wgpu::TextureUsages {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC) {
        usage |= wgpu::TextureUsages::COPY_SRC;
    }
    usage
}

pub fn configure_surface(
    gpu: &Gpu,
    surface: &wgpu::Surface<'static>,
    target: &mut wgpu::TextureFormat,
    width: u32,
    height: u32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let capabilities = surface.get_capabilities(&gpu.adapter);
    // egui は sRGB の値の上で混ぜる前提。sRGB の形式だと縁のぼかしや影が明るく浮く。
    let format = capabilities
        .formats
        .iter()
        .copied()
        .find(|format| !format.is_srgb())
        .or_else(|| capabilities.formats.first().copied())
        .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
    *target = format;
    let alpha = capabilities
        .alpha_modes
        .first()
        .copied()
        .unwrap_or(wgpu::CompositeAlphaMode::Auto);
    // 別ウィンドウは垂直同期を待たない（`Fifo` だと映像をレンダリングするスレッドが待たされる）。
    let present_mode = [
        wgpu::PresentMode::Mailbox,
        wgpu::PresentMode::Immediate,
        wgpu::PresentMode::Fifo,
    ]
    .into_iter()
    .find(|mode| capabilities.present_modes.contains(mode))
    .unwrap_or(wgpu::PresentMode::Fifo);
    surface.configure(
        &gpu.device,
        &wgpu::SurfaceConfiguration {
            usage: surface_usage(&capabilities),
            format,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: alpha,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: Vec::new(),
        },
    );
}
