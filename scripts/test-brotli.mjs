import assert from 'node:assert/strict';
import fs from 'node:fs';
import { brotliCompressSync } from 'node:zlib';
import ts from 'typescript';
const dependency = new URL('../node_modules/brotli-wasm/index.node.js', import.meta.url).href;
const source = ts.transpileModule(fs.readFileSync(new URL('../apps/frontend/src/decodeBrotli.ts', import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText.replace("'brotli-wasm'", JSON.stringify(dependency));
const { decodeBrotli } = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));
for (const input of [Buffer.alloc(0), Buffer.from('{"message":"你好"}'), Buffer.alloc(200_000, 65)]) {
  const compressed = brotliCompressSync(input);
  assert.deepEqual(Buffer.from(await decodeBrotli(compressed)), input);
  if (input.length) await assert.rejects(() => decodeBrotli(compressed, input.length - 1), /超过/);
  await assert.rejects(() => decodeBrotli(compressed.subarray(0, compressed.length - 1)));
}
await assert.rejects(() => decodeBrotli(Buffer.from([255, 255, 255])));
await assert.rejects(() => decodeBrotli(Buffer.concat([brotliCompressSync(Buffer.from('ok')), Buffer.from([0])])), /多余/);
console.log('Brotli: empty, Unicode, multi-chunk, output limit, truncation, corrupt and trailing data passed.');
