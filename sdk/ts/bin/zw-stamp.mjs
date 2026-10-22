#!/usr/bin/env node
// ビルドした wasm にマニフェストを埋める（カスタムセクション `zw_manifest`）。
// これで name.wasm 一つで配置できる。内容は package.json から：
//   name（"zerowave-" は除く）、version、zerowave.hooks（無ければ ["layer"]）、
//   zerowave.layer（どのレイヤーか・何が必要か。無ければ字幕の上）、zerowave.config（設定）。
//
//   zw-stamp dist/name.wasm [package.json]

import { readFileSync, writeFileSync } from "node:fs";

const [wasmPath, packagePath = "package.json"] = process.argv.slice(2);
if (!wasmPath) {
  console.error("zw-stamp <wasm> [package.json]");
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
// 設定（`[{"key":"net","kind":"bool","default":"false"}]` など）。通信するなら `net` を宣言する。
if (Array.isArray(zw.config)) manifest.config = zw.config;

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

const name = Buffer.from("zw_manifest");
const body = Buffer.concat([leb(name.length), name, Buffer.from(JSON.stringify(manifest))]);
const section = Buffer.concat([Buffer.from([0]), leb(body.length), body]);
const wasm = readFileSync(wasmPath);
if (wasm.subarray(0, 4).toString("binary") !== "\0asm") {
  console.error(`${wasmPath}: wasm ではない`);
  process.exit(1);
}
writeFileSync(wasmPath, Buffer.concat([wasm, section]));
console.log(`${wasmPath}: ${JSON.stringify(manifest)}`);
