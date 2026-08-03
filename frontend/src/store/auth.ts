import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface AuthState {
  /** Short-lived (15m); sent as `Authorization: Bearer <token>` on every request. */
  accessToken: string | null;
  /** Long-lived; used only to mint a fresh access token without a password prompt. */
  refreshToken: string | null;
  /**
   * Whatever was typed into the login form — `POST /auth/login` doesn't echo
   * an account's own username back, so this is the frontend's own record of
   * who it asked to sign in as, kept purely so the header can show
   * *something* rather than nothing.
   */
  username: string | null;
  /**
   * `"admin"` or `"account"`, straight from the login response — see
   * `backend/src/routes/auth.rs`'s `TokenPair.role`. A UX routing hint only
   * (e.g. `isGuestFallback` treats an admin session like no session, since
   * admin has no workspace of its own): every actual permission check still
   * happens backend-side regardless of what this says.
   */
  role: 'admin' | 'account' | null;
  setTokens: (tokens: {
    accessToken: string;
    refreshToken: string;
    username: string;
    role: 'admin' | 'account';
  }) => void;
  /**
   * `POST /auth/refresh` rotates the refresh token on every use (see
   * `backend/src/auth.rs`'s `TokenStore::refresh`), so a refresh must update
   * both tokens, not just the access token — the old refresh token stops
   * working the moment this one is stored.
   */
  setRefreshedTokens: (accessToken: string, refreshToken: string) => void;
  /** Drops both tokens — the login gate reappears on the next render. */
  clear: () => void;
}

/**
 * The logged-in session, for the one regular-account auth flow the whole app
 * shares (`POST /auth/login`, `POST /auth/refresh` — see backend/src/auth.rs).
 * Persisted like the other client/device stores (`connection`, `readOnly`) so
 * a reload doesn't force a fresh login; {@link ../lib/api/authFetch} reads it
 * on every request and clears it if the refresh token itself stops working.
 */
const STORAGE_KEY = 'agoralume-auth';

export const useAuth = create<AuthState>()(
  persist(
    (set) => ({
      accessToken: null,
      refreshToken: null,
      username: null,
      role: null,
      setTokens: ({ accessToken, refreshToken, username, role }) =>
        set({ accessToken, refreshToken, username, role }),
      setRefreshedTokens: (accessToken, refreshToken) => set({ accessToken, refreshToken }),
      clear: () => set({ accessToken: null, refreshToken: null, username: null, role: null }),
    }),
    { name: STORAGE_KEY, version: 1 },
  ),
);

/**
 * The refresh token is rotated on every use (see `backend/src/auth.rs`'s
 * `TokenStore::refresh`), so a second tab's in-memory copy of this store goes
 * stale the instant another tab refreshes — `persist` writes to
 * `localStorage`, but nothing re-reads it into a *different* tab's store on
 * its own. Without this, the stale tab's own next refresh presents an
 * already-rotated token, which past the backend's short grace window is
 * denied and signs that tab out for no reason (and its sign-out button sends
 * a token the live session doesn't recognize, revoking nothing). The
 * `storage` event fires in every tab except the one that wrote, which is
 * exactly the one that needs to catch up.
 */
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (e) => {
    if (e.key === STORAGE_KEY) void useAuth.persist.rehydrate();
  });
}
