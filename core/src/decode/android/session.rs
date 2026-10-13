// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Opening a session: the format declared, the routes tried, tearing down.

use super::*;

impl Android {
    /// フォーマットに合わせてデコーダーを開く。configure 済みは変えられないので、フォーマットが変われば作り直す。
    pub(super) fn configure(&mut self, format: Format) -> Result<(), Error> {
        if let Some(session) = self.session.as_mut()
            && crate::decode::session_fits(session.format, format)
        {
            session.format = format;
            return Ok(());
        }
        self.teardown();
        crate::decode::openable(format)?;
        if self.wanted == Route::Direct && self.window.is_none() {
            return Err(Error::Unsupported(
                "直接出力の経路には出力先のウィンドウが要ります",
            ));
        }
        // AImageReader、デコーダーのメモリーの順に試す（宣言は当てにならず、エラーは start でも返る）。
        // メモリー経路ではカラーフォーマットを名前で先に指定する。Flexible だと独自のレイアウトで返す。
        let mut last = None;
        for (route, colour) in Self::attempts(self.wanted, format.depth) {
            match self.start(format, route, colour) {
                Ok(session) => {
                    let settled = format!(
                        "経路が決まりました: {}{}",
                        session.route.name(),
                        match colour {
                            Some(colour) => format!("（カラーフォーマット {colour:#x}）"),
                            None => String::new(),
                        }
                    );
                    self.session = Some(session);
                    self.note(&settled);
                    return Ok(());
                }
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(|| Error::Unavailable(String::from("デコーダーを開けません"))))
    }

    /// 指定された経路で試す順。10 ビットに 8 ビットのレイアウトを指定すると黙って間違う。
    pub(super) fn attempts(wanted: Route, depth: u8) -> Vec<(Route, Option<i32>)> {
        let memory: Vec<(Route, Option<i32>)> = if depth > 8 {
            vec![
                (Route::Memory, Some(media::COLOR_FORMAT_YUV_P010)),
                (Route::Memory, Some(media::COLOR_FORMAT_YUV420_FLEXIBLE)),
            ]
        } else {
            vec![
                (Route::Memory, Some(media::COLOR_FORMAT_YUV420_SEMI_PLANAR)),
                (Route::Memory, Some(media::COLOR_FORMAT_YUV420_PLANAR)),
                (Route::Memory, Some(media::COLOR_FORMAT_YUV420_FLEXIBLE)),
            ]
        };
        match wanted {
            Route::Buffers => std::iter::once((Route::Buffers, None))
                .chain(memory)
                .collect(),
            Route::Direct => vec![(Route::Direct, None)],
            Route::Memory => memory,
        }
    }

    pub(super) fn declare(
        &mut self,
        format: Format,
        colour: Option<i32>,
    ) -> Result<MediaFormat, Error> {
        let mime = media::mime(self.codec);
        let mut declared = MediaFormat::new();
        declared.set_str("mime", mime);
        declared.set_i32("width", format.width as i32);
        declared.set_i32("height", format.height as i32);
        if format.rate.1 != 0 && format.rate.0 != 0 {
            declared.set_i32("frame-rate", (format.rate.0 / format.rate.1).max(1) as i32);
        }
        match self.csd.clone() {
            Some(csd) => {
                self.note(&format!("csd-0 に {} バイト渡します", csd.len()));
                declared.set_buffer("csd-0", &csd);
            }
            None if self.codec == Codec::Mpeg2 => {
                self.note("**csd-0 がありません。**シーケンスヘッダが見つかりませんでした");
            }
            None => {}
        }
        if let Some(colour) = colour {
            declared.set_i32("color-format", colour);
        }
        for (key, value) in &self.extra {
            declared.set_i32(key, *value);
        }
        if !self.extra.is_empty() {
            let listed: Vec<String> = self
                .extra
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            let listed = listed.join(", ");
            self.note(&format!("フォーマットに足す {listed}"));
        }
        Ok(declared)
    }

    pub(super) fn start(
        &mut self,
        format: Format,
        route: Route,
        colour: Option<i32>,
    ) -> Result<Session, Error> {
        let mime = media::mime(self.codec);
        // 直接出力は予備を捨ててから開く。一インスタンスしか許さないデコーダーがある。
        if route == Route::Direct {
            let Some(window) = self.window.clone() else {
                return Err(Error::Unsupported(
                    "直接出力の経路には出力先のウィンドウが要ります",
                ));
            };
            drop(self.spare.take());
            let direct = media::Direct::open(self.chosen.as_deref(), mime)?;
            let declared = self.declare(format, None)?;
            self.note("AMediaCodec_configure（直接出力）");
            direct.start(&declared, &window)?;
            return Ok(Session {
                codec: None,
                direct: Some(direct),
                images: None,
                route,
                format,
            });
        }
        let codec = match self.spare.take() {
            Some(codec) => codec,
            None => {
                let opened = match &self.chosen {
                    Some(name) => MediaCodec::from_codec_name(name),
                    None => MediaCodec::from_decoder_type(mime),
                }
                .ok_or_else(|| Error::Unavailable(format!("{mime} のデコーダーを開けません")))?;
                // SAFETY: `Send1` を見よ。
                unsafe { Send1::new(opened) }
            }
        };

        let mut declared = self.declare(
            format,
            match route {
                Route::Buffers => None,
                _ => Some(colour.unwrap_or(media::COLOR_FORMAT_YUV420_FLEXIBLE)),
            },
        )?;
        let _ = &mut declared;
        let images = match route {
            Route::Buffers => {
                self.note(&format!(
                    "AImageReader を作る（{IMAGES} 枚、{}x{}、{}）",
                    format.width,
                    format.height,
                    if format.depth > 8 {
                        "YCBCR_P010"
                    } else {
                        "YUV_420_888"
                    }
                ));
                Some(Images::open(format, IMAGES)?)
            }
            _ => {
                let colour = colour.unwrap_or(media::COLOR_FORMAT_YUV420_FLEXIBLE);
                declared.set_i32("color-format", colour);
                None
            }
        };
        self.note(&match colour {
            Some(colour) => format!(
                "AMediaCodec_configure（{}、カラーフォーマット {colour:#x}）",
                route.name()
            ),
            None => format!("AMediaCodec_configure（{}）", route.name()),
        });
        codec
            .configure(
                &declared,
                images.as_ref().map(Images::window),
                MediaCodecDirection::Decoder,
            )
            .map_err(|error| Error::Unavailable(format!("AMediaCodec_configure: {error}")))?;
        self.note("AMediaCodec_start");
        codec
            .start()
            .map_err(|error| Error::Unavailable(format!("AMediaCodec_start: {error}")))?;
        Ok(Session {
            codec: Some(codec),
            direct: None,
            images,
            route,
            format,
        })
    }

    /// MPEG-2 のフィールド対をそろえて一つのバッファーで渡す（一つずつだと出力しないデコーダーがある）。
    pub(super) fn pair(&mut self, tag: u64, parity: Structure, bytes: &[u8]) -> Result<(), Error> {
        match self.half.take() {
            Some((first, mut held)) if self.parity != Some(parity) => {
                held.extend_from_slice(bytes);
                self.step(step::PAIRED, "フィールド対を1つのバッファにまとめます");
                self.parity = None;
                self.submit(first, &held)
            }
            // 同じパリティーが続いた（一つ欠けた）。保持していたフィールドを出し、新しい対を始める。
            Some((first, held)) => {
                self.submit(first, &held)?;
                self.half = Some((tag, bytes.to_vec()));
                self.parity = Some(parity);
                Ok(())
            }
            None => {
                self.half = Some((tag, bytes.to_vec()));
                self.parity = Some(parity);
                Ok(())
            }
        }
    }
}
