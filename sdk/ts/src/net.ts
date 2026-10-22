// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// 通信：fetch と WebSocket。本体（zwhost）の待たない通信（netOpen／netWs）を開き、時刻を進めるたびに
// netPoll で届いた分を受け取る。繋がるのは、プラグインが設定 `net`（package.json の
// `zerowave.config` に {"key":"net","kind":"bool"}）を宣言して、それが有効なときだけ。
// 内部ネットワーク（LAN・loopback）へはユーザーが `net_local` を有効にしたときだけ。

import { host } from "./host";

// 記録の種類（zwsdk::net::conn::kind）。
const FAIL = 0;
const HEAD = 1;
const DATA = 2;
const TEXT = 3;
const END = 4;

// 送る種類（zwsdk::net::conn::send）。
const SEND_TEXT = 1;
const SEND_BINARY = 2;
const SEND_CLOSE = 8;

interface Piece {
  kind: number;
  body: Uint8Array;
}

function decode(bytes: Uint8Array): Piece[] {
  const out: Piece[] = [];
  let at = 0;
  while (at + 5 <= bytes.length) {
    const kind = bytes[at];
    const len = ((bytes[at + 1] << 24) | (bytes[at + 2] << 16) | (bytes[at + 3] << 8) | bytes[at + 4]) >>> 0;
    at += 5;
    if (at + len > bytes.length) break;
    out.push({ kind, body: bytes.subarray(at, at + len) });
    at += len;
  }
  return out;
}

function u16(body: Uint8Array): number {
  return body.length >= 2 ? (body[0] << 8) | body[1] : 0;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** 開いている接続：番号 → 記録を受け取る関数。 */
const live = new Map<number, (piece: Piece) => void>();

/** 届いた分を配る。本体が時刻を進めるたび（`__zwTick`）に呼ぶ。 */
export function pollNet(): void {
  for (const [id, take] of [...live]) {
    for (;;) {
      if (!live.has(id)) break;
      const bytes = host.netPoll(id);
      if (!bytes) break;
      for (const piece of decode(bytes)) {
        try {
          take(piece);
        } catch (error) {
          console.log(`net: ${String(error)}`);
        }
      }
    }
  }
}

function closeConn(id: number): void {
  if (live.delete(id)) host.netClose(id);
}

function headerLines(headers: unknown): string {
  const lines: string[] = [];
  if (headers instanceof ZwHeaders) {
    headers.forEach((value, name) => lines.push(`${name}: ${value}`));
  } else if (Array.isArray(headers)) {
    for (const [name, value] of headers as [string, string][]) lines.push(`${name}: ${value}`);
  } else if (headers && typeof headers === "object") {
    for (const [name, value] of Object.entries(headers as Record<string, string>)) lines.push(`${name}: ${value}`);
  }
  return lines.join("\n");
}

function bytesOf(data: unknown): Uint8Array | null {
  if (data === undefined || data === null) return null;
  if (typeof data === "string") return encoder.encode(data);
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return encoder.encode(String(data));
}

function concat(parts: Uint8Array[]): Uint8Array {
  let size = 0;
  for (const part of parts) size += part.length;
  const out = new Uint8Array(size);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

export class ZwHeaders {
  private map = new Map<string, string>();

  constructor(init?: unknown) {
    if (init instanceof ZwHeaders) {
      init.forEach((value, name) => this.append(name, value));
    } else if (Array.isArray(init)) {
      for (const [name, value] of init as [string, string][]) this.append(name, value);
    } else if (init && typeof init === "object") {
      for (const [name, value] of Object.entries(init as Record<string, string>)) this.append(name, value);
    }
  }

  append(name: string, value: string): void {
    const key = name.toLowerCase();
    const had = this.map.get(key);
    this.map.set(key, had === undefined ? String(value) : `${had}, ${value}`);
  }

  set(name: string, value: string): void {
    this.map.set(name.toLowerCase(), String(value));
  }

  get(name: string): string | null {
    return this.map.get(name.toLowerCase()) ?? null;
  }

  has(name: string): boolean {
    return this.map.has(name.toLowerCase());
  }

  delete(name: string): void {
    this.map.delete(name.toLowerCase());
  }

  forEach(fn: (value: string, name: string) => void): void {
    this.map.forEach((value, name) => fn(value, name));
  }

  entries(): IterableIterator<[string, string]> {
    return this.map.entries();
  }

  [Symbol.iterator](): IterableIterator<[string, string]> {
    return this.map.entries();
  }
}

/** fetch の本文。届いた分を読む（`getReader().read()`）か、全部を待つ（`text()` など）。 */
class Body {
  private queue: Uint8Array[] = [];
  private waiting: ((result: { done: boolean; value?: Uint8Array }) => void) | null = null;
  private failing: ((error: Error) => void) | null = null;
  private ended = false;
  private error: Error | null = null;
  used = false;

  constructor(private readonly cancel: () => void) {}

  push(bytes: Uint8Array): void {
    if (this.waiting) {
      const resolve = this.waiting;
      this.waiting = null;
      this.failing = null;
      resolve({ done: false, value: bytes });
    } else {
      this.queue.push(bytes);
    }
  }

  end(): void {
    this.ended = true;
    if (this.waiting) {
      const resolve = this.waiting;
      this.waiting = null;
      this.failing = null;
      resolve({ done: true });
    }
  }

  fail(error: Error): void {
    this.error = error;
    if (this.failing) {
      const reject = this.failing;
      this.waiting = null;
      this.failing = null;
      reject(error);
    }
  }

  read(): Promise<{ done: boolean; value?: Uint8Array }> {
    const next = this.queue.shift();
    if (next) return Promise.resolve({ done: false, value: next });
    if (this.error) return Promise.reject(this.error);
    if (this.ended) return Promise.resolve({ done: true });
    return new Promise((resolve, reject) => {
      this.waiting = resolve;
      this.failing = reject;
    });
  }

  getReader(): { read: () => Promise<{ done: boolean; value?: Uint8Array }>; cancel: () => Promise<void>; releaseLock: () => void } {
    this.used = true;
    return {
      read: () => this.read(),
      cancel: () => {
        this.ended = true;
        this.queue = [];
        this.cancel();
        return Promise.resolve();
      },
      releaseLock: () => {},
    };
  }

  async all(): Promise<Uint8Array> {
    this.used = true;
    const parts: Uint8Array[] = [];
    for (;;) {
      const { done, value } = await this.read();
      if (done) break;
      if (value) parts.push(value);
    }
    return concat(parts);
  }
}

export class ZwResponse {
  readonly ok: boolean;
  readonly statusText = "";
  readonly redirected = false;
  readonly type = "basic";

  constructor(
    readonly status: number,
    readonly headers: ZwHeaders,
    readonly url: string,
    readonly body: Body,
  ) {
    this.ok = status >= 200 && status < 300;
  }

  get bodyUsed(): boolean {
    return this.body.used;
  }

  async bytes(): Promise<Uint8Array> {
    return this.body.all();
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    const bytes = await this.body.all();
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  }

  async text(): Promise<string> {
    return decoder.decode(await this.body.all());
  }

  async json(): Promise<unknown> {
    return JSON.parse(await this.text());
  }
}

interface AbortLike {
  aborted: boolean;
  addEventListener?: (type: string, fn: () => void) => void;
}

function abortError(): Error {
  const error = new Error("The operation was aborted.");
  error.name = "AbortError";
  return error;
}

interface FetchInit {
  method?: string;
  headers?: unknown;
  body?: unknown;
  signal?: AbortLike | null;
}

/** ブラウザの fetch と同じ形。応答のヘッダーが届いたら解決し、本文は届いた分ずつ読める。 */
export function zwFetch(input: string | { url: string }, init: FetchInit = {}): Promise<ZwResponse> {
  const url = typeof input === "string" ? input : String(input.url);
  const method = (init.method ?? "GET").toUpperCase();
  const signal = init.signal ?? null;
  if (signal?.aborted) return Promise.reject(abortError());
  const id = host.netOpen(method, url, headerLines(init.headers), bytesOf(init.body));
  if (id === 0) {
    return Promise.reject(new TypeError(`fetch: ${url} を開けない（net が無効か、接続の数が上限）`));
  }
  return new Promise((resolve, reject) => {
    let body: Body | null = null;
    const stop = () => closeConn(id);
    signal?.addEventListener?.("abort", () => {
      stop();
      if (body) body.fail(abortError());
      else reject(abortError());
    });
    live.set(id, (piece) => {
      switch (piece.kind) {
        case HEAD: {
          const text = decoder.decode(piece.body.subarray(2));
          const headers = new ZwHeaders();
          for (const line of text.split("\r\n")) {
            const at = line.indexOf(": ");
            if (at > 0) headers.append(line.slice(0, at), line.slice(at + 2));
          }
          body = new Body(stop);
          resolve(new ZwResponse(u16(piece.body), headers, url, body));
          break;
        }
        case DATA:
          // 本体は poll のたびに新しい配列を渡すので、そのまま持っていてよい。
          body?.push(piece.body.slice());
          break;
        case END:
          live.delete(id);
          body?.end();
          break;
        default: {
          live.delete(id);
          const code = piece.body.length >= 4 ? new DataView(piece.body.buffer, piece.body.byteOffset).getInt32(0) : -2;
          const error = new TypeError(`fetch: ${url} の通信に失敗した（${code}）`);
          if (body) body.fail(error);
          else reject(error);
        }
      }
    });
  });
}

type Listener = (event: Record<string, unknown>) => void;

/** ブラウザの WebSocket と同じ形。三つ目の引数で `{ headers }` を渡せる（User-Agent・Origin など）。
 *  binaryType は "arraybuffer" だけ。 */
export class ZwWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSING = 2;
  readonly CLOSED = 3;

  readonly url: string;
  readyState = 0;
  protocol = "";
  extensions = "";
  bufferedAmount = 0;
  binaryType = "arraybuffer";
  onopen: Listener | null = null;
  onmessage: Listener | null = null;
  onerror: Listener | null = null;
  onclose: Listener | null = null;
  private listeners = new Map<string, Listener[]>();
  private id: number;

  constructor(url: string, protocols?: string | string[], options?: { headers?: unknown }) {
    this.url = url;
    const list = protocols === undefined ? [] : Array.isArray(protocols) ? protocols : [protocols];
    this.id = host.netWs(url, list.join(", "), headerLines(options?.headers));
    if (this.id === 0) {
      setTimeout(() => {
        this.readyState = ZwWebSocket.CLOSED;
        this.emit("error", {});
        this.emit("close", { code: 1006, reason: "net が無効か、接続の数が上限", wasClean: false });
      }, 0);
      return;
    }
    live.set(this.id, (piece) => this.take(piece));
  }

  private take(piece: Piece): void {
    switch (piece.kind) {
      case HEAD: {
        const text = decoder.decode(piece.body.subarray(2));
        for (const line of text.split("\r\n")) {
          const at = line.indexOf(": ");
          if (at > 0 && line.slice(0, at).toLowerCase() === "sec-websocket-protocol") this.protocol = line.slice(at + 2);
        }
        this.readyState = ZwWebSocket.OPEN;
        this.emit("open", {});
        break;
      }
      case TEXT:
        this.emit("message", { data: decoder.decode(piece.body) });
        break;
      case DATA: {
        const copy = piece.body.slice();
        this.emit("message", { data: copy.buffer });
        break;
      }
      case END:
        live.delete(this.id);
        this.readyState = ZwWebSocket.CLOSED;
        this.emit("close", {
          code: u16(piece.body),
          reason: decoder.decode(piece.body.subarray(2)),
          wasClean: u16(piece.body) !== 1006,
        });
        break;
      case FAIL:
      default:
        live.delete(this.id);
        this.readyState = ZwWebSocket.CLOSED;
        this.emit("error", {});
        this.emit("close", { code: 1006, reason: "", wasClean: false });
    }
  }

  private emit(type: string, fields: Record<string, unknown>): void {
    const event = { type, target: this, ...fields };
    const handler = (this as unknown as Record<string, Listener | null>)[`on${type}`];
    if (handler) handler.call(this, event);
    for (const fn of this.listeners.get(type) ?? []) fn.call(this, event);
  }

  addEventListener(type: string, fn: Listener): void {
    const list = this.listeners.get(type) ?? [];
    list.push(fn);
    this.listeners.set(type, list);
  }

  removeEventListener(type: string, fn: Listener): void {
    this.listeners.set(
      type,
      (this.listeners.get(type) ?? []).filter((f) => f !== fn),
    );
  }

  send(data: unknown): void {
    if (this.readyState !== ZwWebSocket.OPEN) {
      throw new Error("WebSocket は開いていない");
    }
    const text = typeof data === "string";
    host.netSend(this.id, text ? SEND_TEXT : SEND_BINARY, bytesOf(data) ?? new Uint8Array());
  }

  close(code = 1000, reason = ""): void {
    if (this.readyState >= ZwWebSocket.CLOSING) return;
    if (this.readyState === ZwWebSocket.CONNECTING) {
      // まだ開いていなければ、そのまま切る。
      closeConn(this.id);
      this.readyState = ZwWebSocket.CLOSED;
      setTimeout(() => this.emit("close", { code: 1006, reason: "", wasClean: false }), 0);
      return;
    }
    this.readyState = ZwWebSocket.CLOSING;
    const reasonBytes = encoder.encode(reason);
    const payload = new Uint8Array(2 + reasonBytes.length);
    payload[0] = (code >> 8) & 0xff;
    payload[1] = code & 0xff;
    payload.set(reasonBytes, 2);
    host.netSend(this.id, SEND_CLOSE, payload);
  }
}

class ZwAbortSignal {
  aborted = false;
  reason: unknown = undefined;
  onabort: (() => void) | null = null;
  private listeners: (() => void)[] = [];

  addEventListener(type: string, fn: () => void): void {
    if (type === "abort") this.listeners.push(fn);
  }

  removeEventListener(type: string, fn: () => void): void {
    if (type === "abort") this.listeners = this.listeners.filter((f) => f !== fn);
  }

  throwIfAborted(): void {
    if (this.aborted) throw abortError();
  }

  fire(reason: unknown): void {
    if (this.aborted) return;
    this.aborted = true;
    this.reason = reason;
    this.onabort?.();
    for (const fn of this.listeners) fn();
  }
}

class ZwAbortController {
  readonly signal = new ZwAbortSignal();

  abort(reason?: unknown): void {
    this.signal.fire(reason ?? abortError());
  }
}

export function installNet(): void {
  const g = globalThis as unknown as Record<string, unknown>;
  g.fetch = zwFetch;
  g.Headers = ZwHeaders;
  g.Response = ZwResponse;
  g.WebSocket = ZwWebSocket;
  if (!g.AbortController) {
    g.AbortController = ZwAbortController;
    g.AbortSignal = ZwAbortSignal;
  }
}
