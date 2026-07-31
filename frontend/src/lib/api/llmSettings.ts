import type { LlmModelsView, LlmSettingsPatch, LlmSettingsView } from './types';
import { versionedBase } from './version';

/**
 * The LLM provider configuration client — `GET`/`PATCH /llm/settings` on a
 * real backend. Deliberately outside the {@link ChatApi} contract: that
 * interface is the chat data source (message history, replies, streams,
 * usage), which the in-browser mock also implements. Configuring a backend's
 * real-model *provider* is meaningless without a real backend, so this talks
 * to a `baseUrl` directly rather than routing through the mock/HTTP split —
 * callers (the Settings page) only render this section when one is connected.
 */

async function getJson<T>(baseUrl: string, path: string): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, { headers: { Accept: 'application/json' } });
  if (!res.ok) throw new Error(`GET ${path} failed: ${res.status}`);
  return (await res.json()) as T;
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
  const res = await fetch(`${versionedBase(baseUrl)}/llm/settings`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  });
  if (!res.ok) {
    const detail = await res.text().catch(() => '');
    throw new Error(detail || `updateLlmSettings failed: ${res.status}`);
  }
  return (await res.json()) as LlmSettingsView;
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
  const res = await fetch(`${versionedBase(backendUrl)}/llm/models`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(query),
  });
  if (!res.ok) {
    const detail = await res.text().catch(() => '');
    throw new Error(detail || `listLlmModels failed: ${res.status}`);
  }
  return (await res.json()) as LlmModelsView;
}
