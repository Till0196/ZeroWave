#!/usr/bin/env node
// 指定したリリースの実行環境を SHA-256 で確かめて SDK に置く。
import { createHash, randomUUID } from "node:crypto";
import { rename, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

export async function fetchRuntime(tag, base, destination) {
  if (!/^sdk-v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag)) {
    throw new Error("リリースタグを指定してください（例: sdk-v0.1.0）");
  }
  const root = `${base}/${tag}`;
  async function get(name) {
    const response = await fetch(`${root}/${name}`, { signal: AbortSignal.timeout(30_000) });
    if (!response.ok) throw new Error(`${name}: HTTP ${response.status}（${tag} に実行環境があるか確認してください）`);
    return Buffer.from(await response.arrayBuffer());
  }
  const [wasm, sums] = await Promise.all([get("zwjs.wasm"), get("zwjs.wasm.sha256")]);
  const expected = /^([a-f0-9]{64}) [ *]zwjs\.wasm\s*$/i.exec(sums.toString("utf8"))?.[1];
  const actual = createHash("sha256").update(wasm).digest("hex");
  if (!expected || actual !== expected.toLowerCase()) throw new Error("zwjs.wasm の SHA-256 が一致しません");
  if (!wasm.subarray(0, 4).equals(Buffer.from([0, 97, 115, 109]))) throw new Error("zwjs.wasm が WebAssembly ではありません");
  const temporary = `${destination}.${randomUUID()}.tmp`;
  try {
    await writeFile(temporary, wasm, { flag: "wx" });
    await rename(temporary, destination);
  } finally {
    await rm(temporary, { force: true });
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const [tag, ...rest] = process.argv.slice(2);
  if (rest.length || !tag) {
    console.error("使い方: node sdk/ts/bin/fetch-runtime.mjs <リリースタグ>");
    process.exitCode = 2;
  } else {
    const destination = fileURLToPath(new URL("../zwjs.wasm", import.meta.url));
    try {
      await fetchRuntime(tag, "https://github.com/Till0196/ZeroWave/releases/download", destination);
      console.log(`${tag} の zwjs.wasm を取得しました: ${destination}`);
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
