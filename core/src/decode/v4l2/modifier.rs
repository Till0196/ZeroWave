// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Asking Vulkan per plane whether it imports the decoder's tiling (SAND), not assuming it.

use crate::decode::v4l2::uapi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub format: u32,
    pub modifier: u64,
    pub planes: u32,
    pub sampled: bool,
}

/// No `P030`: it is not a Vulkan format.
pub fn planes_of(wide: bool) -> [(u32, ash::vk::Format); 2] {
    if wide {
        [
            (uapi::DRM_FORMAT_R16, ash::vk::Format::R16_UNORM),
            (uapi::DRM_FORMAT_GR1616, ash::vk::Format::R16G16_UNORM),
        ]
    } else {
        [
            (uapi::DRM_FORMAT_R8, ash::vk::Format::R8_UNORM),
            (uapi::DRM_FORMAT_GR88, ash::vk::Format::R8G8_UNORM),
        ]
    }
}

pub fn require(modifier: u64, wide: bool) -> Result<(), String> {
    if modifier == uapi::DRM_FORMAT_MOD_LINEAR {
        return Ok(());
    }
    let answers = ask(modifier, wide)?;
    for ((drm, _), answer) in planes_of(wide).into_iter().zip(&answers) {
        if !answer.sampled {
            return Err(format!(
                "Vulkan は修飾子 {modifier:#018x} の {} を取り込めません\
                 （vkGetPhysicalDeviceFormatProperties2 が列挙しませんでした）。\
                 ソフトウェアデコーダーに切り替えます",
                fourcc_text(drm)
            ));
        }
    }
    Ok(())
}

pub fn ask(modifier: u64, wide: bool) -> Result<Vec<Answer>, String> {
    let wanted = planes_of(wide);
    let mut out = Vec::new();
    with_vulkan(|entry, instance| {
        let device = modifier_device(instance)?;
        for (drm, format) in wanted {
            let found = modifiers_of(instance, device, format)
                .into_iter()
                .find(|entry| entry.drm_format_modifier == modifier);
            out.push(Answer {
                format: drm,
                modifier,
                planes: found.map_or(0, |entry| entry.drm_format_modifier_plane_count),
                sampled: found.is_some_and(|entry| {
                    entry
                        .drm_format_modifier_tiling_features
                        .contains(ash::vk::FormatFeatureFlags::SAMPLED_IMAGE)
                }),
            });
        }
        let _ = entry;
        Ok(())
    })?;
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Taken {
    pub value: u64,
    pub planes: u32,
    pub sampled: bool,
}

pub fn all(wide: bool) -> Result<Vec<(u32, Vec<Taken>)>, String> {
    let mut out = Vec::new();
    with_vulkan(|_entry, instance| {
        let device = modifier_device(instance)?;
        for (drm, format) in planes_of(wide) {
            let list = modifiers_of(instance, device, format)
                .into_iter()
                .map(|entry| Taken {
                    value: entry.drm_format_modifier,
                    planes: entry.drm_format_modifier_plane_count,
                    sampled: entry
                        .drm_format_modifier_tiling_features
                        .contains(ash::vk::FormatFeatureFlags::SAMPLED_IMAGE),
                })
                .collect();
            out.push((drm, list));
        }
        Ok(())
    })?;
    Ok(out)
}

fn with_vulkan<F>(body: F) -> Result<(), String>
where
    F: FnOnce(&ash::Entry, &ash::Instance) -> Result<(), String>,
{
    // SAFETY: `Entry::load` returns an error where `libvulkan.so.1` is missing.
    let entry = unsafe { ash::Entry::load() }
        .map_err(|error| format!("Vulkan を開けません（{error}）ので、修飾子を確かめられません"))?;
    let application = ash::vk::ApplicationInfo::default().api_version(ash::vk::API_VERSION_1_1);
    let create = ash::vk::InstanceCreateInfo::default().application_info(&application);
    // SAFETY: `create` outlives the call; the instance is destroyed on every path.
    let instance = unsafe { entry.create_instance(&create, None) }
        .map_err(|error| format!("vkCreateInstance: {error}"))?;
    let outcome = body(&entry, &instance);
    // SAFETY: only physical devices were taken, and the instance owns them.
    unsafe { instance.destroy_instance(None) };
    outcome
}

fn modifier_device(instance: &ash::Instance) -> Result<ash::vk::PhysicalDevice, String> {
    // SAFETY: the instance is alive; only physical-device handles it owns come back.
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|error| format!("vkEnumeratePhysicalDevices: {error}"))?;
    devices
        .into_iter()
        .find(|device| has_modifier_extension(instance, *device))
        .ok_or_else(|| {
            String::from("VK_EXT_image_drm_format_modifier を持つハードウェアデバイスがありません")
        })
}

fn has_modifier_extension(instance: &ash::Instance, device: ash::vk::PhysicalDevice) -> bool {
    // SAFETY: `device` came from this instance, which is still alive.
    let Ok(extensions) = (unsafe { instance.enumerate_device_extension_properties(device) }) else {
        return false;
    };
    extensions.iter().any(|extension| {
        extension.extension_name_as_c_str() == Ok(ash::ext::image_drm_format_modifier::NAME)
    })
}

fn modifiers_of(
    instance: &ash::Instance,
    device: ash::vk::PhysicalDevice,
    format: ash::vk::Format,
) -> Vec<ash::vk::DrmFormatModifierPropertiesEXT> {
    let mut list = ash::vk::DrmFormatModifierPropertiesListEXT::default();
    let mut properties = ash::vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the chain outlives the call; with a null list only the count is written.
    unsafe { instance.get_physical_device_format_properties2(device, format, &mut properties) };
    let count = list.drm_format_modifier_count as usize;
    if count == 0 {
        return Vec::new();
    }
    let mut found = vec![ash::vk::DrmFormatModifierPropertiesEXT::default(); count];
    let mut list = ash::vk::DrmFormatModifierPropertiesListEXT {
        drm_format_modifier_count: count as u32,
        p_drm_format_modifier_properties: found.as_mut_ptr(),
        ..Default::default()
    };
    let mut properties = ash::vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the list has `count` live entries and outlives the call.
    unsafe { instance.get_physical_device_format_properties2(device, format, &mut properties) };
    found.truncate(list.drm_format_modifier_count as usize);
    found
}

pub fn fourcc_text(code: u32) -> String {
    String::from_utf8_lossy(&code.to_le_bytes())
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linear_picture_is_not_asked_about() {
        assert_eq!(require(uapi::DRM_FORMAT_MOD_LINEAR, false), Ok(()));
        assert_eq!(require(uapi::DRM_FORMAT_MOD_LINEAR, true), Ok(()));
    }

    #[test]
    fn a_tiled_picture_is_refused_where_vulkan_cannot_be_asked() {
        let outcome = require(uapi::sand128_col_height(1088), false);
        if let Err(why) = outcome {
            assert!(!why.is_empty(), "a refusal has to say why");
        }
    }

    #[test]
    fn the_plane_formats_are_the_ones_the_import_path_makes() {
        assert_eq!(planes_of(false)[0].0, uapi::DRM_FORMAT_R8);
        assert_eq!(planes_of(false)[1].0, uapi::DRM_FORMAT_GR88);
        assert_eq!(planes_of(true)[0].0, uapi::DRM_FORMAT_R16);
        assert_eq!(planes_of(true)[1].0, uapi::DRM_FORMAT_GR1616);
    }

    #[test]
    fn a_four_character_code_reads_as_its_letters() {
        assert_eq!(fourcc_text(uapi::DRM_FORMAT_R8), "R8");
        assert_eq!(fourcc_text(uapi::DRM_FORMAT_NV12), "NV12");
        assert_eq!(fourcc_text(uapi::DRM_FORMAT_P030), "P030");
    }
}
