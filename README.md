# ZeroWave

デジタル放送の視聴プログラム。MPEG-2 TS と MMT/TLV のdemuxからデコード、デインタレース、表示までを行います。

| クレート | 中身 |
| --- | --- |
| `mediadecoder` | ES ヘッダー解析、MPEG-2 / H.264 / HEVC / AAC / MPEG Audio / AIFF のデコード、規格が定める参照画像の管理。 |
| `core`（`zwcore`） | 本体。コンテナフォーマット、時計と同期、八つのハードウェアデコードの経路（VA-API・V4L2・NVDEC・D3D12・VideoToolbox・Android・AMF・QSV）、音声の経路（デバイス、パススルー、リサンプル）、wgpu と GLES 2.0 のレンダリング、デインタレース、番組表と、プレイヤー（字幕、データ放送、レイヤー、プラグインの読み込み。feature `plugins`）。 |
| `sdk`（`zwsdk`） | プラグインの SDK。プラグイン側（`plug`）、ホスト側（`host`、feature）、メッセージの型（`layer`・`video`）、フォントの探し方（`fonts`）、ホスト側の canvas 2D（`draw`、feature `host`）、通信（`net`）、UI（`ui`）、リモコン（`control`）、設定（`config`）。`javy/` と `ts/` は TypeScript用の Javy プラグインと SDK |
| `deck` | デスクトップ向けZeroWave実装。ウィンドウ（winit・egui）、wgpu のサーフェス、音声の出力（Linux は libasound を実行時に開き、ほかは cpal）。 |

伝送層（`mpegts` / `mmttlv` / `isdb`）は [coax-rs](https://github.com/Till0196/coax-rs) 。
プラグイン（TSMF、データ放送、映像フィルター、TypeScript のレイヤー）は[zerowave_plugins](https://github.com/Till0196/zerowave_plugins) 。

`core` の package 名は `zerowave-core`、使うときの名前は `zwcore`（`core` は libcore と衝突する）。

### 使い方

```sh
cd deck
cargo run --release -- 録画.ts        # 再生
cargo run --release                   # zerowave.toml のソース（coaxd・Mirakurun）で起動する
```

`coaxd` は [coax](https://github.com/Till0196/coax) のチューナーを管理するデーモンです。
coax のリリースから入手して、`ZeroWave` と同じフォルダーか `PATH` の通った場所に置くと、必要なときに自動で起動します。
`ZeroWave` のフォルダーに書き込めないときは、設定フォルダー（Windows は `%LOCALAPPDATA%\zerowave`、macOS は `~/Library/Application Support/zerowave`、Linux は `~/.local/share/zerowave`）に置きます。
ファームウェアが必要なデバイスは、`coaxd` と同じフォルダに `firmware` フォルダーを作成し、ファームウェアを設置してください（[coax の説明](https://github.com/Till0196/coax/blob/main/docs/setup/firmware.md)）。

クロスビルドは[ビルド](docs/building.md)を参照してください。
macOS でダウンロードした ZeroWave.app を初めて開くときの手順は、[macOS で初めて開くとき](docs/building.md#macos-で初めて開くとき)にあります。

### 文書

| 文書 | 中身 |
| --- | --- |
| [スクリーンショット](docs/screenshots.md) | 画面の例 |
| [構成](docs/architecture.md) | 解析とデコードのフィルター、ストリームの種別、規格の処理を一箇所に置く方針 |
| [番組表とテーブル](docs/guide.md) | TS・MMT/TLV のテーブル、放送波からの番組表の集め方と残し方 |
| [プラグイン](docs/plugins.md) | wasm のプラグインの SDK と hook、TSMF |
| [デコーダー](docs/decoders.md) | OS のデコーダー、デコーダーの優先順位と GPU 間転送、デコーダーのデインタレース、音声のデコーダー |
| [HDR](docs/hdr.md) | HLG・PQ の映像をサーフェスへ出す変換 |
| [AAC のパススルー](docs/passthrough.md) | IEC 61937 で AV アンプへ渡す経路 |
| [検証](docs/testing.md) | 試験の素材、参照実装との突き合わせ、`deck --check` |
| [ビルド](docs/building.md) | 通常のビルド、zig でのクロスビルド、feature |

### 注意

ZeroWave には CAS（限定受信方式）の処理は含まれていません。
暗号化された放送は再生できません。

### ライセンス

ZeroWave は Apache License 2.0 です（[LICENSE](LICENSE)）。

ほかのソフトウェアのヘッダーから書き写した宣言と、同梱している `sdk/javy/zwjavy.wasm` のライセンスは [NOTICE](NOTICE) にあります。

| 書き写した物 | ライセンス |
| --- | --- |
| NVIDIA Optical Flow SDK・Video Codec SDK（nv-codec-headers）のヘッダー | MIT |
| Intel の libvpl・libva のヘッダー | MIT |
| AMD Advanced Media Framework のヘッダー | MIT |
| Linux の V4L2・DRM のヘッダー | GPL-2.0 WITH Linux-syscall-note（一部は BSD-3-Clause・MIT） |
| `sdk/javy/zwjavy.wasm`（Javy・rquickjs・QuickJS-ng ほか） | Apache-2.0 WITH LLVM-exception・MIT |

