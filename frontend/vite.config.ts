import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// Static SPA build. Output in dist/ is plain HTML/JS/CSS and can be hosted
// anywhere; at runtime it talks to a backend chosen by VITE_API_BASE_URL, the
// serving origin (VITE_SAME_ORIGIN=1, the bundle), or the in-browser mock.
//
// The dev proxy below reproduces the bundle's single-origin shape while keeping
// hot reload: `pnpm dev:proxy` serves the SPA in same-origin mode and forwards
// the backend's top-level route prefixes to it, so the whole app is reachable
// on Vite's one port. Client routes live under the URL hash (HashRouter), so
// proxying these real paths never shadows SPA navigation. Override the backend
// with VITE_PROXY_TARGET (default the local backend on 8080).
const API_PREFIXES = [
  '/health',
  '/meta',
  '/debug',
  '/organizations',
  '/departments',
  '/personas',
  '/groups',
  '/settings',
];

// This config runs under Node; declare the one global we read so the app's
// browser-only tsconfig type-checks it without pulling in @types/node.
declare const process: { env: Record<string, string | undefined> };

const proxyTarget = process.env.VITE_PROXY_TARGET ?? 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [react()],
  server: {
    // changeOrigin rewrites the Host header to the target; SSE (/groups/:id/
    // stream) streams straight through since the proxy doesn't buffer plain
    // HTTP responses.
    proxy: Object.fromEntries(
      API_PREFIXES.map((path) => [path, { target: proxyTarget, changeOrigin: true }]),
    ),
  },
});
