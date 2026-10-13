# 検証

試験の素材は、試験の前に `sh testdata/make.sh` で `testdata/` に作ります（git には入れていません）。
どれも ffmpeg で生成した映像と音声をエンコードした小さなファイル（H.264 のベースラインとハイ、HEVC の Main 10、MPEG-2 のインターレースとテレシネ、AAC）です。
ほかに、番組が二つの TS、音声が三本の TS、テレシネの突き合わせに使う輝度だけの Y4M を作ります。
ffmpeg は libx264・libx265・mpeg2video・aac 入りの物が要ります。
放送の録画は要りません（デバイスが必要な試験は、デバイスが無ければスキップします）。

デコーダーは参照実装と突き合わせています。

```
MPEG-2   IEEE 1180 を 4 桁の余裕で通過。実放送で ffmpeg に対し 65〜67 dB
H.264    実放送で ffmpeg に対しビット単位で一致
HEVC     実放送で ffmpeg に対しビット単位で一致
AAC      実放送で ffmpeg に対し 133 dB。22.2ch の 24 チャンネルすべて
レンダリング GPU の無い環境でも動くよう、WGSL と同じ処理を Rust でも書いて突き合わせ
```

ハードウェアの経路は `deck --check` で確かめます。
同じ録画をデバイスと内蔵のソフトウェアデコーダーで二回デコードし、フレームごとの dB と並び順と速さを出します。
実機でしか意味の無い検証なので、`deck` に入れています。

VA-API・V4L2・NVDEC・D3D12・Android の経路は、実機が無くてもまとめて検査できます。

```sh
./tools/check-all.sh
```

対象は書式、Linux（vaapi・v4l2・nvdec・wgpu・coaxd・mirakurun・plugins と、wasm32 の zwsdk）、Windows（d3d12）、Android（64 ビットと 32 ビット）、`deck`、テストです。
`cargo check` ではなく `cargo clippy -- -D warnings` で検査します。
リンクはしないので実機は要りませんが、引数と型の不一致はここで分かります。
AMF・QSV・Media Foundation・パススルーの feature は検査しません。
Windows の行には zig と cargo-zigbuild が要ります。
Apple の行は検査しないので、CI の macOS のランナーで確かめます。
テストが失敗してもスクリプトは失敗しないので、出力を見て確かめてください。

GitHub Actions の CI（`.github/workflows/ci.yml`）も、main への push とプルリクエストで同じ検査を流します。
CI ではテストの失敗も失敗として扱います。
Apple の行は macOS のランナーでそのまま、Windows の行は ubuntu-latest から zig で検査します。

テストは [cargo-nextest](https://nexte.st) で実行します。
テストごとに別のプロセスで並列に実行します（`cargo test` は一つのバイナリの中で直列に実行するので、テストが増えるほど遅くなります）。
設定は `.config/nextest.toml` です。

```sh
cargo nextest run --workspace --features zwsdk/host,zerowave-core/plugins   # デバイスを使わないテスト
cargo nextest run -p mediadecoder                                            # 一つのクレート
cargo nextest run caption                                                    # 名前で絞る
cargo test --doc                                                             # doctest は nextest では実行されない
```

GPU やデバイスを使うテストは、その feature を付けたときだけ動きます（`zerowave-core/wgpu`、`vaapi`、`v4l2`、`nvdec`、`coaxd` など）。
`vaapi`・`nvdec` のテストと mediadecoder の H.264・HEVC のテストには、`h264-decoder`・`hevc-decoder` も必要です。
`deck` は別のワークスペースなので、`cd deck` してから実行します。

cargo-nextest は `curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ~/.cargo/bin` で入ります（macOS は `latest/mac`）。
