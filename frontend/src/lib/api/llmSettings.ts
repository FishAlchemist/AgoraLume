import { authFetch } from './authFetch';
import { jsonOrThrow } from './problem';
import type { LlmModelsView, LlmSettingsPatch, LlmSettingsView } from './types';
import { versionedBase } from './version';

/**
 * The LLM provider configuration client — `GET`/`PATCH /llm/settings` on a
 * real backend. Deliberately outside the {@link ChatApi} contract: that
 * interface is the chat data source (message history, replies, streams,
 * usage), which the in-browser mock also implements. Configuring a backend's
 * real-model *provider* is meaningless without a real backend, so this talks
 * to a `baseUrl` directly rather than routing through the mock/HTTP split —
 * callers (the Settings page) only render this section when a backend is
 * connected. Every call here goes through `authFetch`, not a bare `fetch`,
 * because the backend requires an authenticated caller on all three routes
 * (see `AuthenticatedSubject` in `backend/src/state.rs`) — that's the actual
 * enforcement. This deliberately does not try to guess and pre-block an
 * unauthenticated caller on the frontend: an anonymous guest just gets the
 * real 401 (and its detail text) back from the server, same as anyone else
 * a token stops working for.
 */

async function getJson<T>(baseUrl: string, path: string): Promise<T> {
  const res = await authFetch(`${baseUrl}${path}`, { headers: { Accept: 'application/json' } });
  return jsonOrThrow<T>(res, `GET ${path}`);
}

/** The live LLM provider configuration, with the API key stripped to a presence flag. */
export function getLlmSettings(baseUrl: string): Promise<LlmSettingsView> {
  return getJson<LlmSettingsView>(versionedBase(baseUrl), '/llm/settings');
}

/**
 * Merges a partial update onto the LLM provider configuration and applies it
 * immediately — no restart needed. Rejects (throwing, with the backend's
 * explanation as the message) if the resulting configuration wouldn't build,
 * e.g. `enabled: true` without both `baseUrl` and `model`.
 */
export async function updateLlmSettings(
  baseUrl: string,
  patch: LlmSettingsPatch,
): Promise<LlmSettingsView> {
  const res = await authFetch(`${versionedBase(baseUrl)}/llm/settings`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  });
  return jsonOrThrow<LlmSettingsView>(res, 'updateLlmSettings');
}

/**
 * Lists the models a provider endpoint offers, so the model field can be a
 * picker instead of a blind text box. `apiKey` is optional — omit it to use
 * whatever key is already stored on the backend (the frontend never actually
 * holds it once saved); the backend only honors that fallback when `baseUrl`
 * matches the currently-configured endpoint, so it can't be used to make the
 * server leak its stored key to an arbitrary URL. Rejects (throwing, with the
 * backend's explanation as the message) on an empty `baseUrl`, a `baseUrl`
 * that doesn't match with no `apiKey` given, or the endpoint itself failing.
 */
export async function listLlmModels(
  backendUrl: string,
  query: { baseUrl: string; apiKey?: string },
): Promise<LlmModelsView> {
  const res = await authFetch(`${versionedBase(backendUrl)}/llm/models`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(query),
  });
  return jsonOrThrow<LlmModelsView>(res, 'listLlmModels');
}
