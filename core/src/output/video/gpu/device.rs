// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デバイスを開く（バックエンドの選び方と、プラットフォームごとにデバイスに必要な物）。

use super::*;

impl Gpu {
    /// Opens a device on whatever backend this machine has; [`Error::NoDevice`] is normal.
    pub fn open() -> Result<Self, Error> {
        Self::open_for(None)
    }

    /// The same, for a device that can draw to this surface (only one this made itself).
    pub fn open_for(surface: Option<&wgpu::Surface<'_>>) -> Result<Self, Error> {
        Self::open_on(
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env()),
            surface,
        )
    }

    pub fn open_on(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, Error> {
        Self::open_named(instance, surface, None)
    }

    pub fn names(instance: &wgpu::Instance) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for adapter in pollster::block_on(instance.enumerate_adapters(Self::backends())) {
            let info = adapter.get_info();
            if info.device_type == wgpu::DeviceType::Cpu {
                continue;
            }
            if !names.contains(&info.name) {
                names.push(info.name);
            }
        }
        names
    }

    /// ウィンドウに描けるハードウェアの GPU があるか。ソフトウェアの物（llvmpipe など）と GL は数えない
    /// （GL はディスプレイのハンドル無しで作った Instance ではウィンドウに描けない）。
    pub fn draws_on_hardware(instance: &wgpu::Instance) -> bool {
        pollster::block_on(instance.enumerate_adapters(Self::backends()))
            .iter()
            .map(wgpu::Adapter::get_info)
            .any(|info| {
                info.device_type != wgpu::DeviceType::Cpu && info.backend != wgpu::Backend::Gl
            })
    }

    /// この PC の GPU（ソフトウェアの物は除く）。複数のバックエンドで見える物はレンダリングしている側だけ。
    pub fn adapters(&self) -> Vec<wgpu::AdapterInfo> {
        let ours = self.adapter.get_info();
        let mut found: Vec<wgpu::AdapterInfo> = Vec::new();
        for adapter in pollster::block_on(self.instance.enumerate_adapters(Self::backends())) {
            let info = adapter.get_info();
            if info.device_type == wgpu::DeviceType::Cpu {
                continue;
            }
            // D3D12 は同じ GPU を何度も並べることがあり、同じ型番の二枚とも見分けられないので一つに。
            if found.iter().any(|known| {
                known.backend == info.backend
                    && known.name == info.name
                    && known.vendor == info.vendor
                    && known.device == info.device
                    && known.device_pci_bus_id == info.device_pci_bus_id
                    && known.driver_info == info.driver_info
            }) {
                continue;
            }
            let same = |known: &wgpu::AdapterInfo| {
                known.backend != info.backend
                    && if known.device_pci_bus_id.is_empty() || info.device_pci_bus_id.is_empty() {
                        known.vendor == info.vendor && known.device == info.device
                    } else {
                        known.device_pci_bus_id == info.device_pci_bus_id
                    }
            };
            match found.iter_mut().find(|known| same(known)) {
                Some(known) if info.backend == ours.backend => *known = info,
                Some(_) => {}
                None => found.push(info),
            }
        }
        // OpenGL ES のアダプターは型番も PCI の位置も返さず名前も違うので、レンダリングしているのが別のバックエンドなら数えない。
        if ours.backend != wgpu::Backend::Gl
            && found.iter().any(|info| info.backend != wgpu::Backend::Gl)
        {
            found.retain(|info| info.backend != wgpu::Backend::Gl);
        }
        found
    }

    /// Windows のデコーダーを含むビルドは D3D12 だけ：フレームは `ID3D12Device` の外に出せない。
    fn backends() -> wgpu::Backends {
        #[cfg(all(feature = "d3d12", windows))]
        {
            wgpu::Backends::DX12
        }
        #[cfg(not(all(feature = "d3d12", windows)))]
        {
            wgpu::Backends::all()
        }
    }

    /// 指定した GPU で開く（同じ綴り、無ければ大文字小文字を見ずに含む物）。黙って別の物にしない。
    pub fn open_named(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        name: Option<&str>,
    ) -> Result<Self, Error> {
        let listed = cfg!(all(feature = "d3d12", windows)) || name.is_some();
        let chosen = if listed {
            let mut adapters: Vec<wgpu::Adapter> =
                pollster::block_on(instance.enumerate_adapters(Self::backends()))
                    .into_iter()
                    .filter(|adapter| {
                        surface.is_none_or(|surface| adapter.is_surface_supported(surface))
                    })
                    // ソフトウェアの物は指定されたときだけ（試験用）。
                    .filter(|adapter| {
                        name.is_some() || adapter.get_info().device_type != wgpu::DeviceType::Cpu
                    })
                    .collect();
            adapters.sort_by_key(|adapter| {
                u8::from(adapter.get_info().device_type != wgpu::DeviceType::DiscreteGpu)
            });
            match name {
                Some(name) => {
                    let wanted = name.trim();
                    let exact = adapters
                        .iter()
                        .position(|adapter| adapter.get_info().name == wanted);
                    let loose = || {
                        let wanted = wanted.to_lowercase();
                        adapters.iter().position(|adapter| {
                            adapter.get_info().name.to_lowercase().contains(&wanted)
                        })
                    };
                    let at = exact.or_else(loose).ok_or_else(|| {
                        Error::NoDevice(format!("GPU「{wanted}」がこの PC に無い"))
                    })?;
                    Some(adapters.swap_remove(at))
                }
                None => adapters.into_iter().next(),
            }
        } else {
            None
        };
        let adapter = match chosen {
            Some(adapter) => adapter,
            None => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
                ..Default::default()
            }))
            .map_err(|error| {
                tracing::warn!("wgpu のアダプターを取れない: {error}");
                Error::NoDevice(
                    if matches!(error, wgpu::RequestAdapterError::NotFound { .. }) {
                        String::from("この PC に使える GPU がありません")
                    } else {
                        error.to_string()
                    },
                )
            })?,
        };
        let (device, queue) = Self::device_on(&adapter)?;
        remember_matrices(&device, &adapter);
        let opened = OPENED.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
        if let Ok(mut lost) = LOST.lock() {
            *lost = None;
        }
        say_when_lost(&device, "映像", opened);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    /// 同じアダプターの上にキューだけ別のデバイスを開く（ウィンドウの処理が映像の提示を遅らせないよう）。
    pub fn beside(&self) -> Result<Self, Error> {
        let (device, queue) = Self::device_on(&self.adapter)?;
        remember_matrices(&device, &self.adapter);
        say_when_lost(
            &device,
            "ウィンドウ",
            OPENED.load(std::sync::atomic::Ordering::Acquire),
        );
        Ok(Self {
            instance: self.instance.clone(),
            adapter: self.adapter.clone(),
            device,
            queue,
        })
    }

    /// 同じデバイスとキューを指す組（作り直さない）。
    pub fn shared(&self) -> Self {
        Self {
            instance: self.instance.clone(),
            adapter: self.adapter.clone(),
            device: self.device.clone(),
            queue: self.queue.clone(),
        }
    }

    #[cfg(not(any(
        any(feature = "vaapi", all(feature = "nvdec", target_os = "linux")),
        all(feature = "videotoolbox", target_vendor = "apple"),
        all(feature = "d3d12", windows),
        all(feature = "android", target_os = "android")
    )))]
    fn device_on(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), Error> {
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("zerowave video"),
            required_features: (wgpu::Features::TIMESTAMP_QUERY & adapter.features())
                | compute(adapter),
            required_limits: device_limits(adapter),
            experimental_features: experimental(adapter),
            memory_hints: MEMORY_HINTS,
            ..Default::default()
        }))
        .map_err(|error| Error::NoDevice(error.to_string()))
    }

    /// The device for VideoToolbox pictures: 10-bit planes need the 16-bit normalised formats.
    #[cfg(all(
        feature = "videotoolbox",
        target_vendor = "apple",
        not(any(feature = "vaapi", all(feature = "nvdec", target_os = "linux"))),
        not(all(feature = "d3d12", windows)),
        not(all(feature = "android", target_os = "android"))
    ))]
    fn device_on(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), Error> {
        let wanted = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM & adapter.features()
            | ((wgpu::Features::TIMESTAMP_QUERY & adapter.features()) | compute(adapter));
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("zerowave video"),
            required_features: wanted,
            required_limits: device_limits(adapter),
            experimental_features: experimental(adapter),
            memory_hints: MEMORY_HINTS,
            ..Default::default()
        }))
        .map_err(|error| Error::NoDevice(error.to_string()))
    }

    /// The device for a Windows decoder (`P010`); the decoder adopts it before decoding.
    #[cfg(all(
        feature = "d3d12",
        windows,
        not(any(feature = "vaapi", all(feature = "nvdec", target_os = "linux"))),
        not(all(feature = "android", target_os = "android"))
    ))]
    fn device_on(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), Error> {
        let wanted = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM & adapter.features()
            | ((wgpu::Features::TIMESTAMP_QUERY & adapter.features()) | compute(adapter));
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("zerowave video"),
            required_features: wanted,
            required_limits: device_limits(adapter),
            experimental_features: experimental(adapter),
            memory_hints: MEMORY_HINTS,
            ..Default::default()
        };
        // Vulkan はデコーダーのフレームを D3D12 から取り込む拡張を足して開く（取り込めなければ普通に開く）。
        let opened = match Self::vulkan_importing(adapter, &descriptor) {
            Some(opened) => opened,
            None => pollster::block_on(adapter.request_device(&descriptor))
                .map_err(|error| Error::NoDevice(error.to_string()))?,
        };
        super::import::d3d12::adopt(&opened.0);
        Ok(opened)
    }

    /// Vulkan のアダプターなら、D3D12 のバッファを取り込む拡張を有効にして開く。拡張が無いか開けなければ `None`。
    #[cfg(all(
        feature = "d3d12",
        windows,
        not(any(feature = "vaapi", all(feature = "nvdec", target_os = "linux"))),
        not(all(feature = "android", target_os = "android"))
    ))]
    fn vulkan_importing(
        adapter: &wgpu::Adapter,
        descriptor: &wgpu::DeviceDescriptor<'_>,
    ) -> Option<(wgpu::Device, wgpu::Queue)> {
        use wgpu::hal::api::Vulkan;

        let wanted = super::import::d3d12_vulkan::EXTENSIONS;
        // SAFETY: the hal adapter is borrowed only for the calls; the callback only appends names.
        let opened = unsafe {
            adapter.as_hal::<Vulkan>().and_then(|hal| {
                let instance = hal.shared_instance().raw_instance();
                let has = instance
                    .enumerate_device_extension_properties(hal.raw_physical_device())
                    .ok()?;
                let all = wanted.iter().all(|name| {
                    has.iter()
                        .any(|found| found.extension_name_as_c_str() == Ok(*name))
                });
                if !all {
                    tracing::info!("Vulkan: D3D12 のバッファを取り込む拡張が無い");
                    return None;
                }
                Some(hal.open_with_callback(
                    descriptor.required_features,
                    &descriptor.required_limits,
                    &descriptor.memory_hints,
                    Some(Box::new(
                        |args: wgpu::hal::vulkan::CreateDeviceCallbackArgs<'_, '_, '_>| {
                            for name in wanted {
                                if !args.extensions.contains(name) {
                                    args.extensions.push(name);
                                }
                            }
                        },
                    )),
                ))
            })
        }?;
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => {
                tracing::warn!("Vulkan: 取り込みの拡張を足して開けない: {error}");
                return None;
            }
        };
        // SAFETY: `opened` came from this adapter just now and is unused.
        match unsafe { adapter.create_device_from_hal(opened, descriptor) } {
            Ok(opened) => Some(opened),
            Err(error) => {
                tracing::warn!("Vulkan: 取り込みの拡張を足したデバイスを包めない: {error}");
                None
            }
        }
    }

    #[cfg(all(feature = "android", target_os = "android"))]
    fn device_on(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), Error> {
        use wgpu::hal::api::Vulkan;

        let wanted = (wgpu::Features::TEXTURE_FORMAT_NV12
            | wgpu::Features::TEXTURE_FORMAT_P010
            | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
            & adapter.features();
        let limits = device_limits(adapter);
        // SAFETY: the hal adapter is borrowed only for the call; the callback only appends names.
        let opened = unsafe {
            adapter.as_hal::<Vulkan>().map(|hal| {
                hal.open_with_callback(
                    wanted,
                    &limits,
                    &MEMORY_HINTS,
                    Some(Box::new(
                        |args: wgpu::hal::vulkan::CreateDeviceCallbackArgs<'_, '_, '_>| {
                            for name in super::import::android::EXTENSIONS {
                                if !args.extensions.contains(name) {
                                    args.extensions.push(name);
                                }
                            }
                        },
                    )),
                )
            })
        };
        let Some(opened) = opened else {
            return pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("zerowave video"),
                required_features: wgpu::Features::empty(),
                required_limits: device_limits(adapter),
                memory_hints: MEMORY_HINTS,
                ..Default::default()
            }))
            .map_err(|error| Error::NoDevice(error.to_string()));
        };
        let opened = opened.map_err(|error| Error::NoDevice(error.to_string()))?;
        // SAFETY: `opened` came from this adapter just now and is unused.
        unsafe {
            adapter.create_device_from_hal(
                opened,
                &wgpu::DeviceDescriptor {
                    label: Some("zerowave video"),
                    required_features: wanted,
                    required_limits: limits,
                    experimental_features: experimental(adapter),
                    memory_hints: MEMORY_HINTS,
                    ..Default::default()
                },
            )
        }
        .map_err(|error| Error::NoDevice(error.to_string()))
    }

    #[cfg(all(
        any(feature = "vaapi", all(feature = "nvdec", target_os = "linux")),
        not(all(feature = "android", target_os = "android"))
    ))]
    fn device_on(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), Error> {
        use wgpu::hal::api::Vulkan;

        #[cfg(feature = "vaapi")]
        super::import::dmabuf::adopt_render_node(adapter);
        #[cfg(feature = "vaapi")]
        let importing = super::import::dmabuf::EXTENSIONS;
        #[cfg(not(feature = "vaapi"))]
        let importing: &[&std::ffi::CStr] = &[];
        let wanted = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM & adapter.features()
            | ((wgpu::Features::TIMESTAMP_QUERY & adapter.features()) | compute(adapter));
        let limits = device_limits(adapter);
        // デバイスが持つ拡張だけ足す（知らない名前を渡すと `vkCreateDevice` の前に落ちる）。
        // SAFETY: hal のアダプターは呼び出しの間だけ借りて読むだけ。
        let wanted_extensions: Vec<&'static std::ffi::CStr> = unsafe {
            adapter
                .as_hal::<Vulkan>()
                .and_then(|hal| {
                    let instance = hal.shared_instance().raw_instance();
                    instance
                        .enumerate_device_extension_properties(hal.raw_physical_device())
                        .ok()
                })
                .map(|have| {
                    let mut wanted: Vec<&'static std::ffi::CStr> = importing.to_vec();
                    for name in crate::output::video::host::vulkan::EXTENSIONS {
                        if !wanted.contains(name) {
                            wanted.push(name);
                        }
                    }
                    wanted
                        .into_iter()
                        .filter(|name| {
                            have.iter()
                                .any(|found| found.extension_name_as_c_str() == Ok(*name))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        // SAFETY: the hal adapter is borrowed only for the call; the callback only appends names.
        let opened = unsafe {
            adapter.as_hal::<Vulkan>().map(|hal| {
                hal.open_with_callback(
                    wanted,
                    &limits,
                    &MEMORY_HINTS,
                    Some(Box::new(
                        |args: wgpu::hal::vulkan::CreateDeviceCallbackArgs<'_, '_, '_>| {
                            for name in &wanted_extensions {
                                if !args.extensions.contains(name) {
                                    args.extensions.push(name);
                                }
                            }
                        },
                    )),
                )
            })
        };
        let Some(opened) = opened else {
            return pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("zerowave video"),
                required_features: wgpu::Features::empty(),
                required_limits: device_limits(adapter),
                memory_hints: MEMORY_HINTS,
                ..Default::default()
            }))
            .map_err(|error| Error::NoDevice(error.to_string()));
        };
        let opened = opened.map_err(|error| Error::NoDevice(error.to_string()))?;
        // SAFETY: `opened` came from this adapter just now and is unused.
        unsafe {
            adapter.create_device_from_hal(
                opened,
                &wgpu::DeviceDescriptor {
                    label: Some("zerowave video"),
                    required_features: wanted,
                    required_limits: limits,
                    experimental_features: experimental(adapter),
                    memory_hints: MEMORY_HINTS,
                    ..Default::default()
                },
            )
        }
        .map_err(|error| Error::NoDevice(error.to_string()))
    }

    pub fn name(&self) -> String {
        let info = self.adapter.get_info();
        format!("{} ({:?}, {})", info.name, info.backend, info.driver)
    }
}

/// アロケーターは小さなブロックから確保する。既定の `Performance` は種類ごとに 128 MiB（GPU）と 64 MiB（転送）から
/// 確保して空になるまで返さないので、デバイス二つで数百 MiB が使われないまま残る。
const MEMORY_HINTS: wgpu::MemoryHints = wgpu::MemoryHints::MemoryUsage;

/// D3D12 はこの数だけシェーダーから見える descriptor heap を作る（既定の 100 万では一つで 32〜64 MB）。
/// 同時に生きているバインドグループのテクスチャとバッファーの数で足りる。
const DESCRIPTORS: u32 = 1 << 16;

fn device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    let mut limits = adapter.limits();
    limits.max_non_sampler_bindings = limits.max_non_sampler_bindings.min(DESCRIPTORS);
    limits
}

/// プラグインの映像フィルター（NN など）のために、あれば有効にするコンピュートの機能。
const COMPUTE: wgpu::Features =
    wgpu::Features::SHADER_F16.union(wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX);

/// [`COMPUTE`] のうちアダプターが持つ物。Vulkan の協調行列は、拡張があっても機能の
/// `VkPhysicalDeviceCooperativeMatrixFeaturesKHR::cooperativeMatrix` が偽なら有効にできない（有効にすると
/// `vkCreateDevice` が `VK_ERROR_FEATURE_NOT_PRESENT` を返す）。wgpu は拡張と形の一覧だけで持つと言うので、
/// 機能をここで確かめる。
fn compute(adapter: &wgpu::Adapter) -> wgpu::Features {
    let mut features = COMPUTE & adapter.features();
    if features.contains(wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX)
        && !cooperative_matrix_enabled(adapter)
    {
        features.remove(wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX);
    }
    features
}

/// Vulkan のアダプターなら協調行列の機能を問い合わせる。ほかのバックエンドは wgpu の答えのまま。
// Apple の wgpu には Vulkan のバックエンドが無い（deck の既定の feature の nvdec は macOS でも入る）。
#[cfg(all(
    any(
        feature = "vaapi",
        feature = "v4l2",
        feature = "nvdec",
        feature = "android"
    ),
    not(target_vendor = "apple")
))]
fn cooperative_matrix_enabled(adapter: &wgpu::Adapter) -> bool {
    use wgpu::hal::api::Vulkan;
    // SAFETY: hal のアダプターは呼び出しの間だけ借りて読むだけ。
    unsafe {
        adapter.as_hal::<Vulkan>().is_none_or(|hal| {
            let mut matrix = ash::vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default();
            let mut features = ash::vk::PhysicalDeviceFeatures2::default().push_next(&mut matrix);
            hal.shared_instance()
                .raw_instance()
                .get_physical_device_features2(hal.raw_physical_device(), &mut features);
            matrix.cooperative_matrix == ash::vk::TRUE
        })
    }
}

#[cfg(not(all(
    any(
        feature = "vaapi",
        feature = "v4l2",
        feature = "nvdec",
        feature = "android"
    ),
    not(target_vendor = "apple")
)))]
fn cooperative_matrix_enabled(_adapter: &wgpu::Adapter) -> bool {
    true
}

/// 協調行列はまだ wgpu の実験の機能。アダプターが持つときだけ許す。
fn experimental(adapter: &wgpu::Adapter) -> wgpu::ExperimentalFeatures {
    if compute(adapter).contains(wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX) {
        // SAFETY: 使うのはプラグインが宣言する WGSL の `wgpu_cooperative_matrix` だけ。
        unsafe { wgpu::ExperimentalFeatures::enabled() }
    } else {
        wgpu::ExperimentalFeatures::disabled()
    }
}

/// 開いたデバイスで使える協調行列の形（デバイスからはアダプターを辿れないので覚えておく）。
static MATRICES: std::sync::Mutex<Vec<(wgpu::Device, Vec<wgpu::CooperativeMatrixProperties>)>> =
    std::sync::Mutex::new(Vec::new());

fn remember_matrices(device: &wgpu::Device, adapter: &wgpu::Adapter) {
    if !device
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX)
    {
        return;
    }
    let Ok(mut known) = MATRICES.lock() else {
        return;
    };
    known.retain(|(other, _)| other != device);
    known.push((device.clone(), adapter.cooperative_matrix_properties()));
}

pub fn matrices(device: &wgpu::Device) -> Vec<wgpu::CooperativeMatrixProperties> {
    MATRICES
        .lock()
        .ok()
        .and_then(|known| {
            known
                .iter()
                .find(|(other, _)| other == device)
                .map(|(_, found)| found.clone())
        })
        .unwrap_or_default()
}

static LOST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 映像のデバイスを開いた回数。前のデバイスの通知が、開き直した後のフラグを立てないように。
static OPENED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// GPU のデバイスを失ったか。失っていれば理由（開き直すにはウィンドウのサーフェスごと作り直す）。
pub fn lost() -> Option<String> {
    LOST.lock().ok().and_then(|lost| lost.clone())
}

fn mark_lost(opened: u64, why: String) {
    if opened != OPENED.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    if let Ok(mut lost) = LOST.lock() {
        lost.get_or_insert(why);
    }
}

/// デコーダーが、レンダリングと共有しているデバイスが外されたのを見つけた。レンダリングの側は
/// ウィンドウが隠れている間は GPU を呼ばないので自分では気づかず、映像が止まったままになる。
/// デバイスを今失っているか。wgpu の知らせ（[`lost`]）は遅れることがあるので、D3D12 ならデバイスに直接
/// 問い合わせる。失っていれば印を立てて理由を返す。
pub fn removed(device: &wgpu::Device) -> Option<String> {
    if let Some(why) = lost() {
        return Some(why);
    }
    #[cfg(all(feature = "d3d12", windows))]
    {
        use wgpu::hal::api::Dx12;
        // SAFETY: as_hal の間だけ借り、参照カウントを増やしたコピーだけを持ち出す。
        let raw = unsafe { device.as_hal::<Dx12>().map(|hal| hal.raw_device().clone()) }?;
        // SAFETY: 参照を持っているデバイス。外れていなければ S_OK。
        if let Err(error) = unsafe { raw.GetDeviceRemovedReason() } {
            let why = format!("D3D12 のデバイスが外れた（{error}）");
            mark_lost(
                OPENED.load(std::sync::atomic::Ordering::Acquire),
                why.clone(),
            );
            return Some(why);
        }
    }
    #[cfg(not(all(feature = "d3d12", windows)))]
    let _ = device;
    None
}

pub fn mark_removed(why: &str) {
    mark_lost(
        OPENED.load(std::sync::atomic::Ordering::Acquire),
        format!("デコーダー：{why}"),
    );
}

/// 試験のためだけに、開いているデバイスを失わせる（D3D12 ではドライバーに外させる）。
pub fn lose_for_test(device: &wgpu::Device) {
    tracing::warn!("試験のため GPU デバイスを失わせる");
    #[cfg(all(feature = "d3d12", windows))]
    {
        use wgpu::hal::api::Dx12;
        use windows::core::Interface;
        // SAFETY: as_hal の間だけ借り、参照カウントを増やしたコピーだけを持ち出す。
        let raw = unsafe { device.as_hal::<Dx12>().map(|hal| hal.raw_device().clone()) };
        if let Some(five) = raw.and_then(|raw| {
            raw.cast::<windows::Win32::Graphics::Direct3D12::ID3D12Device5>()
                .ok()
        }) {
            // SAFETY: 生きているデバイス。以後の呼び出しは DEVICE_REMOVED を返すだけ。
            unsafe { five.RemoveDevice() };
            return;
        }
    }
    // `destroy` は通知が Destroyed になり拾わないので、フラグはここで立てる。
    mark_lost(
        OPENED.load(std::sync::atomic::Ordering::Acquire),
        String::from("試験"),
    );
    device.destroy();
}

/// デバイスが失われたら理由をログに残し、フラグを立てる。
fn say_when_lost(device: &wgpu::Device, which: &'static str, opened: u64) {
    // デバイスを失うとどのコマンドもエラーになり既定では panic するので、最初の何件かだけログに出す。
    let said = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    device.on_uncaptured_error(std::sync::Arc::new(move |error: wgpu::Error| {
        if said.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 5 {
            tracing::warn!("GPU エラー（{which}）: {error}");
        }
    }));
    // D3D12 ならロストの理由（`GetDeviceRemovedReason`）も残す。wgpu の知らせには含まれない。
    #[cfg(all(feature = "d3d12", windows))]
    let raw = {
        use wgpu::hal::api::Dx12;
        // SAFETY: as_hal の間だけ借り、参照カウントを増やしたコピーだけを持ち出す。
        unsafe { device.as_hal::<Dx12>().map(|hal| hal.raw_device().clone()) }
    };
    device.set_device_lost_callback(move |reason, message| {
        if reason != wgpu::DeviceLostReason::Destroyed {
            tracing::warn!("GPU デバイスロスト（{which}、{reason:?}）: {message}");
            #[cfg(all(feature = "d3d12", windows))]
            if let Some(raw) = raw.as_ref() {
                // SAFETY: 参照を持っているデバイス。失われた後も理由を返す。
                let removed = unsafe { raw.GetDeviceRemovedReason() };
                tracing::warn!("GPU デバイスロストの理由（{which}）: {removed:?}");
            }
            mark_lost(opened, format!("{which}：{message}"));
            #[cfg(all(feature = "d3d12", windows))]
            crate::decode::d3d12::dx::report_removed();
        }
    });
}
