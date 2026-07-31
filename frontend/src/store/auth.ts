import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface AuthState {
  /** Short-lived (15m); sent as `Authorization: Bearer <token>` on every request. */
  accessToken: string | null;
  /** Long-lived; used only to mint a fresh access token without a password prompt. */
  refreshToken: string | null;
  setTokens: (tokens: { accessToken: string; refreshToken: string }) => void;
  setAccessToken: (accessToken: string) => void;
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
export const useAuth = create<AuthState>()(
  persist(
    (set) => ({
      accessToken: null,
      refreshToken: null,
      setTokens: ({ accessToken, refreshToken }) => set({ accessToken, refreshToken }),
      setAccessToken: (accessToken) => set({ accessToken }),
      clear: () => set({ accessToken: null, refreshToken: null }),
    }),
    { name: 'agoralume-auth', version: 1 },
  ),
);
