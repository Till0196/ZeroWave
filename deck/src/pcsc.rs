// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! PC/SC 互換ソケットを配置する（`[[source]]` の `pcsc = true` ごとに一つ）。

use zwcore::input::source::pcsc::Pcsc;

pub fn apply(file: &crate::config::File) {
    let wanted: Vec<(String, std::path::PathBuf)> = file
        .sources
        .iter()
        .filter(|source| source.kind == "coaxd" && source.pcsc)
        .map(|source| {
            let path = match source.pcsc_socket.trim() {
                "" => default_socket(),
                written => std::path::PathBuf::from(written),
            };
            (source.endpoint.clone(), path)
        })
        .collect();
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    running.retain(|pcsc| {
        wanted
            .iter()
            .any(|(endpoint, path)| pcsc.path() == path && pcsc.serves(endpoint))
    });
    for (endpoint, path) in wanted {
        if running.iter().any(|pcsc| pcsc.path() == path) {
            continue;
        }
        match Pcsc::start(&endpoint, &path, |said| tracing::info!("PC/SC: {said}")) {
            Ok(pcsc) => {
                tracing::info!(
                    "PC/SC: {} {}",
                    pcsc.path().display(),
                    if pcsc.ours() {
                        "で coaxd を公開する"
                    } else {
                        "は他のプロセスが公開済み"
                    }
                );
                running.push(pcsc);
            }
            Err(error) => tracing::warn!("PC/SC: {error}"),
        }
    }
}

static RUNNING: std::sync::Mutex<Vec<Pcsc>> = std::sync::Mutex::new(Vec::new());

pub fn default_socket() -> std::path::PathBuf {
    zwcore::input::source::pcsc::default_socket()
}
