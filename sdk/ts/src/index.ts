// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// ZeroWave のレイヤー（layer hook）を TypeScript で。Rust のプラグインと同じ API。
//
//   import { layer, onFrame, onMessage, send } from "zwsdk";
//   const ctx = layer.getContext("2d")!;
//   onMessage((message) => { if ("Key" in message) … });
//   onFrame(() => { ctx.clearRect(0, 0, layer.width, layer.height); ... });
//
// `layer` は番号 1 の canvas（座標は本体が渡す設定の `size`、既定 1920×1080）。
// 本体は、メッセージが来れば `__zwMessage`、時刻を進めるとき `__zwTick`（タイマー・
// requestAnimationFrame はここで実行される）、レンダリングのとき `__zwFrame`、要求を取得するとき
// `__zwEvents` を呼ぶ。通信（fetch・WebSocket）は `__zwTick` のたびに届いた分を配る。
// どのレイヤーか・何が必要かは package.json の
// `zerowave.layer`（`{"above":"captions","wants":["clock"],"fps":60}`）。
// esbuild で一つにまとめ、javy で wasm にする（zwsdk/javy/zwjavy.wasm）。

import { ZwCanvas } from "./canvas";
import { installDom, runQueues, takePainted } from "./dom";
import { host } from "./host";
import { installNet, pollNet } from "./net";

export { ZwCanvas, ZwContext2D } from "./canvas";
export { host } from "./host";
export type { ZwHost } from "./host";
export { ZwHeaders, ZwResponse, ZwWebSocket, zwFetch } from "./net";

installDom();
installNet();

/** 本体が起動時に渡すもの（`zwsdk::layer::Config`）。 */
export interface Config {
  nvram?: [string, number[]][];
  size?: [number, number];
}

/** 本体 → レイヤーのメッセージ（`zwsdk::layer::Message` の JSON。`{"Key":"Up"}` など）。 */
export type Message = Record<string, unknown> | string;

/** レイヤー → 本体の要求（`zwsdk::layer::Event` の JSON）。 */
export type Event =
  | { Log: string }
  | { Tune: { network: number; transport: number; service: number } }
  | { Sound: number }
  | { CaptionDisplay: { language: number; on: boolean } };

/** レイヤー自体（番号 1、本体が作る）。 */
export const layer: ZwCanvas = new ZwCanvas(0, 0, 1);

let userFrame: (() => void | boolean) | null = null;
let userMessage: ((message: Message, bytes: Uint8Array | null) => void) | null = null;
let userNew: ((config: Config) => void) | null = null;
let pending: Event[] = [];

/** レンダリングのたびに呼ばれる（マニフェストの `fps`、無ければ 20 回/秒）。
 *  `false` を返せば「変わっていない」（本体はフレームを取得し直さない）。
 *  requestAnimationFrame でレンダリングするレイヤーは定義しなくてよい。 */
export function onFrame(fn: () => void | boolean): void {
  userFrame = fn;
}

/** 本体からメッセージが来たとき。大きなデータは `bytes` に添えて来る。 */
export function onMessage(fn: (message: Message, bytes: Uint8Array | null) => void): void {
  userMessage = fn;
}

/** 本体が起動時に渡すもの。 */
export function onNew(fn: (config: Config) => void): void {
  userNew = fn;
}

/** 本体に要求を送る。次の `__zwEvents` で渡る。 */
export function send(event: Event): void {
  pending.push(event);
}

/** レイヤーを開いてからのミリ秒。 */
export function now(): number {
  return host.nowMs();
}

/** プラグインの設定（package.json の `zerowave.config` で宣言したキー）の値。無ければ null。 */
export function cfg(name: string): string | null {
  return host.cfg(name);
}

function guarded(what: string, fn: () => void): void {
  try {
    fn();
  } catch (error) {
    console.log(`${what}: ${String(error)}`);
  }
}

globalThis.__zwNew = (json: string) => {
  let config: Config = {};
  try {
    config = JSON.parse(json) as Config;
  } catch {
    config = {};
  }
  if (userNew) {
    guarded("onNew", () => userNew!(config));
  }
};

globalThis.__zwMessage = (json: string, bytes: Uint8Array | null) => {
  if (!userMessage) {
    return;
  }
  let message: Message;
  try {
    message = JSON.parse(json) as Message;
  } catch {
    return;
  }
  guarded("onMessage", () => userMessage!(message, bytes && bytes.length > 0 ? bytes : null));
};

globalThis.__zwTick = () => {
  pollNet();
  runQueues();
};

globalThis.__zwFrame = (): boolean => {
  // requestAnimationFrame でレンダリングするレイヤー（niconicomments など）は onFrame を
  // 定義しない。そのコールバックが実行されていればフレームは変わっている。
  const viaRaf = takePainted();
  if (!userFrame) {
    return viaRaf;
  }
  let drew = true;
  guarded("onFrame", () => {
    if (userFrame!() === false) {
      drew = false;
    }
  });
  return drew || viaRaf;
};

globalThis.__zwEvents = (): string => {
  if (pending.length === 0) {
    return "";
  }
  const out = JSON.stringify(pending);
  pending = [];
  return out;
};
