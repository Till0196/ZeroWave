// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 録画1本。コンテナは拡張子で決める（途中から始まる録画ではバイト列から判定できない）。

use std::fs::File;
use std::io::BufReader;

use super::{Bytes, Channel, Error, Source};
use crate::filters::Container;

#[derive(Debug)]
pub struct Recording {
    path: String,
    container: Container,
    name: String,
}

impl Recording {
    /// 開けるかだけを確かめ、ファイルは開いたままにしない。
    pub fn of_path(path: &str) -> Result<Self, Error> {
        let Some(container) = Container::of_path(path) else {
            return Err(Error::Refused(format!(
                "拡張子からコンテナが分からない（.ts か .tlv が必要）: {path}"
            )));
        };
        if let Err(why) = File::open(path) {
            return Err(Error::Unreachable(format!("{path}: {why}")));
        }
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
        Ok(Self {
            path: path.to_owned(),
            container,
            name,
        })
    }

    pub fn container(&self) -> Container {
        self.container
    }
}

impl Source for Recording {
    fn name(&self) -> &str {
        "録画"
    }

    fn channels(&mut self) -> Result<Vec<Channel>, Error> {
        Ok(vec![Channel {
            name: self.name.clone(),
            key: None,
            order: None,
            transport: None,
            how: self.path.clone(),
            container: self.container,
        }])
    }

    fn open(&mut self, channel: &Channel) -> Result<(Bytes, Container), Error> {
        #[cfg(feature = "plugins")]
        crate::plugins::lend(None);
        // 別のソースのチャンネルを渡されたら、黙って自分のファイルを開かない。
        if channel.how != self.path {
            return Err(Error::Refused(format!(
                "この録画のものではないチャンネル: {}",
                channel.how
            )));
        }
        let file = File::open(&self.path).map_err(|why| Error::Unreachable(why.to_string()))?;
        Ok((Box::new(BufReader::new(file)), self.container))
    }
}
