// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// メニュー・ページ・下のパネル・通知（ui hook）を TypeScript で。型は `zwsdk::ui` の JSON と同じ。
//
//   import { ui } from "zwsdk";
//   ui({
//     page: (page, ctx) => ({ nodes: [{ kind: "note", text: ctx.program?.title ?? "" }] }),
//     event: (page, ctx, event) => ({}),
//   });
//
// 下のパネル（パネルウィンドウのタブ）は package.json の `zerowave.ui.panels` で宣言し、
// `page` と `event` がパネルの識別子で呼ばれる。設定のページも同じ二つで、識別子はページの名前。
// 本体は呼ぶたびに作り直したウィジェットの並びを表示するので、状態は変数か設定に持つ。
// レイヤーとは別のインスタンスで動くので、レイヤーと共有するデータは設定（`config`）に置く。
// 呼び出しは待たずに返す物で、`fetch` や `setTimeout` の続きはここでは実行されない。
// スクリプトの一番上のコードはレイヤーと ui の両方のインスタンスで実行される。通信は `onNew` の中で始める
// （ui のインスタンスで開いた接続は、続きが実行されないまま残る）。

import { host } from "./host";

export type Tone = "plain" | "muted" | "accent" | "good" | "warn" | "bad";

export interface Program {
  /** ARIB のジャンル（大分類、中分類）。 */
  genres?: [number, number][];
  network?: number;
  service?: number;
  /** 設定の `[[network]]` の名前。どれにも入らなければ空。 */
  band?: string;
  channel?: string;
  title?: string;
  recording?: boolean;
  structure?: "unknown" | "frame" | "field";
  /** 1970-01-01 0時からの分（日本時間の壁時計）。 */
  minute?: number;
}

export interface Format {
  width: number;
  height: number;
  depth: number;
  interlaced: boolean;
  aspect: [number, number];
  transfer: string;
}

export interface Channel {
  network: number;
  service: number;
  name: string;
}

/** 呼ぶときの本体の様子（`zwsdk::ui::Ctx`）。映像の処理の様子は `video`。 */
export interface Ctx {
  program?: Program | null;
  format?: Format | null;
  video?: Record<string, unknown> | null;
  channels?: Channel[];
  /** `Program.band` の取りうる値。ページのときだけ。 */
  networks?: string[];
}

export type Open = "settings" | { page: string } | { panel: string };

export type MenuItem =
  | { kind: "item"; id: string; label: string; checked?: boolean | null; enabled?: boolean }
  | { kind: "open"; label: string; open: Open }
  | { kind: "separator" };

export interface Button {
  id: string;
  label: string;
  enabled?: boolean;
  primary?: boolean;
  icon?: string;
}

export interface Chip {
  id: string;
  label: string;
  on?: boolean;
}

export interface Badge {
  text: string;
  tone?: Tone;
}

export type Inline =
  | { kind: "text"; text: string; tone?: Tone }
  | { kind: "choice"; id: string; options: [string, string][]; value: string; width?: number }
  | { kind: "input"; id: string; value: string; width?: number; hint?: string }
  /** 押すと `value` にチップの識別子、`removable` の × で `remove:<識別子>`。 */
  | { kind: "chips"; id: string; chips: Chip[]; removable?: boolean }
  | ({ kind: "button" } & Button);

export interface Column {
  label: string;
  width?: number;
  right?: boolean;
}

export type Cell =
  | { kind: "text"; text: string; tone?: Tone; sub?: string }
  | { kind: "tag"; text: string; tone?: Tone }
  | { kind: "chips"; chips: string[]; joint?: string }
  | { kind: "toggle"; id: string; on: boolean }
  | { kind: "choice"; id: string; options: [string, string][]; value: string }
  | { kind: "buttons"; buttons: Button[] };

export interface Row {
  id: string;
  cells: Cell[];
  selected?: boolean;
  dim?: boolean;
}

export type Node =
  | { kind: "section"; label: string }
  | { kind: "note"; text: string; tone?: Tone }
  | { kind: "toggle"; id: string; label: string; on: boolean; indent?: number; enabled?: boolean }
  | { kind: "line"; label?: string; items: Inline[] }
  | { kind: "buttons"; buttons: Button[]; right?: boolean }
  /** 行を押すと `id` のイベントが `value` に行の識別子で届く。 */
  | { kind: "table"; id: string; columns: Column[]; rows: Row[]; empty?: string }
  | { kind: "bar"; label: string; parts: [number, Tone][]; left?: string; right?: string }
  /**
   * `scroll` が真なら、ページの残りの高さを埋めて左右を別々にスクロールし、ページ自体はスクロールしない。
   * `left_foot`・`right_foot` はスクロールせず、それぞれの列の下に固定する。
   */
  | { kind: "columns"; width: number; left: Node[]; right: Node[]; scroll?: boolean; left_foot?: Node[]; right_foot?: Node[] }
  | { kind: "space"; height: number }
  /**
   * ページの残りの高さを埋めて `nodes` だけを別にスクロールし、`foot` はスクロールせず下に固定する。
   * これより上の項目は止まったままで、ページ自体はスクロールしない。ページの最後に置く。
   */
  | { kind: "scroll"; nodes: Node[]; foot?: Node[] }
  /** 選ぶと `id` のイベントが `value` に選んだ項目の識別子で届く。 */
  | { kind: "segments"; id: string; options: [string, string][]; value: string }
  /** `kind = "program"` のキーのカード（下のパネルでは見せるだけ）。 */
  | { kind: "program"; key: string; label?: string; badge?: Badge | null; hint?: string }
  /** `kind = "list"` のキーをタグで並べる項目。 */
  | { kind: "list"; key: string; label?: string; hint?: string }
  /** `kind = "map"` のキーのマップ。 */
  | { kind: "map"; key: string; label?: string; empty?: string }
  /** `program`・`args`・`env` のキーから本体が組み立てるコマンドライン。 */
  | { kind: "command"; program: string; args?: string; env?: string; label?: string };

export interface Page {
  title?: string;
  size?: [number, number] | null;
  tabs?: [string, string][];
  tab?: string;
  nodes?: Node[];
  status?: string;
  footer?: Button[];
  /** キーに結び付けた項目の変更を下書きにし、フッターに「OK」「キャンセル」「適用」を出す。 */
  confirm?: boolean;
}

/** ウィジェットを操作したとき。`id` はウィジェットの識別子。 */
export interface Event {
  id: string;
  value?: string;
}

export interface PickFile {
  id: string;
  filters?: [string, string[]][];
}

/** `folder` は `kind = "folder"` のキー、`path` はその下の相対パス（`..` は不可）。 */
export type FileOp =
  | { op: "copy"; from: string; folder: string; path: string }
  | { op: "remove"; folder: string; path: string };

export interface Reply {
  /** このページを開く（設定のページの名前）。 */
  open?: string | null;
  close?: boolean;
  /** OS のファイル選択ダイアログ。選ばれたら `id` のイベントが `value` にパスで届く。 */
  pick_file?: PickFile | null;
  /** 結果は `files` のイベント（`value` が空なら完了）。 */
  files?: FileOp[];
}

export interface Notice {
  /** 同じキーの通知は表示中のものを差し替える。空なら毎回新しい。 */
  key?: string;
  title: string;
  details?: string[];
  tone?: Tone;
  progress?: number | null;
  seconds?: number | null;
}

/** ui hook の関数。定義しなかった物は空を返す。 */
export interface Ui {
  /** プラグインのメニュー。 */
  menu?(ctx: Ctx): MenuItem[];
  /** メニューの `item` が選ばれたとき。 */
  pick?(ctx: Ctx, id: string): void;
  /** ページか下のパネルの中身。 */
  page?(page: string, ctx: Ctx): Page;
  /** ページか下のパネルのウィジェットを操作したとき。 */
  event?(page: string, ctx: Ctx, event: Event): Reply | void;
}

let handlers: Ui = {};

/** ui hook の関数を登録する（package.json の `zerowave.hooks` に `"ui"` を書くこと）。 */
export function ui(fns: Ui): void {
  handlers = fns;
}

/** 本体の通知を出す。文字列なら一行目が題名、残りが詳しい内容。 */
export function notice(notice: Notice | string): void {
  if (typeof notice === "string") {
    const [title = "", ...details] = notice.split("\n");
    notice = { title, details };
  }
  host.notice(JSON.stringify(notice));
}

function parse<T>(json: string, fallback: T): T {
  if (!json) return fallback;
  try {
    return JSON.parse(json) as T;
  } catch {
    return fallback;
  }
}

globalThis.__zwUi = (kind: string, page: string, ctxJson: string, extra: string): string => {
  const ctx = parse<Ctx>(ctxJson, {});
  try {
    switch (kind) {
      case "menu":
        return JSON.stringify(handlers.menu?.(ctx) ?? []);
      case "pick":
        handlers.pick?.(ctx, extra);
        return "";
      case "page":
        return JSON.stringify(handlers.page?.(page, ctx) ?? {});
      case "event":
        return JSON.stringify(handlers.event?.(page, ctx, parse<Event>(extra, { id: "" })) ?? {});
    }
  } catch (error) {
    console.log(`ui ${kind}: ${String(error)}`);
  }
  return "";
};
