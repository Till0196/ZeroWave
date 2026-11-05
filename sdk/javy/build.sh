#!/bin/sh
# sdk/javy をビルドし、JavaScript のプラグインの実行環境 sdk/ts/zwjs.wasm を作る。
# 要るのは Rust の wasm32-wasip1 ターゲット（rustup target add wasm32-wasip1）。
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
cargo build --locked --release --target wasm32-wasip1 --manifest-path "$HERE/Cargo.toml"
cp "$HERE/target/wasm32-wasip1/release/zwjavy.wasm" "$HERE/../ts/zwjs.wasm"
