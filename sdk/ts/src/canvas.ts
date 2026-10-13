// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// canvas 2D のインターフェース。レンダリングは本体（zwdraw）が行い、ここは番号を持って API を呼ぶだけ。
// スタイル（fillStyle など）はここで覚えて、変えたときだけ本体へ送る。

import { host } from "./host";

/** 本体が持たないもの（グラデーションなど）のダミーオブジェクト。呼べるだけ。 */
class Stub {
  addColorStop(): void {}
}

export class ZwContext2D {
  readonly canvas: ZwCanvas;
  readonly id: number;
  private _fillStyle = "#000000";
  private _strokeStyle = "#000000";
  private _font = "10px sans-serif";
  private _textAlign = "start";
  private _textBaseline = "alphabetic";
  private _lineWidth = 1;
  private _globalAlpha = 1;
  private stack: string[][] = [];
  // 本体が持たないもの。値は覚えるだけ。
  globalCompositeOperation = "source-over";
  lineCap = "butt";
  lineJoin = "miter";
  miterLimit = 10;
  imageSmoothingEnabled = true;
  shadowBlur = 0;
  shadowColor = "rgba(0, 0, 0, 0)";
  shadowOffsetX = 0;
  shadowOffsetY = 0;

  constructor(canvas: ZwCanvas, id: number) {
    this.canvas = canvas;
    this.id = id;
  }

  get fillStyle(): string { return this._fillStyle; }
  set fillStyle(v: string | Stub) {
    if (typeof v !== "string") return;
    this._fillStyle = v;
    host.fillStyle(this.id, v);
  }
  get strokeStyle(): string { return this._strokeStyle; }
  set strokeStyle(v: string | Stub) {
    if (typeof v !== "string") return;
    this._strokeStyle = v;
    host.strokeStyle(this.id, v);
  }
  get font(): string { return this._font; }
  set font(v: string) { this._font = v; host.font(this.id, v); }
  get textAlign(): string { return this._textAlign; }
  set textAlign(v: string) { this._textAlign = v; host.textAlign(this.id, v); }
  get textBaseline(): string { return this._textBaseline; }
  set textBaseline(v: string) { this._textBaseline = v; host.textBaseline(this.id, v); }
  get lineWidth(): number { return this._lineWidth; }
  set lineWidth(v: number) { this._lineWidth = v; host.lineWidth(this.id, v); }
  get globalAlpha(): number { return this._globalAlpha; }
  set globalAlpha(v: number) { this._globalAlpha = v; host.globalAlpha(this.id, v); }

  /** 大きさを変えた後：本体のスタイルが既定に戻るので、こちらも戻す。 */
  reset(): void {
    this._fillStyle = "#000000";
    this._strokeStyle = "#000000";
    this._font = "10px sans-serif";
    this._textAlign = "start";
    this._textBaseline = "alphabetic";
    this._lineWidth = 1;
    this._globalAlpha = 1;
    this.stack = [];
  }

  save(): void {
    this.stack.push([
      this._fillStyle, this._strokeStyle, this._font, this._textAlign,
      this._textBaseline, String(this._lineWidth), String(this._globalAlpha),
    ]);
    host.save(this.id);
  }
  restore(): void {
    const top = this.stack.pop();
    if (top) {
      [this._fillStyle, this._strokeStyle, this._font, this._textAlign, this._textBaseline] = top;
      this._lineWidth = Number(top[5]);
      this._globalAlpha = Number(top[6]);
    }
    host.restore(this.id);
  }

  translate(x: number, y: number): void { host.translate(this.id, x, y); }
  scale(x: number, y: number = x): void { host.scale(this.id, x, y); }
  rotate(a: number): void { host.rotate(this.id, a); }
  transform(a: number, b: number, c: number, d: number, e: number, f: number): void {
    host.transform(this.id, a, b, c, d, e, f);
  }
  setTransform(a: number | DOMMatrixLike = 1, b = 0, c = 0, d = 1, e = 0, f = 0): void {
    if (typeof a === "object") {
      host.setTransform(this.id, a.a ?? 1, a.b ?? 0, a.c ?? 0, a.d ?? 1, a.e ?? 0, a.f ?? 0);
    } else {
      host.setTransform(this.id, a, b, c, d, e, f);
    }
  }
  resetTransform(): void { host.setTransform(this.id, 1, 0, 0, 1, 0, 0); }
  getTransform(): DOMMatrixLike {
    const [a, b, c, d, e, f] = host.getTransform(this.id);
    return { a, b, c, d, e, f };
  }

  fillRect(x: number, y: number, w: number, h: number): void { host.fillRect(this.id, x, y, w, h); }
  strokeRect(x: number, y: number, w: number, h: number): void { host.strokeRect(this.id, x, y, w, h); }
  clearRect(x: number, y: number, w: number, h: number): void { host.clearRect(this.id, x, y, w, h); }

  beginPath(): void { host.beginPath(this.id); }
  closePath(): void { host.closePath(this.id); }
  moveTo(x: number, y: number): void { host.moveTo(this.id, x, y); }
  lineTo(x: number, y: number): void { host.lineTo(this.id, x, y); }
  quadraticCurveTo(cx: number, cy: number, x: number, y: number): void {
    host.quadraticCurveTo(this.id, cx, cy, x, y);
  }
  bezierCurveTo(c1x: number, c1y: number, c2x: number, c2y: number, x: number, y: number): void {
    host.bezierCurveTo(this.id, c1x, c1y, c2x, c2y, x, y);
  }
  rect(x: number, y: number, w: number, h: number): void { host.rect(this.id, x, y, w, h); }
  arc(x: number, y: number, r: number, start: number, end: number, anticlockwise = false): void {
    host.arc(this.id, x, y, r, start, end, anticlockwise);
  }
  arcTo(): void {}
  ellipse(): void {}
  fill(): void { host.fill(this.id); }
  stroke(): void { host.stroke(this.id); }
  clip(): void { host.clip(this.id); }
  isPointInPath(): boolean { return false; }
  setLineDash(): void {}
  getLineDash(): number[] { return []; }

  fillText(text: string, x: number, y: number): void { host.fillText(this.id, String(text), x, y); }
  strokeText(text: string, x: number, y: number): void { host.strokeText(this.id, String(text), x, y); }
  measureText(text: string): TextMetricsLike {
    const [width, ascent, descent] = host.measureText(this.id, String(text));
    return {
      width,
      actualBoundingBoxLeft: 0,
      actualBoundingBoxRight: width,
      actualBoundingBoxAscent: ascent,
      actualBoundingBoxDescent: descent,
      fontBoundingBoxAscent: ascent,
      fontBoundingBoxDescent: descent,
    };
  }

  /** 元は別の canvas だけ（`img` は持たない）。 */
  drawImage(source: unknown, ...a: number[]): void {
    if (!(source instanceof ZwCanvas)) return;
    const from = source.id;
    const sw = source.width;
    const sh = source.height;
    if (a.length >= 8) {
      host.drawCanvas(this.id, from, a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]);
    } else if (a.length >= 4) {
      host.drawCanvas(this.id, from, 0, 0, sw, sh, a[0], a[1], a[2], a[3]);
    } else if (a.length >= 2) {
      host.drawCanvas(this.id, from, 0, 0, sw, sh, a[0], a[1], sw, sh);
    }
  }

  getImageData(_x: number, _y: number, w: number, h: number): ImageDataLike {
    return { width: w, height: h, data: new Uint8ClampedArray(Math.max(0, w * h * 4)) };
  }
  createImageData(w: number, h: number): ImageDataLike {
    return this.getImageData(0, 0, w, h);
  }
  putImageData(): void {}
  createLinearGradient(): Stub { return new Stub(); }
  createRadialGradient(): Stub { return new Stub(); }
  createPattern(): Stub { return new Stub(); }
}

export interface DOMMatrixLike { a: number; b: number; c: number; d: number; e: number; f: number }

export interface TextMetricsLike {
  width: number;
  actualBoundingBoxLeft: number;
  actualBoundingBoxRight: number;
  actualBoundingBoxAscent: number;
  actualBoundingBoxDescent: number;
  fontBoundingBoxAscent: number;
  fontBoundingBoxDescent: number;
}

export interface ImageDataLike { width: number; height: number; data: Uint8ClampedArray }

/** `HTMLCanvasElement` のインターフェース。番号 1 はレイヤー自体（本体が作る）。 */
export class ZwCanvas {
  readonly id: number;
  readonly tagName = "CANVAS";
  readonly nodeName = "CANVAS";
  style: Record<string, string> = {};
  private context: ZwContext2D | null = null;

  constructor(width = 300, height = 150, id?: number) {
    this.id = id ?? host.canvasNew(width, height);
  }

  get width(): number { return host.canvasWidth(this.id); }
  set width(v: number) { this.resize(v, this.height); }
  get height(): number { return host.canvasHeight(this.id); }
  set height(v: number) { this.resize(this.width, v); }
  get clientWidth(): number { return this.width; }
  get clientHeight(): number { return this.height; }
  get offsetWidth(): number { return this.width; }
  get offsetHeight(): number { return this.height; }

  private resize(w: number, h: number): void {
    host.canvasResize(this.id, Math.max(0, Math.floor(w)), Math.max(0, Math.floor(h)));
    this.context?.reset();
  }

  getContext(kind: string): ZwContext2D | null {
    if (kind !== "2d") return null;
    if (!this.context) this.context = new ZwContext2D(this, this.id);
    return this.context;
  }
  toDataURL(): string { return "data:,"; }
  getBoundingClientRect(): { x: number; y: number; width: number; height: number; top: number; left: number; right: number; bottom: number } {
    const [width, height] = [this.width, this.height];
    return { x: 0, y: 0, width, height, top: 0, left: 0, right: width, bottom: height };
  }
  addEventListener(): void {}
  removeEventListener(): void {}
  setAttribute(name: string, value: string): void {
    if (name === "width") this.width = Number(value);
    if (name === "height") this.height = Number(value);
  }
  getAttribute(name: string): string | null {
    if (name === "width") return String(this.width);
    if (name === "height") return String(this.height);
    return null;
  }
  /** 使い終わった canvas を本体から外す（レイヤー自体は外れない）。 */
  release(): void {
    host.canvasDrop(this.id);
  }
}
