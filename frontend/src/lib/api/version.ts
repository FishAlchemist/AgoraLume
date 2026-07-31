/**
 * The wire contract's version segment, appended to every backend base URL by
 * {@link ../http.HttpChatApi} and {@link ../workspace.HttpWorkspaceApi}. Mirrors
 * `API_VERSION` in `backend/src/routes/mod.rs` — bump both together.
 */
export const API_VERSION = '/v1beta';

/**
 * Joins a user-supplied backend URL with {@link API_VERSION} through the `URL`
 * API rather than manual string concatenation, so the browser's own resolver
 * — not a hand-rolled regex — decides where slashes go. `URL`'s relative-path
 * resolution only appends (instead of replacing the whole pathname, as a
 * leading `/` would) when the base ends with `/`, so both sides are
 * normalized to exactly one before the join. Trailing slash stripped on the
 * way out so existing call sites (`` `${baseUrl}${path}` `` with a
 * leading-`/` path) keep working unchanged.
 */
export function versionedBase(rawBaseUrl: string): string {
  const base = rawBaseUrl.endsWith('/') ? rawBaseUrl : `${rawBaseUrl}/`;
  const joined = new URL(`${API_VERSION.slice(1)}/`, base);
  return joined.toString().replace(/\/$/, '');
}
