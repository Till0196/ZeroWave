# HDR（HLG）

HLG（ハイブリッドログガンマ）の映像は、原色が BT.2020 です（ARIB STD-B32 第1部 表 4-6：`colour_primaries` 9、`transfer_characteristics` 18、`matrix_coeffs` 9）。
この値を sRGB として表示すると、灰色がかって平たく、色の抜けた映像になります。

ストリームが指定した値は `mediadecoder::es::Format::colour` に入り（HEVC と H.264 は VUI、MPEG-2 は sequence display extension）、`zwcore::output::video::colour` がそれを `Matrix`（Y'CbCr の読み方）、`Primaries`、`Transfer` の三つの値に変換します。
ストリームの指定を優先します。
指定が無いとき、`Matrix` と `Primaries` は解像度で決め、`Transfer` は SDR とします。

映像をサーフェスへどう変換するかは `Mapping` が決め、処理は常に次の三つの手順です。

```
映像の伝達特性を外して cd/m² へ  →  原色が違えば変換  →  サーフェスの伝達特性を掛ける
  HLG: BT.2100 の EOTF          BT.2020 ↔ BT.709          sRGB / scRGB / PQ / HLG
  PQ:  ST 2084
  SDR: sRGB × SDR の白（下）
```

## SDR の白の明るさ

SDR の白の明るさはディスプレイの設定で決まります。
BT.2408 の 203 cd/m² は放送の中での SDR の白の基準で、隣に並ぶほかの SDR のウィンドウの白とは別の物です。
Windows は HDR のディスプレイで SDR のウィンドウを、利用者が「SDR コンテンツの明るさ」のスライダーで決めた明るさ（80〜480 cd/m²）で表示します。
`deck` はその値を取得して（`DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL`、`deck/src/draw/monitor.rs`）、UI と SDR の映像をその明るさで表示します。
ウィンドウを別のディスプレイへ動かしたときと、ウィンドウがアクティブに戻ったときに取得し直します。
ほかの OS では 203 cd/m² のままです。
PQ の映像はこの設定に関係なく、映像が指定する明るさで表示します。
HLG の映像は、信号の 75% がディスプレイの SDR の白になるように合わせます。
ディスプレイの最大輝度（DXGI の `MaxLuminance`）を超える HLG は、BT.2390 のニーで縮めます。
HDR で表示するのは映像だけで、メニューや OSD はほかのウィンドウと同じ明るさです。

## サーフェス

Windows では、ウィンドウのあるディスプレイで HDR が有効なときだけサーフェスを HDR で開きます（同じく `draw/monitor.rs`、`DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO`）。
ほかの OS ではディスプレイに問い合わせず、サーフェスが対応していれば HDR で開きます。
サーフェスとしては PQ を開けても、HDR のディスプレイから SDR のディスプレイへウィンドウを動かすと、PQ のサーフェスがそのまま合成されて白が潰れます。
そのため、動かした先が SDR ならサーフェスを SDR に戻して映像の方を変換し、HDR のディスプレイへ戻せばまた HDR で開きます。

HDR のサーフェスは、HLG（映像が HLG のときだけ。macOS 以外）、PQ、scRGB の順に、サーフェスが対応している物を選びます。

| サーフェス | 映像が HLG のとき |
| --- | --- |
| sRGB（従来のサーフェス） | 1000 cd/m² を 203 cd/m² へ BT.2390 のニーで圧縮する。拡散白は符号値 0.90 になる（サーフェスの白なのでディスプレイの設定は見ない） |
| BT.2100 HLG（Vulkan） | 変換しない。サーフェスがその映像の想定した見え方を扱う |
| BT.2100 PQ（Windows・多くの Vulkan） | 1000 cd/m² の OOTF を掛けて PQ に符号化する |
| scRGB（`Rgba16Float`。macOS はこれだけ） | cd/m² を 80（macOS は SDR の白）で割ってリニアのまま |

macOS で PQ と HLG のサーフェスを開かないのは、wgpu-hal 30.0.1 の Metal が BT.2100 の色空間の名前を取得する所でクラッシュするためです（`libloading` の `Symbol` をグローバル変数の中身ではなくアドレスとして読んでいて、`CGColorSpaceCreateWithName` が SIGBUS になる）。
EDR のサーフェスは scRGB で受けるので、映像は表示されます。

SDR の映像を SDR のサーフェスへ出す経路は変わっていません。
`Mapping::is_identity` のときは、シェーダーはこれまでどおり何も変換しません。
10 ビットの映像は、二つのパスの間のテクスチャ（`picture`）も 10 ビット（`Rgb10a2Unorm`）で持ちます。
8 ビットだと HLG の符号値が 4 段刻みになるためです。

同じ計算が `output/video/colour.rs`（Rust）、`output/video/shader/srgb.wgsl`・`shader/blit.wgsl`・`shader/over.wgsl`（WGSL）、`output/present/glsl.rs`（GLSL）にあります。
`core/tests/video.rs` が四種類のサーフェスで突き合わせます（lavapipe で最大の差は 1。feature `wgpu` が必要です）。

## 選び方

`deck` は `--hdr auto|off|on` でサーフェスを選びます。
`auto` は HLG か PQ の映像が来ているときだけ HDR のサーフェスで開き、SDR の局へ移ったら戻します。
`on` は映像が SDR でも HDR のサーフェスで開き、`off` は常に SDR で開きます。
zerowave.toml では `video.render.hdr` で指定します。
UI（egui）は sRGB しか扱わないので、常に中間テクスチャ（`gpu::Sheet`）へレンダリングして、映像と同じマッピングでサーフェスへ重ねます。
こうすると、サーフェスが SDR と HDR を行き来しても egui のレンダラーを作り直さずに済みます。
## GLES 2.0 の経路

GLES 2.0 の経路も、合成側へ色空間を伝えられる環境では HDR の画面に出します。
10 ビットのサーフェスに、BT.2020 の PQ か HLG（映像が HLG で、HLG を伝えられるとき）で符号化した値を書きます。

| 環境 | 伝え方 |
| --- | --- |
| Android | EGL のサーフェスの色空間（`EGL_EXT_gl_colorspace_bt2020_pq`・`_hlg`） |
| Linux の Wayland | `wp_color_management_v1`（Mesa の EGL はサーフェスの色空間に sRGB とリニアしか受け付けないため） |
| X11 など | 伝える方法が無いので SDR へ変換します |

コンテキストを設定に縛らずに作れる（`EGL_KHR_no_config_context`）ことと、10 ビットの EGLConfig があることが条件です。
HDR と SDR の切り替えはサーフェスだけを作り直し、コンテキストと GL の物は残します。
映像のトーンは `Mapping` と同じ式で、中間のテクスチャは使えれば半精度にします。
画面（egui）は一度テクスチャへ描き、下の映像を cd/m² へ戻して混ぜてから符号化し直します。
Wayland で映像をサブサーフェスに出しているときは、合成側が変換するので、ウィンドウの画面は SDR のままです。
キャプチャは SDR に戻して写します。
