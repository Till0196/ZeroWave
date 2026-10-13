# ビルド

## 通常のビルド

```sh
cd deck
cargo build --release              # target/release/ZeroWave（Windows は ZeroWave.exe）
cargo packager --release           # macOS の ZeroWave.app（cargo install cargo-packager）
```

| OS | 必要な物 | できる物 |
| --- | --- | --- |
| Windows | Visual Studio の C++ ビルドツール（MSVC） | MSVC の ZeroWave.exe |
| macOS | Xcode の Command Line Tools | その Mac の CPU 向け。動くのは macOS 13 以降 |
| Linux | C コンパイラー（開発用のパッケージは不要） | ビルドした環境の glibc 以降で動く物 |

coax-rs（`mpegts`・`mmttlv`・`isdb`・`coax`）は、Cargo が git（`https://github.com/Till0196/coax-rs`）から取ってきます。
手元の coax-rs を使うときは、zerowave と coax-rs を並べたフォルダーに `.cargo/config.toml` を置いて差し替えます。

```toml
[patch."https://github.com/Till0196/coax-rs"]
coax = { path = "coax-rs/coax" }
isdb = { path = "coax-rs/isdb" }
mmttlv = { path = "coax-rs/mmttlv" }
mpegts = { path = "coax-rs/mpegts" }
```

ライブラリだけをビルドするなら、ワークスペースの直下で下記を実行します。

```sh
cargo build --release -p zerowave-core --features wgpu        # レンダリングまで
cargo build --release -p zerowave-core --features vaapi,wgpu  # Linux のハードウェアデコード
```

## クロスビルド（zig）

クロスビルドでは zig でリンクします。
ビルドした環境より古い glibc 向け（配布用）も同じです。
Linux でも Windows でも同じ手順で、cargo の別名（`.cargo/config.toml`）と cargo のサブコマンドを使います。
必要な物は `zig`（PATH に置く）と `cargo install cargo-zigbuild cargo-packager` です。
Windows 向けは MinGW の ABI（`gnullvm`）で、CRT を静的にリンクします。

```sh
cd deck
cargo win                                   # Windows（ZeroWave.exe）
cargo win32                                 # 32 ビットの Windows
cargo win-arm64                             # arm64 の Windows
cargo linux                                 # Linux（glibc 2.28 以降）
cargo linux-old                             # Linux（glibc 2.17 以降）
cargo linux-arm64                           # Linux の arm64（glibc 2.28 以降）
cargo linux-armhf                           # Linux の armv7 hard-float（glibc 2.28 以降。Raspberry Pi 2 など）
cargo winq                                  # Windows（quick プロファイル。確かめるとき用）
```

ターゲットは `rustup target add` で入れます（`x86_64-pc-windows-gnullvm` など）。
macOS 向けは Mac で組みます。

## 配布物

バージョンの形のタグ（`v0.1.0` など）を push すると、`.github/workflows/release.yml` が組んで GitHub Releases に載せます。
macOS は Apple のランナーで組み、それ以外は ubuntu-latest から zig でクロスビルドします（上の `cargo linux`・`cargo linux-arm64`・`cargo linux-armhf`・`cargo win`・`cargo win32` と同じ組み方）。

| 成果物 | 中身 |
| --- | --- |
| `zerowave-<ターゲット>.tar.gz`（Windows は `.zip`） | ZeroWave と README・LICENSE・NOTICE（展開した場所でそのまま動きます） |
| `ZeroWave_<バージョン>_<amd64・arm64・armhf>.deb` | Linux の ZeroWave（`/usr/bin/ZeroWave`）と、メニューに並べる `zerowave.desktop`・アイコン |
| `zerowave-<バージョン>-1.<x86_64・aarch64・armv7hl>.rpm` | deb と同じ物（ライセンスは `/usr/share/licenses/zerowave`） |
| `ZeroWave-<ターゲット>.app.zip` | macOS の ZeroWave.app（ad-hoc の署名） |

ターゲットは macOS（Apple シリコン・Intel）、Linux（x86_64・arm64・armv7 hard-float、glibc 2.28 以降）、Windows（x86_64・i686・arm64、gnullvm）です。
.app と deb は cargo-packager で作ります（設定は deck の `Cargo.toml` の `[package.metadata.packager]`）。
rpm は rpmbuild で作ります（設定は `deck/linux/zerowave.spec`。組んだバイナリを包むだけで、rpm の中ではコンパイルしません）。
rpm の版はタグの `-` を `~` に替えた物です（`v0.2.0-beta.1` なら `0.2.0~beta.1`）。
deb と rpm の .desktop とアイコンは `deck/linux/` の物をそのまま入れます（.desktop の名前を Wayland の app_id の `zerowave` に揃えるため）。
タグに `-` が付く物はプレリリースにします。
zig（ziglang.org の公式のアーカイブを SHA-256 で確かめて使う）・cargo-zigbuild・cargo-packager の版は、release.yml の `env` で決めています。
GitHub の action は actions/ の公式の物（checkout・upload-artifact・download-artifact）だけを使います。

### macOS で初めて開くとき

ZeroWave.app は公証していないので、ダウンロードした物を初めて開くと macOS に止められます。
次の手順で一度だけ許可します。

1. ZeroWave.app を開きます。
   「“ZeroWave.app”は開いていません」と出るので、「完了」を押します（「ゴミ箱に入れる」は押しません）。
2. 「システム設定」の「プライバシーとセキュリティ」を開き、下の方の「セキュリティ」まで進みます。
   「お使いのMacを保護するために“ZeroWave.app”がブロックされました。」の横の「このまま開く」を押します。
3. 「“ZeroWave.app”を開きますか?」と出るので、「このまま開く」を押します。
4. 管理者の Touch ID かパスワードで許可します。

次からは普通に開けます。
ターミナルからなら、`xattr -dr com.apple.quarantine ZeroWave.app` でも開けるようになります。

## feature

ハードウェアの経路は feature になっていて、その環境に無い経路はビルドに含めません。
ドライバーが入っていない環境では、経路は「無い」を返してソフトウェアデコーダーに切り替えます。
libva も libnvcuvid も `dlopen` で開き、バインディングは自前で書いているので、依存するクレートはほとんど増えません。

deck の既定の feature は sound・nvdec・qsv・amf です。
Linux では vaapi・v4l2・passthrough・wayland、Apple では videotoolbox が自動で入ります。
Windows でパススルーを使うときは `--features passthrough` を付けます。
