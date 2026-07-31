import { create } from 'zustand';
import { persist } from 'zustand/middleware';

// Build-time default: honour VITE_API_BASE_URL unless the mock is forced. This
// is only the *initial* value — the user can change it at runtime below, and
// the choice is persisted.
const envUrl = import.meta.env.VITE_API_BASE_URL?.trim();
const forceMock = import.meta.env.VITE_USE_MOCK === '1';
// The single-binary bundle serves the SPA and the API from one origin. Building
// with VITE_SAME_ORIGIN=1 makes the app default to that origin (a real,
// non-empty URL, so every `if (backendUrl)` consumer treats it as a live
// backend) instead of the in-browser mock.
//
// VITE_API_PREFIX adds an edge segment (e.g. `/api`) ahead of that origin —
// set by the two builds that transit the Vite proxy, `dev:proxy` (interactive)
// and `build:proxy` (built assets, used by `start:single`); Vite strips it
// before forwarding (see vite.config.ts), which has no bare passthrough, so
// an edge always requires it. `build:bundle` — the real single-binary bundle
// from `scripts/bundle.mjs`, served by the Rust process directly with no edge
// in front of it — deliberately never sets this, since there's nothing there
// to strip it back off.
const apiPrefix = import.meta.env.VITE_API_PREFIX?.trim() || '';
const sameOrigin =
  import.meta.env.VITE_SAME_ORIGIN === '1' && typeof window !== 'undefined'
    ? `${window.location.origin}${apiPrefix}`
    : null;
const initialBackendUrl = forceMock ? null : envUrl || sameOrigin || null;

interface ConnectionState {
  /** The backend to talk to; `null` means the in-browser mock. */
  backendUrl: string | null;
  /** Sets (or clears, with `null`/empty) the backend URL. Trailing slash trimmed. */
  setBackendUrl: (url: string | null) => void;
}

/**
 * Which data source the app talks to, chosen at runtime and persisted. Kept
 * separate from the workspace store: this is a client/device concern, not part
 * of the (backend-owned) workspace. Changing it re-routes `api` immediately.
 */
export const useConnection = create<ConnectionState>()(
  persist(
    (set) => ({
      backendUrl: initialBackendUrl,
      setBackendUrl: (url) => {
        const trimmed = url?.trim().replace(/\/+$/, '');
        set({ backendUrl: trimmed ? trimmed : null });
      },
    }),
    { name: 'agoralume-connection', version: 1 },
  ),
);
