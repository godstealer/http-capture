import { fileURLToPath } from 'node:url';
import { mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

mkdirSync(new URL('../.local/', import.meta.url), { recursive: true });
for (const script of ['test-close-guard.cjs', 'test-app-update-state.cjs', 'test-response.mjs', 'test-brotli.mjs', 'test-zstd.cjs', 'test-response-worker.cjs', 'test-curl.mjs', 'test-import-curl.mjs', 'test-sessions.cjs', 'test-save-file.cjs', 'test-import-batches.cjs', 'test-header-order-source.cjs', 'test-header-text.cjs', 'test-har.cjs', 'test-grpc.cjs', 'test-library.cjs', 'test-request-runs.mjs', 'test-flow-refresh.cjs', 'test-i18n.cjs']) {
  const result = spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url))], { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
