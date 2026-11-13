// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// fetch・WebSocket を本体の待たない通信（zw_net_v1 の open・ws・poll・send・close）の上に。
// 繋がるのは、マニフェストの config で `net` を宣言し、ユーザーが許可したときだけ。
// LAN などの内部ネットワークへは `net_local` も要る。
// 届いた物は本体が `__zwTick` を呼ぶたびに `pollNet` で受け取り、Promise やイベントに渡す。

import { host } from "./host";

/** 記録の種類（zwsdk::net::conn::kind）。 */
const FAIL = 0;
const HEAD = 1;
const DATA = 2;
const TEXT = 3;
const END = 4;

/** send の種類（zwsdk::net::conn::send）。 */
const SEND_TEXT = 1;
const SEND_BINARY = 2;
const SEND_CLOSE = 8;

type Arrived =
  | { kind: typeof HEAD; status: number; headers: [string, string][] }
  | { kind: typeof DATA; bytes: Uint8Array }
  | { kind: typeof TEXT; text: string }
  | { kind: typeof END; code: number; reason: string }
  | { kind: typeof FAIL; code: number };

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function decodeRecords(bytes: Uint8Array): Arrived[] {
  const out: Arrived[] = [];
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let at = 0;
  while (at + 5 <= bytes.length) {
    const kind = bytes[at];
    const len = view.getUint32(at + 1);
    const start = at + 5;
    if (start + len > bytes.length) break;
    const body = bytes.subarray(start, start + len);
    at = start + len;
    const u16 = len >= 2 ? view.getUint16(start) : 0;
    const rest = () => decoder.decode(body.subarray(Math.min(2, len)));
    switch (kind) {
      case HEAD: {
        const headers: [string, string][] = [];
        for (const line of rest().split("\r\n")) {
          const colon = line.indexOf(": ");
          if (colon > 0) headers.push([line.slice(0, colon), line.slice(colon + 2)]);
        }
        out.push({ kind: HEAD, status: u16, headers });
        break;
      }
      case DATA:
        out.push({ kind: DATA, bytes: body.slice() });
        break;
      case TEXT:
        out.push({ kind: TEXT, text: decoder.decode(body) });
        break;
      case END:
        out.push({ kind: END, code: u16, reason: rest() });
        break;
      default:
        out.push({ kind: FAIL, code: len >= 4 ? view.getInt32(start) : -2 });
    }
  }
  return out;
}

/** 失敗の番号（zwsdk::net::Failure::code）の説明。 */
function failure(code: number): string {
  switch (code) {
    case -1:
      return "要求が正しくない";
    case -2:
      return "切れた";
    case -3:
      return "応答が無い";
    case -400:
      return "名前を引けない";
    case -500:
      return "TLS で接続できない";
    default:
      return "接続できない";
  }
}

/** 開いている接続。届いた物は `pollNet` が渡す。 */
const open = new Map<number, (arrived: Arrived) => void>();

function forget(handle: number): void {
  if (open.delete(handle)) host.netClose(handle);
}

/** 届いた物をそれぞれの接続へ。本体が `__zwTick` を呼ぶたびに。 */
export function pollNet(): void {
  for (const [handle, take] of [...open]) {
    const bytes = host.netPoll(handle);
    if (!bytes) continue;
    for (const arrived of decodeRecords(bytes)) {
      try {
        take(arrived);
      } catch (error) {
        console.log(`net: ${String(error)}`);
      }
      if (arrived.kind === END || arrived.kind === FAIL) {
        forget(handle);
        break;
      }
    }
  }
}

type BodyInit = string | ArrayBuffer | ArrayBufferView | URLSearchParamsLike;

interface URLSearchParamsLike {
  toString(): string;
  append(name: string, value: string): void;
}

function bytesOf(body: ArrayBuffer | ArrayBufferView | string): Uint8Array {
  if (typeof body === "string") return encoder.encode(body);
  if (body instanceof ArrayBuffer) return new Uint8Array(body);
  return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
}

export type HeadersInit = Headers | [string, string][] | Record<string, string>;

/** 名前は小文字で持つ。 */
export class Headers {
  private all: [string, string][] = [];

  constructor(init?: HeadersInit) {
    if (!init) return;
    if (init instanceof Headers) {
      init.forEach((value, name) => this.append(name, value));
    } else if (Array.isArray(init)) {
      for (const [name, value] of init) this.append(name, value);
    } else {
      for (const name of Object.keys(init)) this.append(name, init[name]);
    }
  }

  append(name: string, value: string): void {
    this.all.push([String(name).toLowerCase(), String(value).trim()]);
  }

  set(name: string, value: string): void {
    this.delete(name);
    this.append(name, value);
  }

  delete(name: string): void {
    const key = String(name).toLowerCase();
    this.all = this.all.filter(([n]) => n !== key);
  }

  get(name: string): string | null {
    const key = String(name).toLowerCase();
    const found = this.all.filter(([n]) => n === key).map(([, v]) => v);
    return found.length > 0 ? found.join(", ") : null;
  }

  has(name: string): boolean {
    return this.get(name) !== null;
  }

  forEach(fn: (value: string, name: string, headers: Headers) => void): void {
    for (const [name, value] of this.entries()) fn(value, name, this);
  }

  entries(): IterableIterator<[string, string]> {
    const names = [...new Set(this.all.map(([n]) => n))].sort();
    return names.map((n): [string, string] => [n, this.get(n)!])[Symbol.iterator]();
  }

  keys(): IterableIterator<string> {
    return [...this.entries()].map(([n]) => n)[Symbol.iterator]();
  }

  values(): IterableIterator<string> {
    return [...this.entries()].map(([, v]) => v)[Symbol.iterator]();
  }

  [Symbol.iterator](): IterableIterator<[string, string]> {
    return this.entries();
  }

  /** `名前: 値` の行（本体へ渡す形）。 */
  lines(): string {
    return this.all.map(([n, v]) => `${n}: ${v}\n`).join("");
  }
}

function abortError(): Error {
  const error = new Error("止めた");
  error.name = "AbortError";
  return error;
}

export class AbortSignal {
  aborted = false;
  reason: unknown = undefined;
  onabort: ((event: { type: string }) => void) | null = null;
  private listeners: (() => void)[] = [];

  addEventListener(type: string, fn: () => void): void {
    if (type === "abort") this.listeners.push(fn);
  }

  removeEventListener(type: string, fn: () => void): void {
    if (type === "abort") this.listeners = this.listeners.filter((f) => f !== fn);
  }

  throwIfAborted(): void {
    if (this.aborted) throw this.reason;
  }

  /** AbortController から。 */
  fire(reason: unknown): void {
    if (this.aborted) return;
    this.aborted = true;
    this.reason = reason === undefined ? abortError() : reason;
    this.onabort?.({ type: "abort" });
    for (const fn of this.listeners) fn();
  }

  static abort(reason?: unknown): AbortSignal {
    const signal = new AbortSignal();
    signal.fire(reason);
    return signal;
  }

  /** タイマーは `__zwTick` の中で進むので、期限はその刻みで見る。 */
  static timeout(ms: number): AbortSignal {
    const signal = new AbortSignal();
    setTimeout(() => {
      const error = new Error("応答が無い");
      error.name = "TimeoutError";
      signal.fire(error);
    }, ms);
    return signal;
  }
}

export class AbortController {
  readonly signal = new AbortSignal();

  abort(reason?: unknown): void {
    this.signal.fire(reason);
  }
}

export class Response {
  readonly ok: boolean;
  readonly statusText = "";
  readonly type = "basic";
  bodyUsed = false;

  constructor(
    private readonly bytesPromise: Promise<Uint8Array>,
    readonly status: number,
    readonly headers: Headers,
    readonly url: string,
    readonly redirected: boolean,
  ) {
    this.ok = status >= 200 && status < 300;
    // 読まれなくても未処理の拒否として出さない。
    bytesPromise.catch(() => {});
  }

  bytes(): Promise<Uint8Array> {
    if (this.bodyUsed) return Promise.reject(new TypeError("本文はもう読んだ"));
    this.bodyUsed = true;
    return this.bytesPromise;
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    const bytes = await this.bytes();
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  }

  async text(): Promise<string> {
    return decoder.decode(await this.bytes());
  }

  async json(): Promise<unknown> {
    return JSON.parse(await this.text());
  }
}

export interface RequestInit {
  method?: string;
  headers?: HeadersInit;
  body?: BodyInit | null;
  redirect?: "follow" | "manual" | "error";
  signal?: AbortSignal | null;
}

const REDIRECTS_MOST = 20;

/** `location` を今の URL に対して解く。 */
export function resolveUrl(base: string, location: string): string {
  if (/^[a-z][a-z0-9+.-]*:/i.test(location)) return location;
  const origin = /^([a-z][a-z0-9+.-]*:)\/\/[^/?#]*/i.exec(base);
  if (!origin) return location;
  if (location.startsWith("//")) return origin[1] + location;
  if (location.startsWith("/")) return origin[0] + location;
  const path = base.slice(origin[0].length).split(/[?#]/)[0] || "/";
  if (location.startsWith("?")) return origin[0] + path + location;
  if (location.startsWith("#")) return base.split("#")[0] + location;
  const parts = path.slice(0, path.lastIndexOf("/") + 1).concat(location.split(/[?#]/)[0]).split("/");
  const out: string[] = [];
  for (const part of parts.slice(1)) {
    if (part === "..") out.pop();
    else if (part !== ".") out.push(part);
  }
  if (parts[parts.length - 1] === "." || parts[parts.length - 1] === "..") out.push("");
  const tail = location.slice(location.split(/[?#]/)[0].length);
  return `${origin[0]}/${out.join("/")}${tail}`;
}

function urlOf(input: string | { url: string } | { href: string }): string {
  if (typeof input === "string") return input;
  if ("url" in input) return input.url;
  if ("href" in input) return input.href;
  return String(input);
}

export function fetch(input: string | { url: string } | { href: string }, init: RequestInit = {}): Promise<Response> {
  return new Promise((resolve, reject) => {
    const signal = init.signal ?? null;
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    let method = (init.method ?? "GET").toUpperCase();
    const headers = new Headers(init.headers);
    let body: Uint8Array | null = null;
    if (init.body !== undefined && init.body !== null) {
      const given = init.body;
      if (typeof given === "string" || given instanceof ArrayBuffer || ArrayBuffer.isView(given)) {
        body = bytesOf(given);
        if (typeof given === "string" && !headers.has("content-type")) {
          headers.set("content-type", "text/plain;charset=UTF-8");
        }
      } else {
        body = encoder.encode(given.toString());
        if (!headers.has("content-type")) {
          headers.set("content-type", "application/x-www-form-urlencoded;charset=UTF-8");
        }
      }
    }
    let url = urlOf(input);
    let redirects = 0;
    let handle = 0;
    let settled = false;
    // 本文を集めている間（応答を返した後）。
    let receive: { chunks: Uint8Array[]; done: (bytes: Uint8Array) => void; fail: (error: unknown) => void } | null =
      null;

    const stop = () => {
      const reason = signal?.reason ?? abortError();
      forget(handle);
      if (receive) receive.fail(reason);
      else if (!settled) {
        settled = true;
        reject(reason);
      }
    };
    signal?.addEventListener("abort", stop);
    const finish = () => signal?.removeEventListener("abort", stop);

    const begin = () => {
      handle = host.netOpen(method, url, headers.lines(), body);
      if (handle === 0) {
        settled = true;
        finish();
        reject(new TypeError(`${url}: 接続できない（net が有効でないか、接続の数の上限）`));
        return;
      }
      open.set(handle, take);
    };

    const take = (arrived: Arrived) => {
      switch (arrived.kind) {
        case HEAD: {
          const answer = new Headers(arrived.headers);
          const location = answer.get("location");
          const redirect = init.redirect ?? "follow";
          if (location !== null && [301, 302, 303, 307, 308].includes(arrived.status) && redirect !== "manual") {
            forget(handle);
            if (redirect === "error" || ++redirects > REDIRECTS_MOST) {
              settled = true;
              finish();
              reject(new TypeError(`${url}: 転送が多すぎる`));
              return;
            }
            // 303 と、POST の 301・302 は GET に（ブラウザーと同じ）。
            if (arrived.status === 303 || ((arrived.status === 301 || arrived.status === 302) && method === "POST")) {
              if (method !== "HEAD") method = "GET";
              body = null;
              headers.delete("content-type");
              headers.delete("content-length");
            }
            url = resolveUrl(url, location);
            begin();
            return;
          }
          const bytes = new Promise<Uint8Array>((done, fail) => {
            receive = { chunks: [], done, fail };
          });
          settled = true;
          resolve(new Response(bytes, arrived.status, answer, url, redirects > 0));
          return;
        }
        case DATA:
          receive?.chunks.push(arrived.bytes);
          return;
        case TEXT:
          receive?.chunks.push(encoder.encode(arrived.text));
          return;
        case END: {
          finish();
          if (!receive) return;
          const size = receive.chunks.reduce((n, c) => n + c.length, 0);
          const all = new Uint8Array(size);
          let at = 0;
          for (const chunk of receive.chunks) {
            all.set(chunk, at);
            at += chunk.length;
          }
          receive.done(all);
          return;
        }
        case FAIL: {
          finish();
          const error = new TypeError(`${url}: ${failure(arrived.code)}`);
          if (receive) receive.fail(error);
          else if (!settled) {
            settled = true;
            reject(error);
          }
        }
      }
    };

    begin();
  });
}

interface WsEvent {
  type: string;
  target: WebSocket;
  data?: string | ArrayBuffer;
  code?: number;
  reason?: string;
  wasClean?: boolean;
}

type WsListener = (event: WsEvent) => void;

export class WebSocket {
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
  /** 本体から来るバイナリは ArrayBuffer だけ。 */
  binaryType: "arraybuffer" | "blob" = "arraybuffer";
  onopen: WsListener | null = null;
  onmessage: WsListener | null = null;
  onerror: WsListener | null = null;
  onclose: WsListener | null = null;
  private listeners = new Map<string, WsListener[]>();
  private handle: number;

  constructor(url: string | { href: string }, protocols?: string | string[]) {
    this.url = typeof url === "string" ? url : url.href;
    const list = protocols === undefined ? [] : Array.isArray(protocols) ? protocols : [protocols];
    this.handle = host.netWebSocket(this.url, list.join(", "), "");
    if (this.handle === 0) {
      // ブラウザーと同じく、失敗はイベントで（作った直後に付けたハンドラーへ届くように次の刻みで）。
      setTimeout(() => this.ended(1006, "", false, true), 0);
      return;
    }
    open.set(this.handle, (arrived) => this.take(arrived));
  }

  addEventListener(type: string, fn: WsListener): void {
    const list = this.listeners.get(type) ?? [];
    list.push(fn);
    this.listeners.set(type, list);
  }

  removeEventListener(type: string, fn: WsListener): void {
    this.listeners.set(
      type,
      (this.listeners.get(type) ?? []).filter((f) => f !== fn),
    );
  }

  private emit(event: WsEvent): void {
    const own = (this as unknown as Record<string, WsListener | null>)[`on${event.type}`];
    if (own) own.call(this, event);
    for (const fn of this.listeners.get(event.type) ?? []) fn.call(this, event);
  }

  send(data: string | ArrayBuffer | ArrayBufferView): void {
    if (this.readyState === WebSocket.CONNECTING) {
      throw new Error("InvalidStateError: まだ繋がっていない");
    }
    if (this.readyState !== WebSocket.OPEN) return;
    const text = typeof data === "string";
    const bytes = bytesOf(data);
    host.netSend(this.handle, text ? SEND_TEXT : SEND_BINARY, bytes);
  }

  close(code = 1000, reason = ""): void {
    if (this.readyState === WebSocket.CLOSING || this.readyState === WebSocket.CLOSED) return;
    if (this.readyState === WebSocket.CONNECTING) {
      forget(this.handle);
      this.ended(1006, "", false, true);
      return;
    }
    this.readyState = WebSocket.CLOSING;
    const words = encoder.encode(reason);
    const payload = new Uint8Array(2 + words.length);
    new DataView(payload.buffer).setUint16(0, code);
    payload.set(words, 2);
    host.netSend(this.handle, SEND_CLOSE, payload);
  }

  private ended(code: number, reason: string, wasClean: boolean, error: boolean): void {
    if (this.readyState === WebSocket.CLOSED) return;
    this.readyState = WebSocket.CLOSED;
    if (error) this.emit({ type: "error", target: this });
    this.emit({ type: "close", target: this, code, reason, wasClean });
  }

  private take(arrived: Arrived): void {
    switch (arrived.kind) {
      case HEAD:
        this.readyState = WebSocket.OPEN;
        this.protocol = new Headers(arrived.headers).get("sec-websocket-protocol") ?? "";
        this.emit({ type: "open", target: this });
        return;
      case TEXT:
        this.emit({ type: "message", target: this, data: arrived.text });
        return;
      case DATA: {
        const b = arrived.bytes;
        this.emit({ type: "message", target: this, data: b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer });
        return;
      }
      case END:
        this.ended(arrived.code === 0 ? 1005 : arrived.code, arrived.reason, true, false);
        return;
      case FAIL:
        this.ended(1006, "", false, true);
    }
  }
}

/** まだ無い物だけ globalThis に置く。 */
export function installNet(): void {
  const g = globalThis as unknown as Record<string, unknown>;
  g.fetch ??= fetch;
  g.Headers ??= Headers;
  g.Response ??= Response;
  g.WebSocket ??= WebSocket;
  g.AbortController ??= AbortController;
  g.AbortSignal ??= AbortSignal;
}
