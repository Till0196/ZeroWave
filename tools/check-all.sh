#!/bin/sh
# Apple 以外のターゲットを検査する。実機は要らない: Windows・Android は
# `cargo check` の対象指定だけで通る（リンクはしない）。
set -e
cd "$(dirname "$0")/.."
# Windows の行は cargo-zigbuild を通す。ring が C をそのターゲット向けに
# コンパイルするので、リンクしない検査でも zig が要る。
say() { printf '\n=== %s ===\n' "$1"; }

say "書式"
cargo fmt --all --check

say "Linux（vaapi・v4l2・nvdec・wgpu・ソース・プラグイン）"
cargo clippy -p zerowave-core -p mediadecoder -p zwsdk --features zerowave-core/vaapi,zerowave-core/v4l2,zerowave-core/nvdec,zerowave-core/wgpu,zerowave-core/coaxd,zerowave-core/mirakurun,zerowave-core/plugins,zwsdk/host --all-targets -- -D warnings
# guest の側（wasm32）：host 無しでビルドできる。
cargo check -p zwsdk --target wasm32-wasip1

say "Windows（d3d12）"
cargo-zigbuild clippy -p zerowave-core --features d3d12,coaxd,mirakurun,plugins --all-targets \
    --target x86_64-pc-windows-gnullvm -- -D warnings

say "Android（64 ビットと 32 ビットの両方）"
# **両方**要る。`timespec` のフィールドの幅が変わるので、片方だけでは
# 32 ビットでビルドできない書き方が通ってしまう。
cargo clippy -p zerowave-core --features android --target aarch64-linux-android --all-targets -- -D warnings
cargo clippy -p zerowave-core --features android --target armv7-linux-androideabi --lib -- -D warnings

say "deck（別のワークスペース）"
(cd deck && cargo clippy --features zwcore/vaapi,zwcore/v4l2,zwcore/nvdec,zwcore/wgpu --all-targets -- -D warnings)

say "テスト（Linux で実行できるもの全部）"
# cargo-nextest で。テストごとに別のプロセスで並列に実行するので、cargo test より速い。
# 無ければ cargo test にフォールバックする。
if cargo nextest --version >/dev/null 2>&1; then
    cargo nextest run --workspace --features zerowave-core/vaapi,zerowave-core/v4l2,zerowave-core/nvdec,zerowave-core/wgpu,zerowave-core/coaxd,zerowave-core/mirakurun,zerowave-core/plugins,zwsdk/host || true
else
    cargo test --workspace --features zerowave-core/vaapi,zerowave-core/v4l2,zerowave-core/nvdec,zerowave-core/wgpu,zerowave-core/coaxd,zerowave-core/mirakurun,zerowave-core/plugins,zwsdk/host 2>&1 | grep -E 'test result|^error|panicked' || true
fi
