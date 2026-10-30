// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl Device {
    pub fn wait(&self, value: u64) -> Result<(), Error> {
        self.event.wait_fence(&self.fence, value)
    }

    /// 配列のスライスを同じ形式のテクスチャへコピーキューでコピーする。返す値だけを待てば足りる。
    pub fn copy_planes(
        &mut self,
        from: &ID3D12Resource,
        planes: [u32; 2],
        into: &ID3D12Resource,
        after: u64,
    ) -> Result<u64, Error> {
        self.ensure_copy()?;
        let video = self.fence.clone();
        let Some(copy) = self.copy.as_mut() else {
            return Err(Error::Unavailable(String::from("コピーキューがありません")));
        };
        copy.begin(Some((&video, after)))?;
        // プレーンごとにコピーする。`CopyResource` はコピーキューで色差をコピーしないことがある。
        for plane in 0..2u32 {
            copy.region(
                &subresource(into, plane),
                &subresource(from, planes[plane as usize]),
            );
        }
        let value = copy.submit()?;
        // 次のデコードは、このコピーが読み終わるまでコピー元へ書けない。
        self.pending_copy = value;
        Ok(value)
    }

    pub fn wait_copy(&mut self, value: u64) -> Result<(), Error> {
        match self.copy.as_ref() {
            Some(copy) => copy.wait(value),
            None => Ok(()),
        }
    }

    /// デコードしたフレームをプレーンごとにメインメモリへ読み戻す。
    pub fn read_back(
        &mut self,
        texture: &ID3D12Resource,
        planes: usize,
    ) -> Result<Readback, Error> {
        self.ensure_copy()?;
        let (layouts, rows, total) = footprints(&self.raw, &describe(texture), planes);
        let total = whole_rows(&layouts[..planes], &rows[..planes], total);
        let buffer = self.buffer(
            total,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let Some(copy) = self.copy.as_mut() else {
            return Err(Error::Unavailable(String::from("コピーキューがありません")));
        };
        copy.begin(None)?;
        copy.planes_into(texture, &buffer, &layouts[..planes]);
        let value = copy.submit()?;
        copy.wait(value)?;

        let total = total as usize;
        let mapping = Mapping::new(&buffer, total, 0..total)?;
        let mut out = Vec::with_capacity(planes);
        for (layout, rows) in layouts.iter().zip(&rows).take(planes) {
            let pitch = layout.Footprint.RowPitch as usize;
            let height = *rows as usize;
            let offset = layout.Offset as usize;
            let bytes = mapping.bytes(offset..offset + pitch * height).to_vec();
            out.push((bytes, pitch, height));
        }
        drop(mapping);
        Ok(Readback { planes: out })
    }

    pub(super) fn ensure_copy(&mut self) -> Result<(), Error> {
        if self.copy.is_some() {
            return Ok(());
        }
        self.copy = Some(CopyQueue::new(&self.raw)?);
        Ok(())
    }

    /// ドライバーが列挙するプロファイル（ログ用）。ハードウェアが対応しているとは限らない。
    pub fn profiles(&self) -> String {
        let Some(video) = self.video.as_ref() else {
            return String::from("なし（D3D12 のビデオデコードが無い）");
        };
        let mut count = D3D12_FEATURE_DATA_VIDEO_DECODE_PROFILE_COUNT {
            NodeIndex: 0,
            ProfileCount: 0,
        };
        // SAFETY: a live local of exactly the size given; the call writes only the count.
        let asked = unsafe {
            video.CheckFeatureSupport(
                D3D12_FEATURE_VIDEO_DECODE_PROFILE_COUNT,
                std::ptr::from_mut(&mut count).cast(),
                size_of::<D3D12_FEATURE_DATA_VIDEO_DECODE_PROFILE_COUNT>() as u32,
            )
        };
        if asked.is_err() || count.ProfileCount == 0 {
            return String::from("なし");
        }
        let mut guids = vec![GUID::zeroed(); count.ProfileCount as usize];
        let mut list = D3D12_FEATURE_DATA_VIDEO_DECODE_PROFILES {
            NodeIndex: 0,
            ProfileCount: count.ProfileCount,
            pProfiles: guids.as_mut_ptr(),
        };
        // SAFETY: `list` names a buffer of exactly `ProfileCount` GUIDs that outlives the call.
        let asked = unsafe {
            video.CheckFeatureSupport(
                D3D12_FEATURE_VIDEO_DECODE_PROFILES,
                std::ptr::from_mut(&mut list).cast(),
                size_of::<D3D12_FEATURE_DATA_VIDEO_DECODE_PROFILES>() as u32,
            )
        };
        if asked.is_err() {
            return String::from("?");
        }
        guids
            .iter()
            .map(|guid| profile_name(*guid))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn pci(&self) -> Option<(u32, u32)> {
        let adapter: IDXGIAdapter1 = adapter_by_luid(luid(&self.raw)).ok()?;
        let description = describe_adapter(&adapter)?;
        Some((description.VendorId, description.DeviceId))
    }

    pub fn name(&self) -> String {
        let mut description = D3D12_FEATURE_DATA_ARCHITECTURE {
            NodeIndex: 0,
            ..Default::default()
        };
        // SAFETY: the pointer is to a live local of exactly the size given.
        let _ = unsafe {
            self.raw.CheckFeatureSupport(
                D3D12_FEATURE_ARCHITECTURE,
                std::ptr::from_mut(&mut description).cast(),
                size_of::<D3D12_FEATURE_DATA_ARCHITECTURE>() as u32,
            )
        };
        if !self.shared && !self.adapter.is_empty() {
            return format!(
                "D3D12 ({}・GPU 間転送, UMA: {})",
                self.adapter,
                description.UMA.as_bool()
            );
        }
        format!("D3D12 (UMA: {})", description.UMA.as_bool())
    }
}

pub fn profile_name(guid: GUID) -> String {
    let known = [
        (
            D3D12_VIDEO_DECODE_PROFILE_MPEG1_AND_MPEG2,
            "MPEG1_AND_MPEG2",
        ),
        (D3D12_VIDEO_DECODE_PROFILE_MPEG2, "MPEG2"),
        (D3D12_VIDEO_DECODE_PROFILE_H264, "H264"),
        (D3D12_VIDEO_DECODE_PROFILE_HEVC_MAIN, "HEVC_MAIN"),
        (D3D12_VIDEO_DECODE_PROFILE_HEVC_MAIN10, "HEVC_MAIN10"),
        (D3D12_VIDEO_DECODE_PROFILE_VC1, "VC1"),
        (D3D12_VIDEO_DECODE_PROFILE_VC1_D2010, "VC1_D2010"),
        (D3D12_VIDEO_DECODE_PROFILE_VP9, "VP9"),
        (D3D12_VIDEO_DECODE_PROFILE_VP9_10BIT_PROFILE2, "VP9_10BIT"),
        (D3D12_VIDEO_DECODE_PROFILE_AV1_PROFILE0, "AV1"),
    ];
    match known.iter().find(|(known, _)| *known == guid) {
        Some((_, name)) => String::from(*name),
        None => format!("{guid:?}"),
    }
}
