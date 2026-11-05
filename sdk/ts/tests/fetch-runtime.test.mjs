import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fetchRuntime } from "../bin/fetch-runtime.mjs";

test("指定したリリースの wasm を検証し、不一致なら既存のファイルを残す", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "zw-runtime-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const wasm = Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]);
  const digest = createHash("sha256").update(wasm).digest("hex");
  let valid = true;
  const server = createServer((request, response) => {
    if (request.url === "/sdk-v1.2.3/zwjs.wasm") response.end(wasm);
    else if (request.url === "/sdk-v1.2.3/zwjs.wasm.sha256") response.end(`${valid ? digest : "0".repeat(64)}  zwjs.wasm\n`);
    else { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const base = `http://127.0.0.1:${server.address().port}`;
  const destination = join(directory, "zwjs.wasm");
  await fetchRuntime("sdk-v1.2.3", base, destination);
  assert.deepEqual(await readFile(destination), wasm);
  await writeFile(destination, "old");
  valid = false;
  await assert.rejects(fetchRuntime("sdk-v1.2.3", base, destination), /SHA-256/);
  assert.equal(await readFile(destination, "utf8"), "old");
  await assert.rejects(fetchRuntime("latest", base, destination), /リリースタグ/);
});
