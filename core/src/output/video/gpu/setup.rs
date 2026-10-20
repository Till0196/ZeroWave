// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映像のレンダラーを組み立てる、設定を変える、形式が変わったら作り直す。

use super::*;

impl Video {
    /// Builds everything for one format on one device. `target` is the surface format.
    /// The gamma comes off in the second pass: eight bits of linear light crushes the blacks.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: wgpu::TextureFormat,
        format: Format,
        settings: Settings,
    ) -> Result<Self, Error> {
        Self::of_shape(device, queue, target, format, settings, Shape::Planar)
    }

    /// The same, for a known shape ([`Video::push`] rebuilds for whatever it is handed).
    pub fn of_shape(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: wgpu::TextureFormat,
        format: Format,
        settings: Settings,
        shape: Shape,
    ) -> Result<Self, Error> {
        // プラグインのデインタレースは前後に二つずつ読むので、その分深くする。
        #[cfg(feature = "plugins")]
        let pool_slots = slots(format, shape) + if super::hook::wants_frames() { 2 } else { 0 };
        #[cfg(not(feature = "plugins"))]
        let pool_slots = slots(format, shape);
        let pool = Pool::new(device, format, pool_slots, shape)?;
        // Only the three plane path at ten bits samples integers; imported planes are normalised.
        let uint = format.depth > 8 && shape == Shape::Planar;
        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..shape.bindings())
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: if uint {
                        wgpu::TextureSampleType::Uint
                    } else {
                        wgpu::TextureSampleType::Float { filterable: false }
                    },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: shape.bindings(),
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let planes = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planes"),
            entries: &entries,
        });
        let planes_entries = entries;
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("conversion"),
            size: Params::SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let picture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picture"),
            size: wgpu::Extent3d {
                width: format.width,
                height: format.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: picture_format(format.depth),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let picture_view = picture.create_view(&wgpu::TextureViewDescriptor::default());

        let blit_layout = blit_layout(device);
        let sampler = display_sampler(device);
        let fit = fit_buffer(device);
        let blit = build(
            device,
            &blit_layout,
            shader::blit().to_string(),
            target,
            None,
            "blit",
        );
        let pipeline = build(
            device,
            &planes,
            shader::deinterlace(settings.deinterlace, format.depth, shape),
            picture_format(format.depth),
            None,
            "deinterlace",
        );

        let live = LIVE_VIDEOS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        tracing::debug!(
            "レンダリング: 映像レンダラーを作成（使用中 {live} 個、{}x{}）",
            format.width,
            format.height
        );
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            target,
            space: Space::Srgb,
            sdr_white: crate::output::video::colour::SDR_WHITE,
            hlg_peak: crate::output::video::colour::HLG_PEAK,
            inset: None,
            format,
            settings,
            shape,
            upload: Upload::default(),
            pool,
            picture,
            picture_view,
            planes,
            pipeline,
            params,
            blits: vec![(Blit::default(), blit)],
            blits_coming: Vec::new(),
            pan_scan: super::pan_scan::PanScan::default(),
            fit,
            #[cfg(feature = "vaapi")]
            import: super::import::dmabuf::Import::new(device),
            #[cfg(all(feature = "android", target_os = "android"))]
            import: super::import::android::Import::new(device),
            #[cfg(all(feature = "d3d12", windows))]
            import: super::import::d3d12::Import::new(device),
            #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
            import: super::import::videotoolbox::Import::new(device),
            pushed: 0,
            slot_index: vec![0; pool_slots],
            last_slot: None,
            last_progressive: false,
            matcher: None,
            planes_entries,
            drawn_once: false,
            schedule: Schedule::new(pool_slots),
            scratch: Vec::new(),
            spent: Spent::default(),
            #[cfg(feature = "plugins")]
            hooks: super::hook::Hooks::new(),
            sub: 0,
            pers: std::collections::VecDeque::new(),
            phases: std::collections::VecDeque::new(),
            last_phases: Vec::new(),
            retime_at: 0.0,
            display_mhz: 0,
            lost: 0,
            scaling: super::Scaling::default(),
            adjust: crate::output::video::Adjust::default(),
            shown: None,
            content: (0.0, 0.0),
            #[cfg(feature = "plugins")]
            ahead: Ahead::default(),
            reduce: reduce::Reduce::new(device),
            split: split::Split::new(device),
            blit_layout,
            sampler,
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        if settings.deinterlace != self.settings.deinterlace {
            self.pipeline = build(
                &self.device,
                &self.planes,
                shader::deinterlace(settings.deinterlace, self.format.depth, self.shape),
                picture_format(self.format.depth),
                None,
                "deinterlace",
            );
            let dropped = self.schedule.len().min(self.pers.len());
            self.lose(dropped);
            let _ = self.schedule.forget_outputs();
        }
        if !settings.telecine {
            self.matcher = None;
        }
        self.settings = settings;
    }

    pub fn upload(&self) -> Upload {
        self.upload
    }

    pub fn set_upload(&mut self, upload: Upload) {
        self.upload = upload;
    }

    pub fn space(&self) -> Space {
        self.space
    }

    /// Says what the surface was configured as. The space can change under a running picture.
    pub fn set_space(&mut self, space: Space) {
        self.space = space;
    }

    /// Where SDR white sits on an HDR surface, in cd/m² ([`Mapping::with_white`]).
    pub fn set_sdr_white(&mut self, white: f32) {
        self.sdr_white = white;
    }

    /// The peak an HLG picture is shown at on an HDR surface, in cd/m² ([`Mapping::with_peak`]).
    pub fn set_hlg_peak(&mut self, peak: f32) {
        self.hlg_peak = peak;
    }

    /// Draws the picture into this part of its letterbox for a data broadcast (`None` is all).
    pub fn set_inset(&mut self, inset: Option<[f32; 5]>) {
        self.inset = inset;
    }

    /// 比率（パン&スキャン）。データ放送の文書が配置を決めている間は効かない。
    pub fn set_pan_scan(&mut self, pan_scan: super::pan_scan::PanScan) {
        self.pan_scan = pan_scan;
    }

    pub fn mapping(&self) -> Mapping {
        Mapping::new(self.settings.transfer, self.settings.primaries, self.space)
            .with_white(self.sdr_white)
            .with_peak(self.hlg_peak)
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn retune(&mut self, format: Format, settings: Settings) -> Result<(), Error> {
        self.rebuild(format, settings, self.shape)
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub(super) fn rebuild(
        &mut self,
        format: Format,
        settings: Settings,
        shape: Shape,
    ) -> Result<(), Error> {
        let upload = self.upload;
        let space = self.space;
        let sdr_white = self.sdr_white;
        let hlg_peak = self.hlg_peak;
        // デバイスの都合でない物は持ち越す（落とすと形が決まり直った直後に既定へ戻る）。
        let inset = self.inset;
        let pan_scan = self.pan_scan;
        let scaling = self.scaling;
        let display_mhz = self.display_mhz;
        #[cfg(feature = "plugins")]
        let hooks = std::mem::take(&mut self.hooks);
        *self = Self::of_shape(
            &self.device,
            &self.queue,
            self.target,
            format,
            settings,
            shape,
        )?;
        self.upload = upload;
        self.space = space;
        self.sdr_white = sdr_white;
        self.hlg_peak = hlg_peak;
        self.inset = inset;
        self.pan_scan = pan_scan;
        self.scaling = scaling;
        self.display_mhz = display_mhz;
        #[cfg(feature = "plugins")]
        {
            self.hooks = hooks;
        }
        Ok(())
    }
}
