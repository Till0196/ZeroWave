// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! メインメモリを通す受け渡しの、レンダリング側の GPU の処理（バックエンドの GPU と共有できないときのフォールバック）。
//! バイトの並びは [`Layout`] で、バックエンドの側も同じ並びで読み書きする。

use super::Gpu;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Plane {
    pub offset: u64,
    /// 256 の倍数（wgpu のコピーの制約）。
    pub row_pitch: u32,
    pub rows: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Layout {
    pub planes: Vec<Plane>,
    pub total: u64,
}

impl Layout {
    #[cfg_attr(windows, allow(dead_code))]
    pub fn of(planes: &[(wgpu::TextureFormat, [u32; 2])]) -> Self {
        let mut offset = 0u64;
        let planes = planes
            .iter()
            .map(|(format, size)| {
                let bytes = format.block_copy_size(None).unwrap_or(4);
                let row_pitch = (size[0] * bytes).next_multiple_of(256);
                let plane = Plane {
                    offset,
                    row_pitch,
                    rows: size[1],
                };
                offset = (offset + u64::from(row_pitch) * u64::from(size[1])).next_multiple_of(512);
                plane
            })
            .collect();
        Self {
            planes,
            total: offset,
        }
    }

    fn copy_layout(plane: &Plane) -> wgpu::TexelCopyBufferLayout {
        wgpu::TexelCopyBufferLayout {
            offset: plane.offset,
            bytes_per_row: Some(plane.row_pitch),
            rows_per_image: Some(plane.rows),
        }
    }
}

pub(super) struct Staging {
    pub layout: Layout,
    download: wgpu::Buffer,
    upload: wgpu::Buffer,
}

impl Staging {
    pub fn new(device: &wgpu::Device, layout: Layout, label: &str) -> Self {
        let download = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: layout.total,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let upload = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: layout.total,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            layout,
            download,
            upload,
        }
    }

    fn record_download(&self, encoder: &mut wgpu::CommandEncoder, planes: &[wgpu::Texture]) {
        for (texture, plane) in planes.iter().zip(&self.layout.planes) {
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.download,
                    layout: Layout::copy_layout(plane),
                },
                texture.size(),
            );
        }
    }

    /// 次のサブミットでコピーされる。
    pub fn upload(&self, gpu: &mut Gpu<'_>, planes: &[wgpu::Texture], bytes: &[u8]) {
        gpu.queue.write_buffer(&self.upload, 0, bytes);
        for (texture, plane) in planes.iter().zip(&self.layout.planes) {
            gpu.encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.upload,
                    layout: Layout::copy_layout(plane),
                },
                texture.as_image_copy(),
                texture.size(),
            );
        }
    }
}

/// レンダリング側の GPU を待つ。
pub(super) fn download(
    gpu: &mut Gpu<'_>,
    lanes: &[(&Staging, &[wgpu::Texture])],
    mut each: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    for (staging, planes) in lanes {
        staging.record_download(gpu.encoder, planes);
    }
    gpu.flush();
    for (staging, _) in lanes {
        staging.download.map_async(wgpu::MapMode::Read, .., |_| {});
    }
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("リードバックの待機：{e}"))?;
    let mut result = Ok(());
    for (index, (staging, _)) in lanes.iter().enumerate() {
        if result.is_ok() {
            result = match staging.download.get_mapped_range(..) {
                Ok(view) => each(index, &view),
                Err(e) => Err(format!("リードバックできない：{e:?}")),
            };
        }
        staging.download.unmap();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_256_and_planes_are_512_aligned() {
        let layout = Layout::of(&[
            (wgpu::TextureFormat::R8Unorm, [1440, 1080]),
            (wgpu::TextureFormat::Rg8Unorm, [720, 540]),
        ]);
        assert_eq!(layout.planes[0].row_pitch, 1536);
        assert_eq!(layout.planes[1].offset, 1536 * 1080);
        assert_eq!(layout.planes[1].row_pitch, 1536);
        assert_eq!(
            layout.total,
            (1536 * 1080 + 1536 * 540u64).next_multiple_of(512)
        );
    }
}
