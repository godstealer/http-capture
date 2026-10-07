import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'node:fs';
import http from 'node:http';
import { fileURLToPath } from 'node:url';
const controlTokenPath = fileURLToPath(new URL('../../.local/capture/control.token', import.meta.url));

export default defineConfig({
  // Keep the WASM URL relative to the package during development.
  optimizeDeps: { exclude: ['brotli-wasm'] },
  plugins: [react(), {
    name: 'local-capture-control',
    configureServer(server) {
      server.middlewares.use('/__capture', (req, res) => {
        const host = req.headers.host;
        const allowed = new Set(['127.0.0.1:1420', 'localhost:1420']);
        // Cross-site scripts cannot attach this header without a CORS preflight.
        // Host and Origin checks additionally prevent DNS rebinding / dev proxy abuse.
        if (!host || !allowed.has(host) || req.headers['x-capture-ui'] !== '1' ||
            (req.headers.origin && req.headers.origin !== `http://${host}`)) {
          res.writeHead(403).end('Forbidden'); return;
        }
        let token: string;
        try { token = readFileSync(controlTokenPath, 'utf8').trim(); }
        catch { res.writeHead(503).end('抓包服务未启动，请运行 npm run capture'); return; }
        const upstream = http.request({ hostname: '127.0.0.1', port: 1421, path: req.url,
          method: req.method, headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' } }, response => {
          res.writeHead(response.statusCode ?? 502, { 'content-type': response.headers['content-type'] ?? 'application/json', 'cache-control': 'no-store' });
          response.pipe(res);
        });
        upstream.on('error', () => { if (!res.headersSent) res.writeHead(503); res.end('抓包服务未连接'); });
        upstream.setTimeout(360000, () => upstream.destroy(new Error('timeout')));
        req.pipe(upstream);
      });
    },
  }],
  server: { port: 1420, strictPort: true },
  clearScreen: false,
});
