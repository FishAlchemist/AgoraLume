import { versionedBase } from './version';

export interface TokenPair {
  accessToken: string;
  refreshToken: string;
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
  if (!res.ok) {
    const detail = await res.text().catch(() => '');
    throw new Error(detail || `login failed: ${res.status}`);
  }
  return (await res.json()) as TokenPair;
}
