// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use zwsdk::video::process::{self as wire, Format, Way as WayId};

use super::planar::{Params, Planar};
use super::{Gpu, only};

#[test]
fn only_the_allowed_ways_are_offered_in_order() {
    let found = vec![
        wire::Offer::new(WayId::D3d12, vec![1; 8]),
        wire::Offer::new(WayId::Shm, Vec::new()),
    ];
    assert_eq!(only(found.clone(), &[]), found, "空なら全部");
    let shm = only(found.clone(), &[String::from("shm")]);
    assert_eq!(shm, vec![wire::Offer::new(WayId::Shm, Vec::new())]);
    assert!(only(found, &[String::from("metal")]).is_empty());
}

#[test]
fn params_follow_the_picture_layout() {
    let stride = wire::stride(5, Format::F16);
    let params = Params::new([5, 3], stride, Format::F16);
    assert_eq!(
        (params.stride_words, params.half),
        (3, 1),
        "f16 の 5 画素は 3 ワード"
    );
    assert_eq!(params.pack_groups(), [1, 9, 1], "プレーンと行で 3 × 3");
    assert_eq!(params.unpack_groups(), [1, 1, 1]);
    let bytes = Params::new([1920, 1080], wire::stride(1920, Format::F32), Format::F32).bytes();
    assert_eq!(&bytes[8..16], &[0x80, 7, 0, 0, 0, 0, 0, 0]);
}

/// GPU でパックしたデータがプロトコルのレイアウトどおりで、戻すと元のフレームになる（GPU が無ければスキップ）。
#[test]
fn packing_follows_the_layout_and_goes_round() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("GPU が無いのでスキップ");
        return;
    };
    let Ok((device, queue)) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
    else {
        eprintln!("GPU が開けないのでスキップ");
        return;
    };
    let size = [5u32, 3];
    // 画素 (x, y) の色は (x/8, y/8, (x+y)/16)。
    let colour = |x: u32, y: u32| [x as f32 / 8.0, y as f32 / 8.0, (x + y) as f32 / 16.0];
    let texels: Vec<f32> = (0..size[1])
        .flat_map(|y| {
            (0..size[0]).flat_map(move |x| [colour(x, y)[0], colour(x, y)[1], colour(x, y)[2], 1.0])
        })
        .collect();
    let extent = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
    };
    let picture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("試験"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let raw: Vec<u8> = texels.iter().flat_map(|v| v.to_le_bytes()).collect();
    queue.write_texture(
        picture.as_image_copy(),
        &raw,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 16),
            rows_per_image: None,
        },
        extent,
    );
    for format in [Format::F32, Format::F16] {
        let stride = wire::stride(size[0], format);
        let bytes = wire::picture_bytes(size[1], stride);
        let planes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("試験"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let back = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("試験"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("試験"),
            size: bytes + 256 * u64::from(size[1]),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let planar = Planar::new(&device, size, stride, format);
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut gpu = Gpu {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
        };
        planar.pack(&mut gpu, &picture, &planes);
        planar.unpack(&mut gpu, &planes, &back);
        encoder.copy_buffer_to_buffer(&planes, 0, &read, 0, bytes);
        encoder.copy_texture_to_buffer(
            back.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: bytes.next_multiple_of(256),
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            extent,
        );
        queue.submit([encoder.finish()]);
        read.map_async(wgpu::MapMode::Read, .., |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let view = read.get_mapped_range(..).expect("読める");
        let half = |b: &[u8]| {
            let bits = u16::from_le_bytes([b[0], b[1]]);
            let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
            let exp = i32::from((bits >> 10) & 0x1f);
            let frac = f32::from(bits & 0x3ff);
            sign * if exp == 0 {
                frac * 2f32.powi(-24)
            } else {
                (1.0 + frac / 1024.0) * 2f32.powi(exp - 15)
            }
        };
        for c in 0..3u32 {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let at = ((c * size[1] + y) * stride + x * format.bytes()) as usize;
                    let got = match format {
                        Format::F32 => f32::from_le_bytes(view[at..at + 4].try_into().expect("4")),
                        Format::F16 => half(&view[at..at + 2]),
                    };
                    let want = colour(x, y)[c as usize];
                    assert!(
                        (got - want).abs() < 1e-3,
                        "{format:?} プレーン {c} ({x}, {y})：{got} と {want}"
                    );
                    let back_at =
                        bytes.next_multiple_of(256) as usize + (y * 256 + x * 8 + c * 2) as usize;
                    let round = half(&view[back_at..back_at + 2]);
                    assert!(
                        (round - want).abs() < 1e-3,
                        "{format:?} 戻し ({x}, {y}, {c})：{round} と {want}"
                    );
                }
            }
        }
        if format == Format::F16 {
            let pad = (stride - 2) as usize;
            assert_eq!(&view[pad..pad + 2], &[0, 0]);
        }
    }
}
