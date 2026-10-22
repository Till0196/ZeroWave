#!/bin/sh
# sdk/javy をビルドし、javy init-plugin を通して sdk/javy/zwjavy.wasm を作る。
# 要るのは Rust の wasm32-wasip1 ターゲット（rustup target add wasm32-wasip1）。
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
"$HERE/../fetch-javy.sh"
cargo build --release --target wasm32-wasip1 --manifest-path "$HERE/Cargo.toml"
"$HERE/../bin/javy" init-plugin "$HERE/target/wasm32-wasip1/release/zwjavy.wasm" -o "$HERE/zwjavy.wasm"
