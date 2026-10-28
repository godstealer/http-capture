import { fileURLToPath } from 'node:url';
import { mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

mkdirSync(new URL('../.local/', import.meta.url), { recursive: true });
for (const script of [
  'test-close-guard.cjs', 'test-management-ui.cjs', 'test-certificates-ui.cjs', 'test-workspace-recovery.cjs', 'test-mcp-ui.cjs', 'test-audit-ui.cjs', 'test-config-migration.cjs', 'test-app-update-state.cjs', 'test-todo-ui.cjs', 'test-response-rewrite.cjs', 'test-response-rewrite-ui.cjs', 'test-response.mjs', 'test-brotli.mjs',
  'test-zstd.cjs', 'test-zstd-stream.cjs', 'test-response-worker.cjs', 'test-sse-live.cjs', 'test-recorded-sse.cjs', 'test-recorded-sse-ui.cjs', 'test-response-body-budget-ui.cjs', 'test-json-fold-ranges.cjs', 'test-curl.mjs', 'test-import-curl.mjs',
  'test-script-output.cjs',
  'test-sessions.cjs', 'test-session-native.cjs', 'test-save-file.cjs', 'test-import-batches.cjs', 'test-header-order-source.cjs',
  'test-header-text.cjs', 'test-cookie-headers.cjs', 'test-cookie-header-ui.cjs', 'test-body-format.cjs', 'test-interception-editor.cjs', 'test-request-body.cjs',
  'test-listener-preferences.cjs', 'test-ui-zoom.cjs', 'test-theme.cjs', 'test-har.cjs', 'test-grpc.cjs', 'test-library.cjs',
  'test-request-runs.mjs', 'test-flow-refresh.cjs', 'test-flow-records.cjs', 'test-flow-history-ui.cjs', 'test-history-actions-ui.cjs', 'test-recorded-body.cjs', 'test-recorded-body-ui.cjs', 'test-grpc-recorded-ranges.cjs', 'test-grpc-recorded-ui.cjs', 'test-traffic-origin.cjs', 'test-code-export.cjs', 'test-code-export-web.cjs', 'test-i18n.cjs',
]) {
  const result = spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url))], { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
