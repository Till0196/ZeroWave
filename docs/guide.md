# 番組表とテーブル

放送が送ってくる番組の予定は `zwcore::guide` が保持します。
packet フィルターはテーブルを読まずに渡すだけで、読むのは `filters::tables` です。
保持するかどうかと、バージョンが上がったときだけ書き換える処理は `guide` が受け持ちます。

| テーブルの内容 | TS | MMT/TLV |
| --- | --- | --- |
| 番組（番組名・説明・ジャンル） | EIT | MH-EIT |
| サービスの名前 | SDT | MH-SDT |
| 現在時刻 | TOT | MH-TOT |
| ネットワーク・ストリーム・周波数・サービス一覧 | NIT | TLV-NIT |
| リモコンの番号 | NIT（ストリームごとに一つ） | TLV-NIT（サービスごとに一つ） |

MMT/TLV のリモコン番号は TLV-NIT にしかありません。
MMT-SI に NIT に当たるテーブルは無く、ARIB TR-B39 36.1.2.3 が「自ネットの TLV-NIT を出すときはリモコンキー記述子を入れる」と定めているのが根拠です。

TLV-SI は MMTP の外側を TLV パケットとして流れているので、MMT-SI とは入力を分けています（セクションに `Origin::TlvSi` を付け、`filters::tables` の `Tables::feed_tlv` で読みます）。
記述子のタグも文字の符号化も三通りあり、識別子では見分けられません。
TS は八単位符号、MMT-SI は二バイトのタグと UTF-8、TLV-SI は一バイトのタグと UTF-8 で、同じ 0xCD が TS では TS 情報記述子、TLV-NIT ではリモコンキー記述子を指します。

TLV の録画を一本通すと、各局の名前・種別・リモコン番号と、どのトランスポンダのどの周波数に載っているかまで揃います。
受信しているのは一本のストリームだけで、残りは TLV-NIT と MH-SDT から得た情報です。

## 放送波から集めて残す

`deck` は Mirakurun が無くても番組表を持ちます。

- 別のストリームの番組情報も入れます。
- ファイル（sqlite）に残して、次の起動で読み直します（`zwcore::guide::db`、feature `epg-db`）。
  場所はバイナリと同じフォルダーの `epg.sqlite` です（データ放送の不揮発メモリー `nvram.txt` も同じ場所、`deck::state`）。
  `ZW_STATE_DIR` があればそこに置きます。
  バイナリと同じフォルダーに書けないとき（`.app` の中、`/usr/bin`）は OS の決まった場所（Windows は `%LOCALAPPDATA%\zerowave\epg.sqlite`、macOS は `~/Library/Application Support/zerowave/epg.sqlite`、ほかは `$XDG_DATA_HOME/zerowave/epg.sqlite`、無ければ `~/.local/share/zerowave/epg.sqlite`）に置きます。
  五分ごとと終了時に書き、終わって七日たった番組はファイルから消します（長さの分からない番組は、開始から一日で終わったと見なします）。
  局ロゴは同じ場所の `logos/` に残します。
- 局の一覧は残しますが、表示するのは選べる局だけです。
  番組表ウィンドウ、パネルの番組表、チャンネル送りに出すのは、いまのソースで選べる局（`Receivable`）だけです。
  データ放送の不揮発メモリーも、事業者専用の領域（`nvram://~/`、`local/`、`local_web/`）はネットワークとストリームで分けます（zerowave_plugins の `databroadcast/bml` の `runtime::bindings::nvram`）。
