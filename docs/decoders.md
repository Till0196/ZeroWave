# デコーダー

MPEG-2 のデコーダー、AAC と MPEG Audio のデコーダー、ハードウェアの経路が使うパラメーターセットの解析と参照画像の管理はリリースビルドに含まれます。
H.264 と HEVC の内蔵デコーダーは、特許の使用許諾が要るので feature `h264-decoder`・`hevc-decoder` を付けたときだけ入ります。
ハードウェアをソフトウェアと突き合わせるテスト（`d3d12`・`vaapi`・`nvdec`・`videotoolbox`）には、この二つの feature も必要です。

## OS のソフトウェアデコーダー

`--decoder system`（設定では `video.decoder.default = "system"`）で、OS のソフトウェアデコーダーを使います。
既定の順（ハードウェアのデコーダー、無ければ内蔵）では、内蔵のデコーダーがこのビルドに無いコーデックや、設定で外したコーデックのときだけ、最後に OS のソフトウェアデコーダーを使います。
内蔵のデコーダーを疑うときの比較相手として、`deck <録画> --dump <出力> --decoder system` で映像を書き出して比べることもできます。

| OS | 経路 | 開くデコーダー |
| --- | --- | --- |
| Windows | Media Foundation（feature `mediafoundation`） | 同期の MFT だけ。MPEG-2・H.264 は OS 同梱、HEVC は HEVC Video Extensions がある環境だけ |
| macOS | VideoToolbox | デコーダーの指定で `EnableHardwareAcceleratedVideoDecoder` を false にする |
| Android | MediaCodec | `c2.android.*`、無ければ `OMX.google.*` を名前で |

再生では、OS のデコーダーが無ければ内蔵のデコーダーに切り替え、その理由をログに一行出します。
`--dump` では切り替えず、開けなかった理由を出して終わります。
ストリームの情報のモードは「ソフトウェア」になります。

## デコーダーの優先順位と GPU 間転送

コーデック（MPEG-2・H.264・H.265）ごとに、使うデコーダーを上から並べられます（設定ウィンドウの「メディアコーデック」、`[video.decoder] mpeg2 = [...]`）。
一行が「デバイスと方式」（`zwcore::decode::Pick`）で、上から開き、開けないか一枚も出さずに拒否されたら次へ進みます。
並びがどれも使えなければ、このビルドの既定のハードウェアデコーダーを試し、最後に ZeroWave を使います。
空のコーデックは、このビルドの既定の順（ハードウェアのデコーダー、ZeroWave）になります。
行の先頭に `!` を付けると、その行は使いません（設定ウィンドウで外した物がこの形で残ります）。
`"!zerowave"` を書くと、そのコーデックでは ZeroWave を使いません。

```toml
[video.decoder]
h264 = ["d3d12:AMD Radeon(TM) Graphics", "!system:Microsoft H264 Video Decoder MFT"]
hevc = ["nvdec:NVIDIA GeForce RTX 4070"]
```

- GPU は名前で、方式は行ごとに選びます（Windows は自動・D3D12・D3D11VA、NVIDIA は NVDEC、AMD は AMF、Intel は QSV も）。
  使えるデコーダーは `deck --decoders` で同じ一覧が出ます。
  対応のテーブルを問い合わせるだけで、映像のキューは作りません。
- レンダリングに使っていない GPU も選べます（GPU 間転送）。
  その GPU でデコードし、フレームをメモリーへ読み戻してレンダリングの GPU へ渡します（Windows は `decode/d3d12.rs`、Linux は `decode/vaapi.rs`）。
  いまは一枚ずつ読み戻しを待つ形です。
  AMF と QSV は、どの GPU でもフレームをメモリーで渡します。
  NVDEC は、Windows ではレンダリングの GPU でだけ使えます。
  Linux の NVDEC は CUDA の最初のデバイスを使います。
- `--decoder software` と `--decoder system` は明示の指定なので、並びより優先します。
- Intel の QSV は Windows と Linux で使えます（oneVPL のランタイムか、第 6〜10 世代は旧 Media SDK の `libmfx1`）。
  32 ビットのプロセスは 32 ビットのランタイム（`libmfx32-gen.dll` など）を読みます。
  Linux は VA-API と QSV の GPU を名前で選べます。
- 自動では、D3D12 が対応しないコーデックを D3D11VA で開きます（AMD の MPEG-2 など。Linux は VA-API）。
  AMD の MPEG-2 は AMF でも開けますが、フィールドピクチャーの MPEG-2 は AMF ではデコードしません。
  RDNA2 以降の GPU は MPEG-2 のハードウェアデコーダーを持たないので、ハードウェアでは扱えません。
- デコーダーにデインタレースもさせられます（行の末尾に `:adaptive`・`:bob`・`:motion`。デバイス名を省くときは `nvdec::bob` のように書きます）。
  使える経路は NVDEC・VA-API・QSV・AMF・V4L2 です。
  `:motion`（動き補償）は VA-API と AMF だけ、V4L2 は `:adaptive` だけです。
  AMF は D3D11 のビデオプロセッサーで、V4L2 は SoC のデインターレーサーでデインタレースします。
  一フレームからフィールドレートで二枚出し、二枚目には一フィールド分後の時刻を付けます。
  このときは ZeroWave のデインタレースと逆テレシネは掛かりません（フィールドがもう無いため）。
  ドライバーの実装では品質を確かめられないので、既定は「しない」です。
- Linux の NVDEC は、DMA-BUF を書き出せないデバイスでは、フレームをメモリーへ読み戻して渡します。

音声のデコーダーも同じ形で並べられます（`[audio.decoder] aac = [...]`・`mpeg = [...]`）。
並ぶのは OS のデコーダー（Windows は Media Foundation の MFT、macOS は AudioToolbox、Android は MediaCodec）と ZeroWave で、空なら ZeroWave だけです（OS のデコーダーは選んだときだけ使います）。
Linux には OS のデコーダーがありません。

```toml
[audio.decoder]
aac = ["system:Microsoft AAC Audio Decoder MFT"]
```

- チャンネル構成 0 で PCE を使う AAC（22.2ch など）が来たら OS のデコーダーをやめ、以後は ZeroWave でデコードします。
  途中で OS のデコーダーがエラーを返したときも、その場で ZeroWave に替えて同じフレームをデコードし直します。
- 二か国語の AAC（チャンネル構成 0 で PCE を持たず、SCE が二つ）は、前に SCE を二つ置く PCE を設定に入れて OS のデコーダーに渡します。
  出てきた二つのチャンネルは、ZeroWave と同じく主・副として扱います。
- MPEG Audio のデュアルチャンネルも OS のデコーダーに渡します。
  Windows の MFT は、指定しなければ主を両方に出すので、ステレオのまま出すよう設定します。
- macOS は、24 kHz 以下の AAC（ワンセグの HE-AAC）を、HE-AAC と明示した設定にして渡します。
  LC の設定のままでは、AudioToolbox は 24 kHz のまま出します。
- Android は、MPEG Audio のデュアルチャンネルを OS のデコーダーに渡さず、ZeroWave でデコードします。
  AOSP には MPEG Audio Layer II のデコーダーが無く、機種ごとのデコーダーがどう出すかが決まらないためです。
- Windows と Android は、出てくるチャンネルの並びを ZeroWave の並びに直して渡します。
  macOS は、チャンネル構成どおりの並びで出てくるものとして扱います。
  放送が指定するダウンミックスの係数は付かないので、5.1ch を 2ch にするときは規格の既定の係数を使います。
- `deck --sound-check <録画>` で、同じ録画の音声を ZeroWave と OS のデコーダーでデコードして突き合わせます（`--frames` は集める回数）。

## 経路の選び方とログ

どの経路でも共通のオプションで選びます。

```sh
deck --decoder hardware --deinterlace bwdif --deinterlace-by hardware --render wgpu
#     auto|hardware|software|system    auto|off|weave|bob|yadif|bwdif
#                               auto|software|hardware        auto|wgpu|gles
```

Windows では `--decoder-api auto|d3d12|d3d11`（設定では `video.decoder.api`）で API を固定できます。

デバイスが返した内容を見たいときは `ZW_TRACE`（`ZEROWAVE_TRACE` でも可）を使います（`1`・`all`、または `v4l2,nvdec` のように経路を選ぶ）。
既定では出しません。
