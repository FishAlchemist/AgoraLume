import { create } from 'zustand';
import { useAuth } from './auth';

/**
 * Reachability of the data source, kept separate from its *mode*:
 * - `local`    — the in-browser mock (no server; nothing to reach)
 * - `checking` — a backend is configured, first probe in flight
 * - `online`   — the backend answered
 * - `offline`  — the backend is unreachable
 */
export type Reachability = 'local' | 'checking' | 'online' | 'offline';

export interface BackendStatus {
  reachable: Reachability;
  /**
   * Mock = no LLM and no persistence (pure in-memory). True for the in-browser
   * mock and for a backend running in mock mode; independent of reachability.
   * Undefined while `checking`/`offline` (mode unknown).
   */
  mock?: boolean;
  /**
   * Whether the backend enforces login. Independent of `mock`: a persistent
   * backend with no real model still requires it, and `AGORALUME_AUTH_DISABLED`
   * waives it even on a live one. Undefined while `checking`/`offline`.
   */
  authRequired?: boolean;
}

/**
 * Backend liveness/mode, live outside React. `lib/useBackendStatus`'s poll
 * loop is the only writer (it owns the `/meta` calls); everything else —
 * this module's `isGuestFallback`, `lib/api/index.ts`'s routing, and
 * `store/workspace.ts`'s `backend()` — only reads it, so none of them need to
 * be React components to make a mock-vs-real decision.
 */
export const useBackendStatusStore = create<BackendStatus>(() => ({
  reachable: 'checking',
}));

/**
 * Whether a live, auth-requiring backend should be treated as unreachable for
 * data purposes because there's no usable session — the "browse a demo, log
 * in for your real data" fallback (see `pages/LoginPage`). `authRequired`
 * starts `undefined` until the first `/meta` probe resolves; treated the same
 * as `true` so the app never briefly renders real-looking data before it
 * actually knows whether a session is required to see it.
 *
 * An admin session counts as "no usable session" too, on purpose: admin has
 * no workspace of its own (see `backend/src/state.rs`'s `CurrentAccount`),
 * so every group/message route would just 401 for it. Routing an admin
 * session through the same shared-seed demo a guest sees — rather than
 * building a second, admin-specific "nothing to show" path — means every
 * caller of this function (chat routing, subscription effects, the
 * data-source badge) already does the right thing with no further changes.
 */
export function isGuestFallback(): boolean {
  const { accessToken, role } = useAuth.getState();
  return (
    useBackendStatusStore.getState().authRequired !== false && (!accessToken || role === 'admin')
  );
}

/**
 * Reactive form of `isGuestFallback`, for components that open a live
 * subscription (chat streams, SSE) against whichever data source is current —
 * without this in an effect's deps, a subscription opened during the guest
 * fallback (or during it) never rebinds when a login, a logout, or the
 * `/meta` probe resolving flips the routing target.
 */
export function useIsGuestFallback(): boolean {
  const authRequired = useBackendStatusStore((s) => s.authRequired);
  const accessToken = useAuth((s) => s.accessToken);
  const role = useAuth((s) => s.role);
  return authRequired !== false && (!accessToken || role === 'admin');
}
