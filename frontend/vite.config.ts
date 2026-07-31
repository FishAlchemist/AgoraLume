import react from '@vitejs/plugin-react';
import { defineConfig, loadEnv } from 'vite';

// Static SPA build. Output in dist/ is plain HTML/JS/CSS and can be hosted
// anywhere; at runtime it talks to a backend chosen by VITE_API_BASE_URL, the
// serving origin (VITE_SAME_ORIGIN=1, the bundle), or the in-browser mock.
//
// The dev proxy below reproduces an edge-fronted deployment while keeping hot
// reload: `pnpm dev:proxy` serves the SPA in same-origin mode and forwards
// `/api`-prefixed traffic to the backend, so the whole app is reachable on
// Vite's one port. This is one fixed rule, not a per-resource list that needs
// a new line every time a route is added — the backend itself lives entirely
// under one version prefix (`API_VERSION` in backend/src/routes/mod.rs) and
// is never mounted under `/api` (it's its own API service, not a sub-resource
// of one); `/api` here plays the role an operator's own edge (Nginx, etc.)
// would in a real multi-service deployment. Only two builds ever run through
// this proxy — `dev:proxy`/`dev:single` and `build:proxy`'s output (used by
// `start:single`) — and both set `VITE_API_PREFIX=/api`, so there is no bare,
// unprefixed passthrough here: an edge always requires going through `/api`.
// (`build:bundle`'s output — the real single-binary bundle from
// `scripts/bundle.mjs` — has no edge in front of it at all: the Rust process
// serves its own `/v1beta` routes directly, so it deliberately stays
// bare-root and never transits this proxy.) Client routes live under the URL
// hash (HashRouter), so proxying `/api` never shadows SPA navigation.

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

  // Shared by both the dev server and `vite preview`, so a production build
  // served by preview (`pnpm start:single`) forwards the backend routes exactly
  // like the dev proxy does — the only difference being dev vs. built assets.
  const serverOptions = {
    port: devPort,
    // Only override when configured, so the default stays Vite's own list.
    ...(allowedHosts.length > 0 ? { allowedHosts } : {}),
    strictPort: devPort !== undefined,
    // changeOrigin rewrites the Host header to the target; SSE (/groups/:id/
    // stream) streams straight through since the proxy doesn't buffer plain
    // HTTP responses. The rewrite only ever trims the literal `/api` segment,
    // so it's version-agnostic — a version bump never touches this file.
    proxy: {
      '/api': {
        target: proxyTarget,
        changeOrigin: true,
        rewrite: (path: string) => path.replace(/^\/api/, ''),
      },
    },
  };

  return {
    plugins: [react()],
    server: serverOptions,
    preview: serverOptions,
  };
});
