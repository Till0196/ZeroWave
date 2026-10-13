// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! BS・CS（とケーブル）の局ロゴを全受信機共通データのカルーセル（ARIB TR-B15、JCTEA STD-007）
//! から作るフィルター。告知（SDTT）とカルーセルは同時には来ないので待たず、データの識別子は全部開く。

use std::collections::BTreeMap;

use mpegts::dsmcc::{Brought, Carousel};

use super::{Kind, Program};

#[derive(Debug)]
enum Watched {
    Gathering(Carousel),
    /// 次の DII までバッファしない。
    NotLogos(u32),
}

#[derive(Debug, Default)]
pub struct Logos {
    watched: BTreeMap<u16, Watched>,
}

impl Logos {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_program(&mut self, program: &Program) {
        for stream in &program.streams {
            if stream.kind == Kind::Data {
                self.watched
                    .entry(stream.id)
                    .or_insert_with(|| Watched::Gathering(Carousel::new()));
            }
        }
    }

    pub fn wants(&self, pid: u16) -> bool {
        self.watched.contains_key(&pid)
    }

    pub fn section(&mut self, pid: u16, section: &[u8]) -> Option<isdb::logo::LogoModule> {
        let watched = self.watched.get_mut(&pid)?;
        if let Watched::NotLogos(transaction) = watched {
            // DII が替わったときだけ見直す。
            if section.first() != Some(&mpegts::dsmcc::TABLE_DII) {
                return None;
            }
            let dii = mpegts::dsmcc::dii(section)?;
            if dii.transaction_id == *transaction {
                return None;
            }
            *watched = Watched::Gathering(Carousel::new());
        }
        let Watched::Gathering(carousel) = watched else {
            return None;
        };
        match carousel.push(section)? {
            Brought::Listed(dii) => {
                if !dii
                    .modules
                    .iter()
                    .any(|module| isdb::logo::is_logo_module(&module.info))
                {
                    *watched = Watched::NotLogos(dii.transaction_id);
                }
                None
            }
            Brought::Module(module) if isdb::logo::is_logo_module(&module.info) => {
                isdb::logo::logo_module(&module.data)
            }
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        self.watched.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::Stream;

    fn program(number: u16, pid: u16, kind: Kind) -> Program {
        Program {
            number,
            map: 0x100,
            streams: vec![Stream {
                id: pid,
                kind,
                framing: mediadecoder::es::Framing::StartCodes,
                declared: "0x0d".to_owned(),
                tag: Some(0x79),
                component: None,
                name: None,
                hierarchy: None,
                data: None,
                decode: None,
            }],
            pcr: 0x1fff,
            ecm: Vec::new(),
            application: None,
            region: None,
            emergency_news: None,
            emergency: Vec::new(),
        }
    }

    fn dii(transaction: u32, download: u32, modules: &[(u16, u32, &[u8])]) -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(&download.to_be_bytes());
        message.extend_from_slice(&[0x00, 0x10]);
        message.extend_from_slice(&[0; 10]);
        message.extend_from_slice(&[0x00, 0x00]);
        message.extend_from_slice(&(modules.len() as u16).to_be_bytes());
        for (id, size, info) in modules {
            message.extend_from_slice(&id.to_be_bytes());
            message.extend_from_slice(&size.to_be_bytes());
            message.push(1);
            message.push(info.len() as u8);
            message.extend_from_slice(info);
        }
        message.extend_from_slice(&[0x00, 0x00]);
        let mut body = vec![0x11, 0x03, 0x10, 0x02];
        body.extend_from_slice(&transaction.to_be_bytes());
        body.push(0xff);
        body.push(0);
        body.extend_from_slice(&(message.len() as u16).to_be_bytes());
        body.extend_from_slice(&message);
        section(0x3b, &body)
    }

    fn ddb(download: u32, module: u16, block: u16, data: &[u8]) -> Vec<u8> {
        let mut message = module.to_be_bytes().to_vec();
        message.push(1);
        message.push(0);
        message.extend_from_slice(&block.to_be_bytes());
        message.extend_from_slice(data);
        let mut body = vec![0x11, 0x03, 0x10, 0x03];
        body.extend_from_slice(&download.to_be_bytes());
        body.push(0xff);
        body.push(0);
        body.extend_from_slice(&(message.len() as u16).to_be_bytes());
        body.extend_from_slice(&message);
        section(0x3c, &body)
    }

    fn section(table: u8, body: &[u8]) -> Vec<u8> {
        let length = body.len() + 5 + 4;
        let mut out = vec![table, 0xb0 | (length >> 8) as u8, length as u8];
        out.extend_from_slice(&[0x00, 0x01, 0xc1, 0x00, 0x00]);
        out.extend_from_slice(body);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    fn logo_module_bytes() -> Vec<u8> {
        let mut bytes = vec![0x02, 0x00, 0x01];
        bytes.extend_from_slice(&[0x00, 0x2c, 1]);
        bytes.extend_from_slice(&[0x00, 0x04, 0x40, 0xf1, 0x00, 0x65]);
        bytes.extend_from_slice(&[0x00, 0x03, 9, 9, 9]);
        bytes
    }

    #[test]
    fn logo_modules_are_gathered_from_any_data_stream_that_names_them() {
        let mut logos = Logos::new();
        logos.on_program(&program(929, 0x779, Kind::Data));
        logos.on_program(&program(101, 0x140, Kind::Data));
        logos.on_program(&program(
            101,
            0x100,
            Kind::Video(mediadecoder::es::Codec::H264),
        ));
        assert!(logos.wants(0x779));
        assert!(logos.wants(0x140));
        assert!(!logos.wants(0x100));
        let plain: &[u8] = b"\x02\x05plain";
        assert!(logos.section(0x140, &dii(7, 1, &[(1, 3, plain)])).is_none());
        assert!(matches!(logos.watched[&0x140], Watched::NotLogos(7)));
        assert!(logos.section(0x140, &ddb(1, 1, 0, &[1, 2, 3])).is_none());
        assert!(logos.section(0x140, &dii(7, 1, &[(1, 3, plain)])).is_none());
        assert!(matches!(logos.watched[&0x140], Watched::NotLogos(7)));
        assert!(logos.section(0x140, &dii(8, 1, &[(1, 3, plain)])).is_none());
        assert!(matches!(logos.watched[&0x140], Watched::NotLogos(8)));
        let module = logo_module_bytes();
        let named: &[u8] = b"\x02\x07LOGO-02";
        let listed = dii(0x8000_0000, 0x5908, &[(2, module.len() as u32, named)]);
        assert!(logos.section(0x779, &listed).is_none());
        assert!(matches!(logos.watched[&0x779], Watched::Gathering(_)));
        assert!(
            logos
                .section(0x779, &ddb(0x5908, 2, 0, &module[..16]))
                .is_none()
        );
        let got = logos
            .section(0x779, &ddb(0x5908, 2, 1, &module[16..]))
            .expect("モジュールが揃った");
        assert_eq!(got.logo_type, 2);
        assert_eq!(got.entries[0].logo_id, 0x2c);
        assert_eq!(got.entries[0].services, vec![(4, 0x40f1, 0x65)]);
        assert_eq!(got.entries[0].png, vec![9, 9, 9]);
        logos.reset();
        assert!(!logos.wants(0x779));
    }
}
