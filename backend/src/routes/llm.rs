//! Operator config for the real-model provider: endpoint, key, tuning, and
//! pricing. Separate from `/preferences` (`routes::workspace`), which is the
//! signed-in person's own display choices with no secret in it — this is server
//! config with one, so it gets its own path and a response type that's never a
//! straight serialization of the stored settings (see [`LlmSettingsView`]).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::agent::llm::normalize_base_url;
use crate::api_error::ApiError;
use crate::models::{LlmModelsQuery, LlmModelsView, LlmSettingsPatch, LlmSettingsView};
use crate::state::{AppState, AuthenticatedSubject, CurrentAdmin};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(routes!(
        get_llm_settings,
        update_llm_settings,
        list_llm_models
    ))
}

/// The live LLM provider configuration. Any signed-in caller may read it;
/// `canEdit` says whether this caller may write it.
//
// `apiKey` is never included — only `hasApiKey`. Reading is open to admin and
// regular accounts alike (see [`AuthenticatedSubject`]); `canEdit` is computed
// here from the resolved `Subject` rather than hard-coded on the frontend, so a
// future change to who may write only has to change this one line.
#[utoipa::path(get, path = "/llm/settings", tag = "llm",
    responses((status = 200, description = "The live configuration; the API key is reduced to `hasApiKey`", body = LlmSettingsView)))]
async fn get_llm_settings(
    AuthenticatedSubject(subject): AuthenticatedSubject,
    State(s): State<Arc<AppState>>,
) -> Json<LlmSettingsView> {
    let mut view = LlmSettingsView::from(&s.llm_settings());
    view.can_edit = matches!(subject, crate::auth::Subject::Admin);
    Json(view)
}

/// Merges a partial update onto the LLM provider configuration and applies it
/// immediately. Admin-only.
//
// The candidate configuration is validated — the brain it describes must
// actually build — before anything is swapped in or written to `llm.toml`, so a
// rejected patch changes nothing. Admin-only (see [`CurrentAdmin`]): a regular
// account can read this config but not change shared operator-level server
// config or its real-model spend.
#[utoipa::path(patch, path = "/llm/settings", tag = "llm",
    request_body = LlmSettingsPatch,
    responses(
        (status = 200, description = "Applied and persisted to llm.toml; no restart needed", body = LlmSettingsView),
        (status = 422, description = "The resulting configuration would not build; nothing changed")))]
async fn update_llm_settings(
    _admin: CurrentAdmin,
    State(s): State<Arc<AppState>>,
    Json(patch): Json<LlmSettingsPatch>,
) -> Result<Json<LlmSettingsView>, ApiError> {
    let mut settings = s.llm_settings();
    patch.apply(&mut settings);
    let applied = s.apply_llm_settings(settings).map_err(ApiError::unprocessable)?;
    if s.auth_required() {
        tracing::info!("LLM provider settings updated by admin");
    } else {
        tracing::info!("LLM provider settings updated (auth not enforced on this server)");
    }
    // Reaching this line already proved `CurrentAdmin`, so this response's
    // `canEdit` is `true` too — same fact `GET` would report right after.
    let mut view = LlmSettingsView::from(&applied);
    view.can_edit = true;
    Ok(Json(view))
}

/// Lists the models a provider endpoint offers, so the model field can be a
/// picker. Admin-only. `apiKey` is required unless `baseUrl` matches the
/// already-configured endpoint.
//
// That last rule is the credential guard: without it, pointing `baseUrl` at an
// arbitrary third-party URL and omitting `apiKey` would have the server hand
// that URL the real provider key in an outbound `Authorization` header. A POST
// rather than a GET because the body may carry a key, and a URL gets logged by
// every proxy in between. Admin-only (see [`CurrentAdmin`]) — it can spend the
// stored key on an outbound request.
#[utoipa::path(post, path = "/llm/models", tag = "llm",
    request_body = LlmModelsQuery,
    responses(
        (status = 200, description = "The models that endpoint reports", body = LlmModelsView),
        (status = 422, description = "Empty or non-HTTP(S) baseUrl, no usable key, or the endpoint refused")))]
async fn list_llm_models(
    _admin: CurrentAdmin,
    State(s): State<Arc<AppState>>,
    Json(query): Json<LlmModelsQuery>,
) -> Result<Json<LlmModelsView>, ApiError> {
    let base_url = query.base_url.trim();
    if base_url.is_empty() {
        return Err(ApiError::unprocessable("baseUrl is required"));
    }
    check_outbound_scheme(base_url).map_err(ApiError::unprocessable)?;
    let stored = s.llm_settings();
    let api_key =
        resolve_api_key(query.api_key, base_url, stored.base_url.as_deref(), stored.api_key)
            .map_err(ApiError::unprocessable)?;
    let models =
        crate::agent::llm::list_models(base_url, &api_key).await.map_err(ApiError::unprocessable)?;
    Ok(Json(LlmModelsView { models }))
}

/// Refuses a `baseUrl` this server has no business dialling.
///
/// This endpoint is the one place the API makes an *outbound* request to an
/// address the caller chose, so the caller's string is treated as hostile even
/// though only an admin can reach it. A scheme check stops the categories that
/// aren't HTTP at all — `file://`, `gopher://`, and anything else a URL parser
/// downstream might be talked into.
///
/// Private and loopback addresses are deliberately **not** blocked: running a
/// local model server (Ollama, LM Studio, llama.cpp) at `127.0.0.1` or on a LAN
/// host is a first-class way to use this project, and blocking RFC 1918 would
/// break that to defend against a caller who already holds an admin token and
/// can rewrite `llm.toml` anyway. The residual risk is that an admin can use
/// the server as an HTTP client against hosts it can reach; that is inherent to
/// the feature, not incidental to it.
fn check_outbound_scheme(base_url: &str) -> Result<(), String> {
    let lowered = base_url.to_ascii_lowercase();
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        Ok(())
    } else {
        Err("baseUrl must be an http:// or https:// URL".to_string())
    }
}

/// The api-key half of [`list_llm_models`]'s guard, pulled out as a pure
/// function so the security-relevant case (an explicit key wins; an omitted
/// one falls back to the stored key only when `requested_base_url` matches
/// the already-configured endpoint) is directly unit-testable without a router.
fn resolve_api_key(
    query_api_key: Option<String>,
    requested_base_url: &str,
    stored_base_url: Option<&str>,
    stored_api_key: Option<String>,
) -> Result<String, String> {
    if let Some(key) = query_api_key {
        return Ok(key);
    }
    let matches_stored = stored_base_url
        .is_some_and(|url| normalize_base_url(url) == normalize_base_url(requested_base_url));
    if !matches_stored {
        return Err(
            "apiKey is required for a baseUrl other than the currently-configured one".to_string(),
        );
    }
    Ok(stored_api_key.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_api_key_wins_regardless_of_stored_base_url() {
        let key = resolve_api_key(
            Some("fresh-key".to_string()),
            "https://attacker.example/v1",
            Some("https://api.openai.com/v1"),
            Some("stored-key".to_string()),
        )
        .expect("an explicit key is always accepted");
        assert_eq!(key, "fresh-key");
    }

    #[test]
    fn omitted_api_key_falls_back_to_stored_only_for_the_matching_base_url() {
        let key = resolve_api_key(
            None,
            "https://api.openai.com/v1",
            Some("https://api.openai.com/v1"),
            Some("stored-key".to_string()),
        )
        .expect("baseUrl matches the stored one");
        assert_eq!(key, "stored-key");

        // Trailing slash / letter case differences still count as a match.
        let key = resolve_api_key(
            None,
            "HTTPS://API.OPENAI.COM/v1/",
            Some("https://api.openai.com/v1"),
            Some("stored-key".to_string()),
        )
        .expect("normalization tolerates slash/case differences");
        assert_eq!(key, "stored-key");
    }

    #[test]
    fn omitted_api_key_is_rejected_for_a_different_base_url() {
        // This is the credential-exfiltration path: without this guard, an
        // omitted apiKey would send the real stored key to any baseUrl the
        // caller names.
        let err = resolve_api_key(
            None,
            "https://attacker.example/v1",
            Some("https://api.openai.com/v1"),
            Some("stored-key".to_string()),
        )
        .expect_err("a different baseUrl must not receive the stored key");
        assert!(err.contains("apiKey is required"));
    }

    #[test]
    fn omitted_api_key_is_rejected_when_nothing_is_stored_yet() {
        let err = resolve_api_key(None, "https://api.openai.com/v1", None, None)
            .expect_err("no stored baseUrl to match against");
        assert!(err.contains("apiKey is required"));
    }
}
