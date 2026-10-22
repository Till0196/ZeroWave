// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// 本体（zwhost）が提供する API。zwjavy が `globalThis.__zw` に追加する。
// 番号は canvas（1 はレイヤー自体）。

export interface ZwHost {
  canvasNew(w: number, h: number): number;
  canvasDrop(id: number): void;
  canvasResize(id: number, w: number, h: number): void;
  canvasWidth(id: number): number;
  canvasHeight(id: number): number;
  fillStyle(id: number, s: string): void;
  strokeStyle(id: number, s: string): void;
  font(id: number, s: string): void;
  textAlign(id: number, s: string): void;
  textBaseline(id: number, s: string): void;
  lineWidth(id: number, w: number): void;
  globalAlpha(id: number, a: number): void;
  save(id: number): void;
  restore(id: number): void;
  translate(id: number, x: number, y: number): void;
  scale(id: number, x: number, y: number): void;
  rotate(id: number, a: number): void;
  transform(id: number, a: number, b: number, c: number, d: number, e: number, f: number): void;
  setTransform(id: number, a: number, b: number, c: number, d: number, e: number, f: number): void;
  getTransform(id: number): number[];
  fillRect(id: number, x: number, y: number, w: number, h: number): void;
  strokeRect(id: number, x: number, y: number, w: number, h: number): void;
  clearRect(id: number, x: number, y: number, w: number, h: number): void;
  beginPath(id: number): void;
  closePath(id: number): void;
  moveTo(id: number, x: number, y: number): void;
  lineTo(id: number, x: number, y: number): void;
  quadraticCurveTo(id: number, cx: number, cy: number, x: number, y: number): void;
  bezierCurveTo(id: number, c1x: number, c1y: number, c2x: number, c2y: number, x: number, y: number): void;
  rect(id: number, x: number, y: number, w: number, h: number): void;
  arc(id: number, x: number, y: number, r: number, start: number, end: number, anticlockwise: boolean): void;
  fill(id: number): void;
  stroke(id: number): void;
  clip(id: number): void;
  fillText(id: number, s: string, x: number, y: number): void;
  strokeText(id: number, s: string, x: number, y: number): void;
  /** [幅, アセント, ディセント] */
  measureText(id: number, s: string): number[];
  drawCanvas(id: number, from: number, sx: number, sy: number, sw: number, sh: number, dx: number, dy: number, dw: number, dh: number): void;
  layerWidth(): number;
  layerHeight(): number;
  /** レイヤーを開いてからのミリ秒。 */
  nowMs(): number;
  /** 1970 年からのミリ秒。 */
  unixMs(): number;
  /** HTTP を開く。0 は開けなかった（`net` が無効など）。`headers` は `名前: 値` の行。 */
  netOpen(method: string, url: string, headers: string, body: Uint8Array | null): number;
  /** WebSocket を開く。`protocols` は `,` 区切り。 */
  netWs(url: string, protocols: string, headers: string): number;
  /** 届いた記録（zwsdk::net::conn）。無ければ null。 */
  netPoll(id: number): Uint8Array | null;
  /** WebSocket に送る。`kind` は 1 テキスト、2 バイナリ、8 close（u16 BE のコードと理由）。 */
  netSend(id: number, kind: number, bytes: Uint8Array): boolean;
  netClose(id: number): void;
  /** プラグインの設定の値。無ければ null。 */
  cfg(name: string): string | null;
}

// 本体（zwjavy）が呼ぶ関数。zwsdk の index が定義する。
declare global {
  // eslint-disable-next-line no-var
  var __zw: ZwHost;
  // eslint-disable-next-line no-var
  var __zwNew: ((json: string) => void) | undefined;
  // eslint-disable-next-line no-var
  var __zwMessage: ((json: string, bytes: Uint8Array | null) => void) | undefined;
  // eslint-disable-next-line no-var
  var __zwTick: (() => void) | undefined;
  // eslint-disable-next-line no-var
  var __zwFrame: (() => boolean) | undefined;
  // eslint-disable-next-line no-var
  var __zwEvents: (() => string) | undefined;
}

export const host: ZwHost = globalThis.__zw;
