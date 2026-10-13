#!/bin/sh
# javy（JS → wasm）を tools/bin/javy に置く。あれば何もしない。
# バージョンを変えるときは VERSION と下のチェックサムを（リリースの .gz.sha256 の値）。
# JAVY_VERSION で別のバージョンを使うなら、その .gz の SHA-256 を JAVY_SHA256 で渡す。
set -e
VERSION="${JAVY_VERSION:-v9.1.0}"
DIR="$(cd "$(dirname "$0")" && pwd)/bin"
BIN="$DIR/javy"
if [ -x "$BIN" ]; then
  exit 0
fi
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) TARGET=x86_64-linux; SUM=a68b122d48eb3dfc1b801d4e14c39271fde3638243d3272d206e376ac9189e39 ;;
  Linux-aarch64) TARGET=arm-linux; SUM=826962f0e82354cf97d6e928ea36777872bbae76c626e326948e0367c4b55cb5 ;;
  Darwin-arm64) TARGET=arm-macos; SUM=99e9ec6a8e8c98e119d137c08a921d2443d3b873c675a5571e1800f4451e6294 ;;
  Darwin-x86_64) TARGET=x86_64-macos; SUM=6eed2927575dc2b3fb5a1563eee1ce0874e6da91c9cec39a728c9377a8cc7b5a ;;
  MINGW*|MSYS*|CYGWIN*) TARGET=x86_64-windows; SUM=7148baab85d7426e8c18e2cc4deed4e9f03270cf3245dba5617a5be25a1de836 ;;
  *) echo "javy: このマシン向けのビルドが分からない: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
URL="https://github.com/bytecodealliance/javy/releases/download/$VERSION/javy-$TARGET-$VERSION.gz"
mkdir -p "$DIR"
echo "javy $VERSION をダウンロードする: $URL" >&2
if [ -n "${JAVY_SHA256:-}" ]; then
  SUM="$JAVY_SHA256"
elif [ "$VERSION" != v9.1.0 ]; then
  echo "javy: $VERSION のチェックサムを JAVY_SHA256 で渡す" >&2
  exit 1
fi
curl -fsSL "$URL" -o "$BIN.gz"
if command -v sha256sum >/dev/null 2>&1; then
  GOT=$(sha256sum "$BIN.gz" | cut -d' ' -f1)
else
  GOT=$(shasum -a 256 "$BIN.gz" | cut -d' ' -f1)
fi
if [ "$GOT" != "$SUM" ]; then
  rm -f "$BIN.gz"
  echo "javy: チェックサムが合わない（$GOT、期待 $SUM）" >&2
  exit 1
fi
gunzip -c "$BIN.gz" > "$BIN"
rm -f "$BIN.gz"
chmod +x "$BIN"
