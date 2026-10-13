# AAC のパススルー

`--passthrough` を付けると、音声をデコードせず、エンコードされたまま AV アンプへ渡します。
5.1ch は 5.1ch のまま、AV アンプのデコーダーとスピーカー配置で再生されます。

```sh
deck --passthrough 録画.ts
deck --passthrough --sound-device "Denon" 録画.ts
```

IEC 61937 のパケット化は ZeroWave で行い、どの経路にも IEC 61937 でパケット化したバイト列を渡します。
四つの経路の違いは、同じバイト列の渡し方だけです。
macOS にはパススルーの経路がありません。

| 経路 | feature | 環境 | 開き方 |
| --- | --- | --- | --- |
| WASAPI | `passthrough` | Windows | 排他モード + `KSDATAFORMAT_SUBTYPE_IEC61937_AAC` |
| PipeWire | `passthrough` | Linux | `SPA_MEDIA_SUBTYPE_iec958` + `MPEG2-AAC`。POD は手で組み立てる |
| ALSA | `passthrough` | Linux | `iec958:` か `hdmi:`（どちらも `AES0=0x6,...`）を直接開く |
| AudioTrack | `passthrough` | Android | `ENCODING_IEC61937` の直接出力のトラック。JNI |

Linux の deck には常に入ります。
Windows の deck は `cargo build --release --features passthrough` でビルドします。
Linux は PipeWire を先に試し、開けなければ ALSA を使います。
ALSA では `--sound-device` の名前をそのままデバイス名として使うので、`AES0=0x6` を付けて指定します。

zerowave.toml の `audio.passthrough` は `off`・`on`・`auto` です。
`auto` ではマルチチャンネルの音声だけをパススルーし、ステレオなどはデコードします。

パススルーの出力を開けないときは、その旨を知らせてデコードした音声で再生します。
22.2ch は IEC 61937 で送れないので、パススルー中は音声を止めて知らせます。
MPEG Audio はパススルーできず、`--passthrough` のときは音が出ません。
`--no-sound` と同時に指定すると、パススルーは無効になります。

パススルー中はリサンプラーを使えないので、ドリフト補正の単位が 1 サンプルから 1 フレーム（1024 サンプル、21.33 ms）になります。
`--dual-mono` も効きません（並べ替える対象が無いため）。
