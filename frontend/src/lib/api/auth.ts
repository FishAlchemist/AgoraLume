import { jsonOrThrow, throwIfNotOk } from './problem';
import { versionedBase } from './version';

export interface TokenPair {
  accessToken: string;
  refreshToken: string;
  /** Which kind of session this token belongs to — see `store/auth.ts`'s `role`. */
  role: 'admin' | 'account';
}

/**
 * `POST /auth/login` — the one login flow every role shares (see
 * backend/src/routes/auth.rs). Plain `fetch`, not `authFetch`: there is no
 * session yet to attach or refresh. Throws (with the backend's explanation,
 * where it has one) on a wrong username/password.
 */
export async function login(
  backendUrl: string,
  username: string,
  password: string,
): Promise<TokenPair> {
  const res = await fetch(`${versionedBase(backendUrl)}/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  return jsonOrThrow<TokenPair>(res, 'login');
}

/**
 * `POST /auth/logout` — ends the session on the *server*, not just in this tab.
 *
 * Clearing the local store alone left both tokens live until they expired (the
 * refresh token for 30 days), so anything that had captured one kept the
 * session after the user believed they'd signed out. Plain `fetch`, and no
 * retry: the endpoint is public by design (holding a token is what entitles you
 * to destroy it) and always answers 204, so there is nothing to refresh or
 * re-attempt. Best-effort — a signed-out user must not be blocked by a network
 * failure, so the caller clears local state either way.
 */
export async function logout(
  backendUrl: string,
  accessToken: string | null,
  refreshToken: string | null,
): Promise<void> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (accessToken) headers.Authorization = `Bearer ${accessToken}`;
  const res = await fetch(`${versionedBase(backendUrl)}/auth/logout`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ refreshToken }),
  });
  await throwIfNotOk(res, 'logout');
}
