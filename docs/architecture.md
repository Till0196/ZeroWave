# 構成

## 解析とデコードのフィルター（`zwcore::filters`）

放送を解析・デコードする処理は一つの役割に一つのフィルターに分けています。
バイト列は上から順に通り、それぞれのフィルターが自分の分だけを読んで次へ渡します。
同じテーブルを二箇所で読まず、同じ記述子を二箇所で解析しません（記述子の解析は coax-rs の `isdb` に一つ。番組とストリームは `isdb::descriptors`、NIT は `isdb::nit` と `isdb::nitdescriptors`）。

```
  packet     TS（同期＋mpegts::Demuxer）／TLV（tlv::Feed＋MPU の組み立て＋時刻）
             → セクション（出どころ付き）・単位・字幕・ファイルの断片・時計
  analyzer   PAT/PMT、PA/MPT、EIT[p/f] の音声記述子 → Program（番組の成分）
  tables     SDT/NIT/EIT/TOT/BIT/CDT/SDTT/CAT/SIT と MH-*／TLV-NIT
             → guide（番組表・局名・時刻・ロゴ）と si（受信の情報）
  logos      全受信機共通データのカルーセル（SDTT と DSM-CC）→ 局ロゴ
  captions   字幕の PES/MFU → Out::Caption（見ている番組のぶんだけ）
  data       データ放送のセクションとファイル → Out::Data / Out::Item
  units      選んだ成分の単位 → AU の切り出し・音声のデコード → Out::Access / Sound
  selector   見る番組・映像・音声を決める（成分タグで追う、最良の番組を選ぶ）
  counter    受信の質（見ているサービスのぶんだけ）
```

フィルターをつなぐのは `zwcore::engine::Engine` で、順番は固定です。
ツールとテスト用に、packet と analyzer をまとめた `filters::Feed`（`Sink` に番組の成分と単位を渡す）があります。

## ストリームの種別

デマルチプレクサは、番組が持っているものを映像、音声、字幕、データ放送の四つに分けます。
字幕とデータ放送はデコードしませんが、種別の名前は付けます。

見分け方は伝送方式で違います。
MMT はアセットの四文字（`stpp` と `aapp`）で分かります。
TS はストリーム種別だけでは足りない（字幕も文字スーパーもデータ放送の PES も全部 0x06）ため、データ符号化方式記述子で見分けます。

## 規格の処理は一箇所に

H.264 の DPB（clause 8.2）、HEVC の参照画像集合（8.3）、MPEG-2 のヘッダー解析と並べ替えは `mediadecoder` にあります。
VA-API・NVDEC・D3D12・V4L2 の四つの経路は、その結果を自分の API の構造体へ移すだけです。
VideoToolbox は参照の追跡にだけ DPB を使います。
フレームを一枚開く処理（サーフェスを取り、番号の巻き戻りを見て、押し出されるフレームを出す）も DPB に一つです（H.264 は `Dpb::start`、HEVC は `Dpb::begin`）。
経路が渡すのは、取れたサーフェスの番号から、フレームと一緒に持ち回る物を作る処理だけです。

サーフェスのプールを規格の処理へつなぐ所も一つです。
復号画像バッファが要求するのは「サーフェスをください」「もう要らない」「これを表示」の三つで、それを経路の `Pool` とキューへつなぐのが `decode::frames` です。
サーフェスの識別子（スロット番号か、V4L2 のタイムスタンプか）だけが型引数になっています。

CABAC の算術デコーダーも一つです（`mediadecoder::video::cabac`）。
H.264 と HEVC で違うのはコンテキストの初期化だけで、範囲のテーブルも遷移も九ビットの初期値も同じです。

計測ツールが使う「録画を一本、アクセス単位に切る」処理も `zwcore::filters::cut` に一つで、五つの経路（VA-API・NVDEC・D3D12・V4L2・VideoToolbox）のテストと `deck --check` が同じ物を通ります。
