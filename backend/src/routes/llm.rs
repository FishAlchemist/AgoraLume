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

use crate::models::{LlmSettingsPatch, LlmSettingsView};
use crate::state::AppState;

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(routes!(get_llm_settings, update_llm_settings))
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
