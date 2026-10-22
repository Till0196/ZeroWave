# プラグイン（wasm）

本体に手を入れずに機能を足すときは、wasm のプラグインにします。
プラグインは判断だけを持ち、重い処理（構文解析・I/O）は本体の SDK が native で行います。
ランタイムは wasmtime（JIT）で、SDK は `zwsdk` です。

- `zwsdk::plug`：プラグイン側。
  `Stream` を実装して `zwsdk::export_stream!` で公開します。
  native にビルドすればそのままテストできます（`plug::abi::native`）。
  `plug::host` は本体が提供する汎用の API（時計・設定・フォント・通信・プロセス・ソケット・ファイル・チューナー・リモコン・通知）です。
  通信は、待って応答を全部受け取る `net::request` と、待たない `net::Conn` があります。
  `net::Conn` は HTTP の本文を届いた分ずつ、WebSocket（`ws://`・`wss://`）はメッセージごとに `poll` で受け取ります（一つの接続に本体のスレッドが一つ）。
  どちらも、プラグインが設定 `net` を宣言して有効なときだけ繋がり、内部ネットワークへは `net_local` が有効なときだけです。
  ソケット（`zw_sock_v1`）の接続先は `kind = "socket"` のキーにユーザーが入れた値で、ドメインソケットのパス、`host:port`（TCP）、Windows の名前付きパイプ（`\\.\pipe\…`）のどれかです。
  値の `$名前`・`${名前}` は本体の環境変数で展開するので、`$XDG_RUNTIME_DIR/…` や macOS の `$TMPDIR/…` のようにも書けます。
- `zwsdk::host`（feature `host`）：ホスト側。
  マニフェスト（`zw_manifest` か `manifest.json`）を読んで hook ごとにインスタンスを作ります。
  ビルドした物は `.cwasm` として、プラグインフォルダーの隣の `plugins_cache/` に残します。
  そこに書けなければ OS のキャッシュの場所（Windows は `%LOCALAPPDATA%\zerowave\plugins_cache`、macOS は `~/Library/Caches/zerowave/plugins_cache`、ほかは `$XDG_CACHE_HOME`（無ければ `~/.cache`）の `zerowave/plugins_cache`）に残します。
  使われなくなった物は読み込むたびに消します。`plugins_cache/` は消しても構いません。
- `zwsdk::abi`：import の名前空間（`zw_buf_v1`、`zw_output_v1`、`zw_log_v1`、`zw_draw_v1`、`zw_layer_v1`、`zw_clock_v1`、`zw_cfg_v1`、`zw_font_v1`、`zw_net_v1`、`zw_proc_v1`、`zw_tuner_v1`、`zw_sock_v1`、`zw_files_v1`、`zw_ui_v1`、`zw_control_v1`、`zw_video_v1`）と export の名前。
  名前の文字列はここにだけ書きます。
  guest の `#[link]` はリテラルしか取れないので、`sdk/tests/abi.rs` で同じ文字列かを確かめます。
- `zwsdk::layer`：レイヤーのプラグインと本体の間のメッセージ（JSON）。
  どのレイヤーか（`Place`）、何が欲しいか（`Want`）、`Message`／`Event`／`Frame`。
- `zwsdk::video`：映像フィルターのメッセージの型。
  プラグインが返すパスの構成（`Graph`：WGSL のパスの並びと中間のテクスチャ・バッファ）、バインドの仕方、本体が毎フレーム書く `Zw` です。
  NN のように何百も dispatch を積む物は、compute のパスにワークグループの数（`groups`）とバッファ（`buffers`、`@group(2)`）を指定し、重みのような大きなデータは `Graph::blobs`（`zw_video_v1` で JSON とは別に渡す）で渡します。
  続く `groups` のパスは、本体が一つの compute パスにまとめます。
  GPU の能力（f16・協調行列・上限）は `Env::device` にあります。
- `zwsdk::ui`：メニュー・ページ・通知のデータ型。
- `zwsdk::control`：リモコン API のデータ型。
- `zwsdk::config`：一覧とマップの設定（`kind = "list"`・`"map"`）。
- `zwsdk::fonts`：フォントの探し方です。
- `zwsdk::draw`：canvas 2D の API でレンダリングして画像にします（ホスト側だけ、feature `host`）。
- `zwsdk::net`：データ放送の通信など。本体がプラグインの代わりに通信します。
- `sdk/ts`・`sdk/javy`：TypeScript でレイヤーを書くための SDK と、それを wasm にする Javy のプラグイン。
  canvas 2D・タイマーのほか、`fetch`・`WebSocket`（`net::Conn` の上に作ったもの）と設定（`cfg`）が使えます。
  設定は `package.json` の `zerowave.config` に書くと、ビルドのときにマニフェストに入ります。
- `sdk/examples/layer.rs`：レイヤーのプラグインを本体なしで動かし、最後の画面を PNG に書きます
  （`cargo run -p zwsdk --features host --example layer -- <プラグイン> <フレーム数> out.png --cfg net=true --clock live`）。

プラグインは [zerowave_plugins](https://github.com/Till0196/zerowave_plugins) にあります。

| プラグイン | 中身 |
| --- | --- |
| `tsmf` | TSMF を分離する stream |
| `databroadcast` | データ放送（BML・HTML5・Hybridcast）のレイヤー |
| `pipe` | 受けたストリームを、利用者が選んだプログラムの標準入出力に通す stream |
| `video-filter` | mpv のユーザーシェーダーと WGSL を掛ける映像のフィルター、デインタレース、フレーム補間 |
| `clock-comments` | TypeScript で書いた、字幕の上のレイヤー |

ビルドの仕方は zerowave_plugins の README にあります。

本体は `zwcore::plugins::Loaded::load_except` で、設定で無効にした物を除き、プラグインフォルダーの `*.wasm` と、`manifest.json` を持つディレクトリを全部ビルドします。
`deck` のプラグインフォルダーは、`ZW_PLUGINS`、設定の `[data] plugins`、設定と同じ場所の `plugins/`、バイナリと同じフォルダーの `plugins/` の順に決まります。
stream のプラグインはマニフェストの `order` の小さい順に通し、同じなら名前の順にします。
`*.wasm` 一つのときのマニフェストは、カスタムセクション `zw_manifest`（TypeScript のプラグインはビルド時に埋め込みます）か、export の `zw_manifest`（`zwsdk::plug`）です。

## hook

- `stream`：ソースから読んだバイト列を加工します。
  読み取りスレッドで動き、選局のたびに新しいインスタンスを作ります。
- `layer`：画面のレイヤーの一つになります。
  画面は映像 → データ放送 → 字幕 → OSD の順に重なっていて、プラグインはマニフェストの `layer` で位置を指定します。
  `{"at":"databroadcast"}` なら本体のそのレイヤーを置き換え（字幕や OSD も置き換えられ、そのとき本体はそのレイヤーを表示しません）、`{"above":"captions"}` や `{"below":"osd"}` ならその間に重なります。
  `wants` で欲しい入力（streams・captions・keys・clock・area・broadcast・popup）を指定すると、本体はそれだけを `zwsdk::layer::Message` で送ります（指定が無ければ clock・area・keys）。
  エントリポイントはどのレイヤーでも同じ六つです（new／message／tick／frame／events／drop。Rust は `zwsdk::plug::Layer` と `zwsdk::export_layer!`、TypeScript は `onNew`／`onMessage`／`onFrame`／`send`）。
  画像は RGBA で返しても、本体の canvas 2D（`zw_draw_v1`）にレンダリングしてそのことを返しても構いません。
  データ放送は `at: databroadcast` のレイヤー（文書の大きさ・映像の位置・別ウィンドウはこのレイヤーだけが指定できます）。
  プラグインが無ければ、そのレイヤーが表示されないだけです。
- `ui`：メニュー・ページ・通知を足します。
  プラグインはウィジェットの並びを返し、レンダリングは本体が行います（`zwsdk::plug::Ui` と `zwsdk::export_ui!`）。
- マニフェストの `hooks` に `control` を書くとリモコン API（`zw_control_v1`）を、`tuners` を書くとチューナーの貸し出しを使えます。
  リモコン API では、選局やキー操作のほか、いま見ている局と番組（`now`：サービス名・事業者名・ロゴ ID、現在と次の番組の名前・説明・拡張説明・ジャンル・時刻）、局ロゴの PNG（`logo`、deck はいま見ている局の分だけ）、本体の名前とバージョン（`host`）を、どの hook からでも取りに行けます。
  放送ソースが無く録画だけを見ているときも、ストリームの SDT・EIT から分かる分は入ります。
- `video`：映像のフィルターです。
  プラグインはローダーで、パスの構成（`zwsdk::video::Graph`：WGSL のパスの並び、中間のテクスチャ、一つの入力から何枚出すか）を返すだけです。
  画素を処理するのは本体の GPU です（`zwcore::output::video::hook`）。
  呼ばれるのはビルドし直すとき（読み込んだとき、映像の縦横比や画面の大きさが変わったとき、設定や読めるフォルダーの中身が変わったとき）です。
  `"program": true` なら番組が変わるたびに `key` を呼び、値が変わったときもビルドし直します。
  位置は `{"at":["picture"]}`（デインタレース後の RGB。名前の順に連結します。シャープ化・拡大・フレーム補間など。省略時はこれ）と `{"at":["deinterlace"]}`（本体のデインタレースの代わり。一つだけで、前後二フレームまで読めます）で、両方に置くこともできます。
  ビルドできない物は外して理由をログに出します。
  毎フレームの処理時間を測り、合計が上限を超えたら一番重い物を一つ止め、フレーム間隔が延びたら止めた物を測り直します（`keep` の物は止めません）。
  パスに外部のプロセスを指定すると、本体がそのプロセスを起動してフレームを渡します（`zwsdk::video::Process`、手順は `sdk/src/video/process.md`）。
  wgpu の経路だけで、GLES 2.0 の経路では使えません。

## 多重フレーム（TSMF）

TSMF の分離は本体には入っておらず、[zerowave_plugins](https://github.com/Till0196/zerowave_plugins) の `tsmf`（`zerowave_tsmf.wasm`）が stream hook として行います。
