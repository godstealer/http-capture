import { fileURLToPath } from 'node:url';
import { mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

mkdirSync(new URL('../.local/', import.meta.url), { recursive: true });
for (const script of ['test-response.mjs', 'test-brotli.mjs', 'test-zstd.cjs', 'test-response-worker.cjs', 'test-curl.mjs', 'test-import-curl.mjs', 'test-sessions.cjs', 'test-request-runs.mjs', 'test-i18n.cjs']) {
  const result = spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url))], { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
