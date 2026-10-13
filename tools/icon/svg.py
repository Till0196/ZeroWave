#!/usr/bin/env python3
# Copyright 2026 Till0196
# SPDX-License-Identifier: Apache-2.0
"""アイコンの SVG を書く（1024 四方）。サイズで詳細度を三段階に分ける。

    tools/icon/svg.py            # deck/icon/ に書く
    tools/icon/svg.py <出力先>

大（128px 以上）・中（24〜64px）・小（16px）の三段階を、背景付き（Linux）、
背景なし（Windows）、macOS のグリッド（本体 824 四方・影付き）の三通りで。
ラスタは tools/icon/raster.py で。
"""

import math
import os
import sys

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "..", "deck", "icon")
FRAME_COLORS = ["#3FD98F", "#46C79A", "#E06AD6", "#3A8DFF", "#6FE6FF"]  # back -> front


def squircle(x, y, s, n=5.0, steps=256):
    cx, cy, r = x + s / 2, y + s / 2, s / 2
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, si = math.cos(t), math.sin(t)
        px = cx + r * math.copysign(abs(c) ** (2 / n), c)
        py = cy + r * math.copysign(abs(si) ** (2 / n), si)
        pts.append(f"{px:.1f},{py:.1f}")
    return "M" + " L".join(pts) + " Z"


def rrect(x, y, s, rx):
    return f'M{x+rx},{y} H{x+s-rx} A{rx},{rx} 0 0 1 {x+s},{y+rx} V{y+s-rx} A{rx},{rx} 0 0 1 {x+s-rx},{y+s} H{x+rx} A{rx},{rx} 0 0 1 {x},{y+s-rx} V{y+rx} A{rx},{rx} 0 0 1 {x+rx},{y} Z'


def quad(x, y, w, h, dy, r):
    # 左右の辺が縦、上下の辺が右下がりの四角。角を r で丸める
    p = [(x, y), (x + w, y + dy), (x + w, y + dy + h), (x, y + h)]
    d = ""
    for i in range(4):
        a, b, c = p[i - 1], p[i], p[(i + 1) % 4]
        def toward(u, v, k):
            l = math.hypot(v[0] - u[0], v[1] - u[1])
            return (u[0] + (v[0] - u[0]) * k / l, u[1] + (v[1] - u[1]) * k / l)
        s = toward(b, a, r)
        e = toward(b, c, r)
        d += ("M" if i == 0 else " L") + f"{s[0]:.1f},{s[1]:.1f} Q{b[0]:.1f},{b[1]:.1f} {e[0]:.1f},{e[1]:.1f}"
    return d + " Z"


def wave_path(i, n, x0, x1, cy, amp, lam, spread, fan=0, base=7, xc=120 + 130):
    # 束の i 本目。振れ幅は揃え、少しずつずらして帯にする
    k = (i - (n - 1) / 2) / max(1, (n - 1) / 2)
    pts = []
    steps = 120
    for j in range(steps + 1):
        x = x0 + (x1 - x0) * j / steps
        ph = 2 * math.pi * (x - xc) / lam + math.pi / 2
        a = amp * (1 + 0.04 * k)
        # 両端へ行くほど束をわずかに開く
        e = min(1.0, abs(x - 512) / 512) ** 2
        y = cy - a * math.sin(ph + k * spread) + k * (base + fan * e)
        pts.append(f"{x:.1f},{y:.1f}")
    return "M" + " L".join(pts)


def content(p):
    """1024 四方いっぱいの本体の図形"""
    o = []
    n = p["frames"]
    colors = FRAME_COLORS[-n:] if n < 5 else FRAME_COLORS
    if n == 3:
        colors = [FRAME_COLORS[0], FRAME_COLORS[2], FRAME_COLORS[4]]
    w, h, dy = p["fw"], p["fh"], p["fdy"]
    sx, sy = p["step"]
    spanx = w + (n - 1) * sx
    x0 = 512 - spanx / 2
    # 縦は一番奥の上端から手前の右下まで
    top = -(n - 1) * sy
    bot = h + dy
    # 板の束はアイコンの真ん中に置き、波は手前の板の真ん中寄りを通す
    c = (n - 1) * sy / 4
    y0 = 512 - (top + bot) / 2
    cy = 512 + 2 * c

    # 走査線
    fade = ' mask="url(#fade)"' if p.get("bare") else ""
    if p["scan"]:
        o.append(f"<g{fade}>")
        for k, yy in enumerate(p["scan"]):
            c = "#C9D6EA" if yy == 0 else "#2E6FD0"
            op = 0.55 if yy == 0 else 0.45
            o.append(f'<line x1="0" x2="1024" y1="{cy+yy}" y2="{cy+yy}" stroke="{c}" stroke-opacity="{op}" stroke-width="{p["scanw"]}"/>')
        o.append("</g>")

    # 奥から手前へ
    for i in range(n):
        x = x0 + (n - 1 - i) * sx
        y = y0 - (n - 1 - i) * sy
        c = colors[i]
        d = quad(x, y, w, h, dy, p["fr"])
        front = i == n - 1
        fo = 0.20 if front else 0.13
        if p.get("bare"):
            o.append(f'<path d="{d}" fill="#0E1320" fill-opacity="{p.get("under", 0.55)}"/>')
        o.append(f'<path d="{d}" fill="url(#pane{i})" />')
        o.append(f'<path d="{d}" fill="none" stroke="{c}" stroke-width="{p["fsw"]*p.get("halo",2.6):.1f}" stroke-opacity="0.35" filter="url(#soft)"/>')
        o.append(f'<path d="{d}" fill="none" stroke="{c}" stroke-width="{p["fsw"]}" stroke-linejoin="round"/>')
        o.insert(0, f'<linearGradient id="pane{i}" x1="{x}" y1="{y}" x2="{x+w}" y2="{y+h+dy}" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="{c}" stop-opacity="{fo+0.08:.2f}"/><stop offset="1" stop-color="{c}" stop-opacity="{fo*0.35:.2f}"/></linearGradient>')
        if front:
            fx, fy = x, y
    # 手前の板のつや
    if p["sheen"]:
        g = quad(fx + w * 0.30, fy + dy * 0.30, w * 0.34, h, dy * 0.34, 4)
        o.append(f'<path d="M{fx+w*0.42:.1f},{fy+dy*0.42+6:.1f} L{fx+w*0.62:.1f},{fy+dy*0.62+6:.1f} L{fx+w*0.30:.1f},{fy+h+dy*0.30-6:.1f} L{fx+w*0.10:.1f},{fy+h+dy*0.10-6:.1f} Z" fill="#FFFFFF" fill-opacity="0.06"/>')

    # 波の束
    nw = p["waves"]
    wx0, wx1 = (p.get("wx0", 40), 1024 - p.get("wx0", 40)) if p.get("bare") else (-20, 1044)
    lam = w * p["lamk"]
    xc = fx + w * p.get("crest", 0.2)
    waves = [wave_path(i, nw, wx0, wx1, cy, p["amp"], lam, p["spread"], p.get("fan", 0), p.get("base", 7), xc) for i in range(nw)]
    o.append(f"<g{fade}>")
    allw = " ".join(waves)
    o.append(f'<path d="{allw}" fill="none" stroke="#4FB6FF" stroke-width="{p["wsw"]*p.get("whalo",4):.1f}" stroke-opacity="0.45" filter="url(#glow)" stroke-linecap="round"/>')
    for i, wv in enumerate(waves):
        mid = abs(i - (nw - 1) / 2) < 0.6
        o.append(f'<path d="{wv}" fill="none" stroke="url(#wave)" stroke-width="{p["wsw"]*(1.25 if mid else 1):.1f}" stroke-linecap="round"/>')
    o.append("</g>")

    # 手前の板の四隅のかぎ
    if p["brackets"]:
        L, ins, bw = p["blen"], p["bins"], p["bsw"]
        k = dy / w
        def pt(u, v):
            return (fx + u, fy + v + k * u)
        corners = [
            (ins, ins, 1, 1), (w - ins, ins, -1, 1), (ins, h - ins, 1, -1), (w - ins, h - ins, -1, -1)
        ]
        for u, v, su, sv in corners:
            a = pt(u, v + sv * L)
            b = pt(u, v)
            c = pt(u + su * L, v)
            o.append(f'<path d="M{a[0]:.1f},{a[1]:.1f} L{b[0]:.1f},{b[1]:.1f} L{c[0]:.1f},{c[1]:.1f}" fill="none" stroke="#F4F8FF" stroke-width="{bw}" stroke-linecap="round" stroke-linejoin="round"/>')
    return o


def defs(p):
    return f'''<linearGradient id="bg" x1="0" y1="0" x2="0" y2="1024" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#1A1F2D"/><stop offset="1" stop-color="#0B0E15"/></linearGradient>
<linearGradient id="fadeg" x1="0" y1="0" x2="1024" y2="0" gradientUnits="userSpaceOnUse"><stop offset="0.02" stop-color="#FFFFFF" stop-opacity="0"/><stop offset="0.12" stop-color="#FFFFFF"/><stop offset="0.88" stop-color="#FFFFFF"/><stop offset="0.98" stop-color="#FFFFFF" stop-opacity="0"/></linearGradient>
<mask id="fade" maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024"><rect width="1024" height="1024" fill="url(#fadeg)"/></mask>
<linearGradient id="wave" x1="0" y1="0" x2="1024" y2="0" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#2F7DFF"/><stop offset="0.35" stop-color="#7FE9FF"/><stop offset="0.7" stop-color="#9AF0FF"/><stop offset="1" stop-color="#3B8CFF"/></linearGradient>
<linearGradient id="rim" x1="0" y1="0" x2="0" y2="1024" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#FFFFFF" stop-opacity="0.16"/><stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0.03"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0.08"/></linearGradient>
<filter id="glow" x="-10%" y="-30%" width="120%" height="160%"><feGaussianBlur stdDeviation="{p["glow"]}"/></filter>
<filter id="soft" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="{p["glow"]*0.8:.1f}"/></filter>'''


def build(p, mac):
    body_full = squircle(0, 0, 1024) if p["shape"] == "squircle" else rrect(0, 0, 1024, p["rx"])
    inner = content(p)
    grads = [s for s in inner if s.startswith("<linearGradient")]
    rest = [s for s in inner if not s.startswith("<linearGradient")]
    rimw = p.get("rimw", 4)
    g = "" if p.get("bare") else f'''<clipPath id="body"><path d="{body_full}"/></clipPath>
'''
    art = f'''<g clip-path="url(#body)">
<path d="{body_full}" fill="url(#bg)"/>
{chr(10).join(rest)}
</g>
<path d="{body_full}" fill="none" stroke="url(#rim)" stroke-width="{rimw}"/>'''
    if p.get("bare"):
        art = chr(10).join(rest)
    if mac:
        s = p["macsize"]
        off = (1024 - s) / 2
        sc = s / 1024
        shadow = '<filter id="drop" x="-10%" y="-10%" width="120%" height="125%"><feGaussianBlur in="SourceAlpha" stdDeviation="10"/><feOffset dy="10"/><feComponentTransfer><feFuncA type="linear" slope="0.3"/></feComponentTransfer><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter>'
        art = f'<g filter="url(#drop)"><g transform="translate({off:g} {off:g}) scale({sc:.6f})">{art}</g></g>'
        g += shadow
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
<title>zerowave</title>
<defs>
{defs(p)}
{chr(10).join(grads)}
{g}</defs>
{art}
</svg>
'''


FULL = dict(shape="rrect", rx=230, frames=5, fw=540, fh=420, fdy=140, step=(62, 42), fr=16, fsw=7,
            scan=[-84, -42, 0, 42, 84], scanw=2.5, sheen=True, waves=7, amp=104, lamk=1.3, spread=0.16, fan=14, base=13,
            wsw=4.4, brackets=True, blen=44, bins=28, bsw=7, glow=9, macsize=824)
MID = dict(FULL, frames=4, fw=530, fh=415, fdy=135, step=(70, 48), fsw=16, scan=[-60, 0, 60], scanw=6,
           sheen=False, waves=4, wsw=11, amp=108, spread=0.2, fan=12, base=16, blen=60, bins=40, bsw=16, glow=12, rimw=8)
SMALL = dict(FULL, rx=190, frames=3, fw=560, fh=420, fdy=120, step=(84, 60), fr=28, fsw=52, scan=[],
             sheen=False, waves=1, amp=100, lamk=1.4, spread=0, fan=0, wsw=58, brackets=False, glow=7, halo=1.5,
             whalo=1.8, rimw=0)
FULL_MAC = dict(FULL, shape="squircle", fw=580, fh=450, fdy=150, step=(66, 45), amp=112)
MID_MAC = dict(MID, shape="squircle", fw=560, fh=440, fdy=140, amp=114)
SMALL_MAC = dict(SMALL, shape="squircle", macsize=880)
# Windows は背景なし。板の裏を暗くして明るい背景でも見えるように
FULL_WIN = dict(FULL, bare=True, fw=560, fh=440, fdy=145, step=(66, 45), fsw=9, bsw=9, blen=50, bins=32,
                amp=110, fan=14, base=14, wsw=5.2, wx0=24, scan=[-96, -48, 0, 48, 96], scanw=3)
MID_WIN = dict(MID, bare=True, fw=620, fh=480, fdy=160, step=(80, 56), fsw=20, bsw=20, blen=70, bins=46,
               amp=112, wsw=14, wx0=24, scan=[], under=0.6)
SMALL_WIN = dict(SMALL, bare=True, fw=660, fh=500, fdy=150, step=(100, 72), fsw=60, amp=112, wsw=64,
                 wx0=40, under=0.7)

os.makedirs(OUT, exist_ok=True)
for name, p, mac in [
    ("zerowave.svg", FULL, False), ("zerowave-medium.svg", MID, False), ("zerowave-small.svg", SMALL, False),
    ("zerowave-macos.svg", FULL_MAC, True), ("zerowave-macos-medium.svg", MID_MAC, True),
    ("zerowave-macos-small.svg", SMALL_MAC, True),
    ("zerowave-win.svg", FULL_WIN, False), ("zerowave-win-medium.svg", MID_WIN, False),
    ("zerowave-win-small.svg", SMALL_WIN, False),
]:
    with open(os.path.join(OUT, name), "w") as f:
        f.write(build(p, mac))

