// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// ブラウザの API の最小限：document.createElement("canvas")、タイマー、
// requestAnimationFrame、performance.now。wasm の中にはイベントループが無いので、
// タイマーと rAF は本体が毎回呼ぶ `frame()` の中で実行する。

import { ZwCanvas } from "./canvas";
import { host } from "./host";

interface Timer {
  id: number;
  due: number;
  every: number | null;
  fn: () => void;
}

let timers: Timer[] = [];
let nextTimer = 1;
let rafs: { id: number; fn: (t: number) => void }[] = [];
let nextRaf = 1;
/** 前回本体がフレームを取得してから、requestAnimationFrame のコールバックが実行されたか。 */
let painted = false;

export function takePainted(): boolean {
  const was = painted;
  painted = false;
  return was;
}

function setTimer(fn: () => void, ms: number, every: boolean): number {
  const delay = Math.max(0, Number(ms) || 0);
  const id = nextTimer++;
  timers.push({ id, due: host.nowMs() + delay, every: every ? delay : null, fn });
  return id;
}

function clearTimer(id: number): void {
  timers = timers.filter((t) => t.id !== id);
}

/** 期限の来たタイマーと、積んだ rAF を実行する。本体が毎回呼ぶ。 */
export function runQueues(): void {
  const now = host.nowMs();
  const due = timers.filter((t) => t.due <= now);
  for (const t of due) {
    if (t.every !== null) {
      t.due = now + t.every;
    } else {
      clearTimer(t.id);
    }
    try {
      t.fn();
    } catch (error) {
      console.log(`timer: ${String(error)}`);
    }
  }
  const batch = rafs;
  rafs = [];
  if (batch.length > 0) {
    painted = true;
  }
  for (const r of batch) {
    try {
      r.fn(now);
    } catch (error) {
      console.log(`requestAnimationFrame: ${String(error)}`);
    }
  }
}

const g = globalThis as unknown as Record<string, unknown>;

/** 放送の時刻は日本標準時。本体の `Date` と同じ（マシンのタイムゾーンに依らない）。 */
const TZ_OFFSET_MIN = 540;

/** wasm の中の `Date` は UTC しか知らないので、読む側を日本標準時に。 */
function installJstDate(): void {
  const proto = Date.prototype as unknown as Record<string, unknown>;
  const shifted = (d: Date) => new Date(d.getTime() + TZ_OFFSET_MIN * 60000);
  const pairs: [string, string][] = [
    ["getFullYear", "getUTCFullYear"],
    ["getMonth", "getUTCMonth"],
    ["getDate", "getUTCDate"],
    ["getDay", "getUTCDay"],
    ["getHours", "getUTCHours"],
    ["getMinutes", "getUTCMinutes"],
    ["getSeconds", "getUTCSeconds"],
    ["getMilliseconds", "getUTCMilliseconds"],
  ];
  for (const [local, utc] of pairs) {
    proto[local] = function (this: Date) {
      return (shifted(this) as unknown as Record<string, () => number>)[utc]();
    };
  }
  proto.getTimezoneOffset = () => -TZ_OFFSET_MIN;
}

export function installDom(): void {
  if (g.__zwDomInstalled) return;
  g.__zwDomInstalled = true;
  // Javy の console は log と error だけ。
  const c = console as unknown as Record<string, unknown>;
  c.warn = c.warn ?? console.error;
  c.info = c.info ?? console.log;
  c.debug = c.debug ?? console.log;
  installJstDate();
  g.setTimeout = (fn: () => void, ms = 0) => setTimer(fn, ms, false);
  g.setInterval = (fn: () => void, ms = 0) => setTimer(fn, ms, true);
  g.clearTimeout = clearTimer;
  g.clearInterval = clearTimer;
  g.requestAnimationFrame = (fn: (t: number) => void) => {
    const id = nextRaf++;
    rafs.push({ id, fn });
    return id;
  };
  g.cancelAnimationFrame = (id: number) => {
    rafs = rafs.filter((r) => r.id !== id);
  };
  g.performance = { now: () => host.nowMs(), timeOrigin: host.unixMs() - host.nowMs() };
  g.HTMLCanvasElement = ZwCanvas;
  g.OffscreenCanvas = ZwCanvas;
  g.Image = class {
    width = 0;
    height = 0;
    src = "";
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
  };
  g.navigator = { userAgent: "ZeroWave", language: "ja" };
  g.window = globalThis;
  g.self = globalThis;
  const element = () => ({
    style: {},
    children: [],
    appendChild() {},
    removeChild() {},
    setAttribute() {},
    getAttribute: () => null,
    addEventListener() {},
    removeEventListener() {},
  });
  g.document = {
    createElement(tag: string) {
      return String(tag).toLowerCase() === "canvas" ? new ZwCanvas() : element();
    },
    getElementById: () => null,
    querySelector: () => null,
    documentElement: element(),
    body: element(),
    addEventListener() {},
    removeEventListener() {},
  };
}
