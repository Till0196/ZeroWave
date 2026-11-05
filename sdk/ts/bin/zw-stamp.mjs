#!/usr/bin/env node
// プラグインの wasm を作る。JavaScript の実行環境（zwjs.wasm）に、スクリプト（カスタムセクション
// `zw_script`）とマニフェスト（`zw_manifest`）を足す。name.wasm 一つで配置できる。
// マニフェストは package.json から：name（"zerowave-" は除く）、version、zerowave.hooks（無ければ ["layer"]）、
// zerowave.layer（どのレイヤーか・何が必要か。無ければ字幕の上）。
//
//   zw-stamp dist/index.js -o dist/name.wasm [package.json]
//   zw-stamp dist/name.wasm [package.json]   （wasm にマニフェストだけを足す）

import { existsSync, readFileSync, writeFileSync } from "node:fs";

const args = process.argv.slice(2);
const at = args.indexOf("-o");
const out = at >= 0 ? args.splice(at, 2)[1] : undefined;
const [input, packagePath = "package.json"] = args;
if (!input || (at >= 0 && !out)) {
  console.error("zw-stamp <script.js> -o <name.wasm> [package.json]\nzw-stamp <name.wasm> [package.json]");
  process.exit(2);
}
const pkg = JSON.parse(readFileSync(packagePath, "utf8"));
const zw = pkg.zerowave ?? {};
const manifest = {
  abi: 1,
  name: zw.name ?? String(pkg.name).replace(/^zerowave-/, ""),
  version: pkg.version ?? "",
  hooks: zw.hooks ?? ["layer"],
  layer: zw.layer ?? { above: "captions" },
};

function leb(n) {
  const out = [];
  do {
    let byte = n & 0x7f;
    n >>>= 7;
    if (n !== 0) byte |= 0x80;
    out.push(byte);
  } while (n !== 0);
  return Buffer.from(out);
}

function section(name, content) {
  const named = Buffer.from(name);
  const body = Buffer.concat([leb(named.length), named, content]);
  return Buffer.concat([Buffer.from([0]), leb(body.length), body]);
}

const script = out !== undefined;
const base = script ? new URL("../zwjs.wasm", import.meta.url) : input;
if (script && !existsSync(base)) {
  console.error("zwjs.wasm がありません。node sdk/ts/bin/fetch-runtime.mjs <SDKリリースタグ> で取得してください");
  process.exit(1);
}
const wasm = readFileSync(base);
if (wasm.subarray(0, 4).toString("binary") !== "\0asm") {
  console.error(`${base}: wasm ではない`);
  process.exit(1);
}
const sections = [section("zw_manifest", Buffer.from(JSON.stringify(manifest)))];
if (script) sections.push(section("zw_script", readFileSync(input)));
const target = out ?? input;
writeFileSync(target, Buffer.concat([wasm, ...sections]));
console.log(`${target}: ${JSON.stringify(manifest)}`);
