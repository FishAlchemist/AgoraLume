import react from '@vitejs/plugin-react';
import { defineConfig, loadEnv } from 'vite';

// Static SPA build. Output in dist/ is plain HTML/JS/CSS and can be hosted
// anywhere; at runtime it talks to a backend chosen by VITE_API_BASE_URL, the
// serving origin (VITE_SAME_ORIGIN=1, the bundle), or the in-browser mock.
//
// The dev proxy below reproduces the bundle's single-origin shape while keeping
// hot reload: `pnpm dev:proxy` serves the SPA in same-origin mode and forwards
// the backend's top-level route prefixes to it, so the whole app is reachable
// on Vite's one port. Client routes live under the URL hash (HashRouter), so
// proxying these real paths never shadows SPA navigation.
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

// This config runs under Node; declare the globals we read so the app's
// browser-only tsconfig type-checks it without pulling in @types/node.
declare const process: { env: Record<string, string | undefined>; cwd(): string };

export default defineConfig(({ mode }) => {
  // Merge .env files and real env vars, so the proxy/port settings work whether
  // they live in frontend/.env or the shell.
  const env = loadEnv(mode, process.cwd(), '');
  // Backend the dev server proxies API routes to (default the local backend).
  const proxyTarget = env.VITE_PROXY_TARGET || 'http://127.0.0.1:8080';
  // Port Vite exposes; an explicit value is honoured strictly (a clash fails
  // loudly instead of hopping to another port). Unset uses Vite's default.
  const devPort = env.VITE_DEV_PORT ? Number(env.VITE_DEV_PORT) : undefined;
  // Extra hostnames the dev server answers to, beyond localhost — needed when
  // exposing it through a tunnel (e.g. Cloudflare's *.trycloudflare.com).
  // Comma-separated; a leading dot matches subdomains. Kept in the gitignored
  // frontend/.env so personal tunnel hosts aren't tracked. Unset = Vite default.
  const allowedHosts = (env.VITE_ALLOWED_HOSTS || '')
    .split(',')
    .map((host) => host.trim())
    .filter(Boolean);

  return {
    plugins: [react()],
    server: {
      port: devPort,
      // Only override when configured, so the default stays Vite's own list.
      ...(allowedHosts.length > 0 ? { allowedHosts } : {}),
      strictPort: devPort !== undefined,
      // changeOrigin rewrites the Host header to the target; SSE (/groups/:id/
      // stream) streams straight through since the proxy doesn't buffer plain
      // HTTP responses.
      proxy: Object.fromEntries(
        API_PREFIXES.map((path) => [path, { target: proxyTarget, changeOrigin: true }]),
      ),
    },
  };
});
