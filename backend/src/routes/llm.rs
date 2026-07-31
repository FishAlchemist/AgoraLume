//! Operator config for the real-model provider: endpoint, key, tuning, and
//! pricing. Separate from `/settings` (`routes::workspace`), which is
//! client-side preferences with no secret in it — this is server config with
//! one, so it gets its own path and a response type that's never a straight
//! serialization of the stored settings (see [`LlmSettingsView`]).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::agent::llm::normalize_base_url;
use crate::models::{LlmModelsQuery, LlmModelsView, LlmSettingsPatch, LlmSettingsView};
use crate::state::AppState;

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(routes!(
        get_llm_settings,
        update_llm_settings,
        list_llm_models
    ))
}

/// The live LLM provider configuration. `apiKey` is never included — only
/// `hasApiKey`, whether one is currently stored.
#[utoipa::path(get, path = "/llm/settings", tag = "llm",
    responses((status = 200, body = LlmSettingsView)))]
async fn get_llm_settings(State(s): State<Arc<AppState>>) -> Json<LlmSettingsView> {
    Json(LlmSettingsView::from(&s.llm_settings()))
}

/// Merges a partial update onto the LLM provider configuration and applies it
/// immediately — no restart needed. The candidate configuration is validated
/// (the brain it describes must actually build) before anything is swapped in
/// or written to `llm.toml`; an invalid patch is rejected with 422 and changes
/// nothing.
#[utoipa::path(patch, path = "/llm/settings", tag = "llm",
    request_body = LlmSettingsPatch,
    responses(
        (status = 200, description = "Applied immediately; persisted to llm.toml", body = LlmSettingsView),
        (status = 422, description = "e.g. enabled=true without both baseUrl and model, or an endpoint that fails to construct", body = String),
    ))]
async fn update_llm_settings(
    State(s): State<Arc<AppState>>,
    Json(patch): Json<LlmSettingsPatch>,
) -> Result<Json<LlmSettingsView>, (StatusCode, String)> {
    let mut settings = s.llm_settings();
    patch.apply(&mut settings);
    let applied = s
        .apply_llm_settings(settings)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(LlmSettingsView::from(&applied)))
}

/// Lists the models a provider endpoint offers, so the Settings page can offer
/// a picker instead of a blind text field. `apiKey` is optional: when omitted,
/// the stored key is used, but *only* when `baseUrl` names the same endpoint
/// already configured — otherwise the request must carry its own key. Without
/// that check, an operator (or anything else that can reach this API) could
/// point `baseUrl` at an arbitrary third-party URL and have the server hand it
/// the real provider key in an outbound `Authorization` header.
#[utoipa::path(post, path = "/llm/models", tag = "llm",
    request_body = LlmModelsQuery,
    responses(
        (status = 200, body = LlmModelsView),
        (status = 422, description = "empty baseUrl, no usable key, or the endpoint rejected the request", body = String),
    ))]
async fn list_llm_models(
    State(s): State<Arc<AppState>>,
    Json(query): Json<LlmModelsQuery>,
) -> Result<Json<LlmModelsView>, (StatusCode, String)> {
    let base_url = query.base_url.trim();
    if base_url.is_empty() {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "baseUrl is required".to_string()));
    }
    let stored = s.llm_settings();
    let api_key = resolve_api_key(query.api_key, base_url, stored.base_url.as_deref(), stored.api_key)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let models = crate::agent::llm::list_models(base_url, &api_key)
        .await
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(LlmModelsView { models }))
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
