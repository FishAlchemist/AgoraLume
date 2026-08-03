//! The one error shape every endpoint returns.
//!
//! Before this existed, a failure could reach the client three different ways:
//! a bare status with no body at all (every workspace 404/409/422), a
//! `text/plain` sentence (`/auth/*`, `/accounts`, `/llm/*`), or — for the auth
//! extractors — a `text/plain` sentence the OpenAPI document never mentioned.
//! A client had no way to tell *what* went wrong without matching on English
//! prose, and half the failures carried no prose either.
//!
//! Everything now returns [`ApiError`], serialized as `application/problem+json`
//! per RFC 9457 (which obsoletes RFC 7807): `type` is a stable machine-readable
//! identifier, `title` and `status` restate the HTTP status, and `detail`
//! carries the human sentence that used to be the entire body. The
//! `application/problem+json` media type is what tells a generic client this is
//! a problem document rather than the resource it asked for — a plain
//! `application/json` object would be ambiguous.
//!
//! `type` is a URN rather than an `https://` URL on purpose: RFC 9457 wants a
//! URI that *identifies* the problem type, and a URN says "this is an
//! identifier, don't try to dereference it" instead of promising a
//! documentation page this project doesn't host.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// The URN namespace every [`ApiError::kind`] lives in. Callers match on the
/// full string; the suffix is the stable slug.
const TYPE_PREFIX: &str = "urn:agoralume:error:";

/// An RFC 9457 problem document — the body of every 4xx this API produces.
///
/// Constructed through the named helpers below rather than field-by-field, so
/// the slug used for a given status is decided in exactly one place and can't
/// drift between handlers.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiError {
    /// Stable, machine-readable identifier for *what* went wrong, e.g.
    /// `urn:agoralume:error:not-admin`. Match on this rather than on `detail`,
    /// whose wording is free to change. Always present.
    #[serde(rename = "type")]
    #[schema(rename = "type", example = "urn:agoralume:error:not-found")]
    pub kind: String,
    /// The status code's canonical reason phrase (e.g. `Forbidden`) — a short
    /// human summary of the problem *type*, not of this occurrence.
    pub title: String,
    /// The HTTP status code, repeated in the body so a problem document stays
    /// meaningful when it's logged or forwarded away from its response.
    pub status: u16,
    /// The human-readable explanation of *this specific* occurrence — the
    /// sentence that used to be the entire `text/plain` body. Safe to show a
    /// user; never carries a secret, a stack trace, or a provider's raw
    /// response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The `WWW-Authenticate` challenge to send alongside a 401. Not part of
    /// the body — RFC 9110 requires the *header* on a 401, and this is where
    /// the value rides along until [`IntoResponse`] can set it.
    #[serde(skip)]
    challenge: Option<&'static str>,
    /// The `Retry-After` value (seconds) to send alongside a 429. Same
    /// carry-along mechanism as `challenge`, just for a header whose value
    /// isn't known until the call site, so it can't be `&'static str`.
    #[serde(skip)]
    retry_after_secs: Option<u32>,
}

impl ApiError {
    /// The shared constructor: `title` is always the status's canonical reason,
    /// so it can never disagree with `status`.
    fn new(status: StatusCode, slug: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: format!("{TYPE_PREFIX}{slug}"),
            title: status.canonical_reason().unwrap_or("Error").to_string(),
            status: status.as_u16(),
            detail: Some(detail.into()),
            challenge: None,
            retry_after_secs: None,
        }
    }

    /// 401 — the request isn't authenticated at all: no token, or one the
    /// server doesn't recognize. Carries the `WWW-Authenticate` challenge RFC
    /// 9110 §15.5.2 requires on every 401. Contrast [`ApiError::forbidden`],
    /// which is for a caller the server *did* authenticate.
    pub fn unauthorized(slug: &str, detail: impl Into<String>, challenge: &'static str) -> Self {
        Self { challenge: Some(challenge), ..Self::new(StatusCode::UNAUTHORIZED, slug, detail) }
    }

    /// 403 — the caller is authenticated, but this identity may not do this.
    /// Retrying with a fresh token is pointless, which is exactly what
    /// distinguishes it from a 401 to a client that auto-refreshes.
    pub fn forbidden(slug: &str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, slug, detail)
    }

    /// 404 — no such resource. One slug for all of them; `detail` says which.
    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not-found", detail)
    }

    /// 409 — the request is well-formed but collides with a rule about the
    /// current state (a taken name, the last remaining user identity).
    pub fn conflict(slug: &str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, slug, detail)
    }

    /// 422 — the body parsed as JSON but doesn't describe something the server
    /// can act on (a field of the wrong type, a configuration that wouldn't
    /// build, a username already taken).
    pub fn unprocessable(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid-request", detail)
    }

    /// 429 — this caller (or this specific username, for a login throttle)
    /// has to wait `retry_after_secs` before trying again. Carries
    /// `Retry-After` per RFC 9110 §10.2.3.
    pub fn too_many_requests(slug: &str, detail: impl Into<String>, retry_after_secs: u32) -> Self {
        Self {
            retry_after_secs: Some(retry_after_secs),
            ..Self::new(StatusCode::TOO_MANY_REQUESTS, slug, detail)
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // `self.status` came from a `StatusCode` in every constructor, so the
        // round-trip can't realistically fail; falling back to 500 rather than
        // unwrapping keeps a future hand-built value from panicking a handler.
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let challenge = self.challenge;
        let retry_after_secs = self.retry_after_secs;
        let mut response = (status, Json(self)).into_response();
        // `Json` sets `application/json`; the whole point of a problem document
        // is that it announces itself as one, so overwrite it.
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(challenge) = challenge {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
        }
        if let Some(secs) = retry_after_secs {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_is_a_stable_urn_and_title_matches_the_status() {
        let error = ApiError::forbidden("not-admin", "this route requires the admin role");
        assert_eq!(error.kind, "urn:agoralume:error:not-admin");
        assert_eq!(error.title, "Forbidden");
        assert_eq!(error.status, 403);
    }

    /// The two things a generic HTTP client keys off: the media type that says
    /// "this is a problem document", and the challenge RFC 9110 requires on a
    /// 401 (and must *not* appear on anything else).
    #[test]
    fn problem_media_type_always_set_and_challenge_only_on_401() {
        let unauthorized = ApiError::unauthorized("invalid-token", "expired", "Bearer")
            .into_response();
        assert_eq!(
            unauthorized.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        assert_eq!(unauthorized.headers().get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");

        let not_found = ApiError::not_found("no such group").into_response();
        assert_eq!(
            not_found.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        assert!(not_found.headers().get(header::WWW_AUTHENTICATE).is_none());
    }
}
