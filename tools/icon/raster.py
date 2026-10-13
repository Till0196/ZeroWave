#!/usr/bin/env python3
# Copyright 2026 Till0196
# SPDX-License-Identifier: Apache-2.0
"""deck/icon/ の SVG から、OS ごとのアイコンを書き出す。

    python3 -m venv venv && venv/bin/pip install resvg-py
    venv/bin/python tools/icon/raster.py

出力するファイル:
  deck/icon/zerowave.ico           Windows（背景なし。exe に埋め込む。deck/build.rs）
  deck/icon/zerowave.icns          macOS（.app バンドル）
  deck/icon/about.png              「ZeroWave について」の画像（背景なし、256px）
  deck/icon/dock.png               macOS でバンドルにしていないときの Dock（512px）
  deck/linux/icons/hicolor/…       Linux（背景付き。deb が /usr/share/icons に置く）

サイズごとにどの版を使うか：16px は小、24〜64px は中、それより大きいものは大。
ICO と ICNS の中身は PNG のまま入れる（Windows Vista 以降・macOS 10.7 以降が読む）。
"""

import io
import os
import struct
import sys

import resvg_py

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
ICON = os.path.join(ROOT, "deck", "icon")
LINUX = os.path.join(ROOT, "deck", "linux", "icons", "hicolor")


def tier(px):
    if px <= 16:
        return "small"
    if px <= 64:
        return "medium"
    return "large"


def png(flavour, px):
    """`flavour` は "" "win" "macos"。"""
    name = "zerowave" + (f"-{flavour}" if flavour else "")
    step = tier(px)
    if step != "large":
        name += f"-{step}"
    path = os.path.join(ICON, name + ".svg")
    return bytes(resvg_py.svg_to_bytes(svg_path=path, width=px, height=px))


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as out:
        out.write(data)
    print(f"{os.path.relpath(path, ROOT)}  {len(data)} バイト")


def ico(sizes):
    images = [png("win", px) for px in sizes]
    head = struct.pack("<HHH", 0, 1, len(images))
    at = 6 + 16 * len(images)
    entries = b""
    for px, data in zip(sizes, images):
        side = 0 if px >= 256 else px
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), at)
        at += len(data)
    return head + entries + b"".join(images)


# 種類の四文字と、その画像の画素数・何 pt 用か。
ICNS = [
    (b"icp4", 16, 16),
    (b"ic11", 32, 16),
    (b"icp5", 32, 32),
    (b"ic12", 64, 32),
    (b"ic07", 128, 128),
    (b"ic13", 256, 128),
    (b"ic08", 256, 256),
    (b"ic14", 512, 256),
    (b"ic09", 512, 512),
    (b"ic10", 1024, 512),
]


def icns():
    chunks = b""
    for kind, px, _pt in ICNS:
        data = png("macos", px)
        chunks += kind + struct.pack(">I", 8 + len(data)) + data
    return b"icns" + struct.pack(">I", 8 + len(chunks)) + chunks


def main():
    write(os.path.join(ICON, "zerowave.ico"), ico([16, 20, 24, 32, 40, 48, 64, 96, 128, 256]))
    write(os.path.join(ICON, "zerowave.icns"), icns())
    write(os.path.join(ICON, "about.png"), png("win", 256))
    write(os.path.join(ICON, "dock.png"), png("macos", 512))
    for px in [16, 24, 32, 48, 64, 128, 256, 512]:
        write(os.path.join(LINUX, f"{px}x{px}", "apps", "zerowave.png"), png("", px))
    with open(os.path.join(ICON, "zerowave.svg"), "rb") as source:
        write(os.path.join(LINUX, "scalable", "apps", "zerowave.svg"), source.read())


if __name__ == "__main__":
    sys.exit(main())
