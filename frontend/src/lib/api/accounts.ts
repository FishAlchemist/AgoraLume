import { authFetch } from './authFetch';
import type { AccountSummary } from './types';
import { versionedBase } from './version';

/**
 * Admin account management client — `GET`/`POST /accounts` on a real
 * backend. Same shape as `llmSettings.ts`: outside the {@link ChatApi}
 * contract (there's nothing to mock — creating accounts only makes sense
 * against a real backend), goes through `authFetch` so the admin's token is
 * attached, and doesn't pre-guess who's allowed to call it — a non-admin
 * caller just gets the backend's real 401 back (see `CurrentAdmin` in
 * `backend/src/state.rs`), same as everywhere else this pattern is used.
 */

async function getJson<T>(baseUrl: string, path: string): Promise<T> {
  const res = await authFetch(`${baseUrl}${path}`, { headers: { Accept: 'application/json' } });
  if (!res.ok) {
    const detail = await res.text().catch(() => '');
    throw new Error(detail || `GET ${path} failed: ${res.status}`);
  }
  return (await res.json()) as T;
}

/** Every existing account, for the admin dashboard's account list. */
export function listAccounts(backendUrl: string): Promise<AccountSummary[]> {
  return getJson<AccountSummary[]>(versionedBase(backendUrl), '/accounts');
}

/**
 * Provisions a brand-new account with an admin-chosen username and password.
 * There's no self-service registration — this is the only way an account
 * gets created. Rejects (throwing, with the backend's explanation as the
 * message) on an empty username/password, a reserved or already-taken
 * username, or a backend with no persistent data directory configured.
 */
export async function createAccount(
  backendUrl: string,
  username: string,
  password: string,
): Promise<AccountSummary> {
  const res = await authFetch(`${versionedBase(backendUrl)}/accounts`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  if (!res.ok) {
    const detail = await res.text().catch(() => '');
    throw new Error(detail || `createAccount failed: ${res.status}`);
  }
  return (await res.json()) as AccountSummary;
}
