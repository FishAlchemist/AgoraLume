/**
 * The one place a failed backend response becomes an `Error`.
 *
 * Every 4xx from the backend is an RFC 9457 problem document
 * (`application/problem+json`, see `backend/src/api_error.rs`). Before that
 * existed, each client module here had its own copy of "read the body as text,
 * fall back to the status code" — and two of them didn't bother, so a failed
 * workspace write surfaced as `PATCH /groups/x failed: 422` with the server's
 * actual explanation thrown away.
 */

/** An RFC 9457 problem document, as the backend emits it. */
export interface Problem {
  /** Stable machine-readable identifier, e.g. `urn:agoralume:error:not-admin`. */
  type: string;
  /** The status code's reason phrase, e.g. `Forbidden`. */
  title: string;
  status: number;
  /** The human-readable explanation of this particular failure. */
  detail?: string;
}

const TYPE_PREFIX = 'urn:agoralume:error:';

/**
 * A failed request, carrying enough to both show the user and branch on.
 *
 * `message` is the server's own `detail` wherever there was one, so callers
 * that just render `error.message` show the real reason instead of a status
 * code.
 */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly type: string,
    message: string,
    /** From the `Retry-After` header on a 429 (see `ApiError::too_many_requests`); undefined otherwise. */
    readonly retryAfterSecs?: number,
  ) {
    super(message);
    this.name = 'ApiError';
  }

  /**
   * The problem type's stable slug — `not-admin`, `not-found`,
   * `invalid-request`, … — for the rare caller that needs to branch rather
   * than display. Falls back to the whole `type` if it isn't one of ours.
   */
  get code(): string {
    return this.type.startsWith(TYPE_PREFIX) ? this.type.slice(TYPE_PREFIX.length) : this.type;
  }

  /** Whether this identity is not permitted, as opposed to not signed in. */
  get isForbidden(): boolean {
    return this.status === 403;
  }
}

/**
 * Resolves when the response succeeded; throws an {@link ApiError} otherwise.
 *
 * `context` names the operation for the fallback message — used only when the
 * server sent no problem document at all (a proxy error page, a network-level
 * failure body), since anything from this backend carries one.
 */
export async function throwIfNotOk(res: Response, context: string): Promise<Response> {
  if (res.ok) return res;

  let problem: Partial<Problem> | null = null;
  try {
    // Read as text first: a non-JSON error body (an nginx 502 page, say) would
    // otherwise reject here and lose the status we do have.
    const body = await res.text();
    if (body) problem = JSON.parse(body) as Partial<Problem>;
  } catch {
    // Not a problem document — fall through to the status-only message.
  }

  const detail = typeof problem?.detail === 'string' ? problem.detail : '';
  const type = typeof problem?.type === 'string' ? problem.type : '';
  const retryAfter = Number(res.headers.get('Retry-After'));
  throw new ApiError(
    res.status,
    type,
    detail || `${context} failed: ${res.status}`,
    Number.isFinite(retryAfter) && retryAfter > 0 ? retryAfter : undefined,
  );
}

/** {@link throwIfNotOk}, then the parsed JSON body. */
export async function jsonOrThrow<T>(res: Response, context: string): Promise<T> {
  await throwIfNotOk(res, context);
  return (await res.json()) as T;
}
