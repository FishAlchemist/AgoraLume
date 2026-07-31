import { useAuth } from '../../store/auth';
import { useConnection } from '../../store/connection';
import { versionedBase } from './version';

// Concurrent 401s (a page loading several resources at once) must trigger one
// refresh, not one per caller — the second refresh_token use would otherwise
// race the first. Every caller awaits this same in-flight promise instead.
let refreshing: Promise<boolean> | null = null;

/** Exchanges the stored refresh token for a new access token, if it still works. */
async function refreshAccessToken(): Promise<boolean> {
  if (refreshing) return refreshing;
  refreshing = (async () => {
    const backendUrl = useConnection.getState().backendUrl;
    const refreshToken = useAuth.getState().refreshToken;
    if (!backendUrl || !refreshToken) return false;
    try {
      const res = await fetch(`${versionedBase(backendUrl)}/auth/refresh`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ refreshToken }),
      });
      if (!res.ok) return false;
      const data = (await res.json()) as { accessToken: string };
      useAuth.getState().setAccessToken(data.accessToken);
      return true;
    } catch {
      return false;
    }
  })();
  try {
    return await refreshing;
  } finally {
    refreshing = null;
  }
}

/**
 * `fetch`, with the stored access token attached as `Authorization: Bearer`.
 * A 401 triggers one refresh-and-retry — the access token is short-lived by
 * design (15 minutes), so this is the expected way it renews, not a rare
 * failure path. If the refresh token itself is missing or no longer works,
 * the session is cleared so the login gate (see `pages/LoginPage`) reappears;
 * the original 401 is still returned to the caller either way.
 */
export async function authFetch(input: string, init: RequestInit = {}): Promise<Response> {
  const attempt = () => {
    const token = useAuth.getState().accessToken;
    const headers = new Headers(init.headers);
    if (token) headers.set('Authorization', `Bearer ${token}`);
    return fetch(input, { ...init, headers });
  };

  const res = await attempt();
  if (res.status !== 401 || !useAuth.getState().refreshToken) return res;
  const refreshed = await refreshAccessToken();
  if (!refreshed) {
    useAuth.getState().clear();
    return res;
  }
  return attempt();
}
