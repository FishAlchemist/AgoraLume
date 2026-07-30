//! LLM provider configuration, persisted to a hand-editable `llm.toml` instead
//! of environment variables.
//!
//! Env vars have three problems this file fixes: they're awkward to manage (a
//! dozen exports, or a `.env` beside the binary), they can't be changed without
//! restarting the process, and — since nothing about *which* model produced a
//! given trace was ever recorded — the cost readout couldn't break usage down
//! by model. [`LlmSettings`] is the single record of that configuration now;
//! [`LlmConfigStore`] reads and writes it as TOML under the data directory, and
//! [`AppState::apply_llm_settings`](crate::state::AppState::apply_llm_settings)
//! applies a change to the live [`AgentRuntime`] without a restart.
//!
//! The API key is the one field that must never round-trip back to a client:
//! [`crate::models::LlmSettingsView`] reports only `has_api_key`, never the key
//! itself. Hand-editing the file directly still works — it just takes effect on
//! the next restart, since the running server doesn't watch it for changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::agent::brain::AgentBrain;
use crate::agent::llm::{LlmBrain, LlmConfig};
use crate::agent::mock::RuleBrain;
use crate::agent::turn::{AgentRuntime, LoopConfig};
use crate::models::Cost;

/// Token pricing used to turn usage into an estimated cost. Rates are per
/// 1,000,000 tokens, in `currency`. A rough operator-supplied estimate — models
/// and providers differ — so the UI always labels the result "for reference".
/// Doubles as a wire type (`GET`/`PATCH /llm/settings`), hence `camelCase`; the
/// one place that reads a little unusual is its own corner of `llm.toml`.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pricing {
    /// Price per 1M fresh (non-cached) input tokens.
    pub input_per_m: f64,
    /// Price per 1M cached input tokens. Usually cheaper than fresh input;
    /// defaults to the fresh input rate when not set (a conservative estimate
    /// that shows no cache saving until a discounted rate is provided).
    pub cached_input_per_m: f64,
    /// Price per 1M output tokens.
    pub output_per_m: f64,
    /// Currency label the rates are quoted in.
    pub currency: String,
}

impl Pricing {
    /// Estimates the cost of accumulated usage. Fresh input = prompt tokens not
    /// served from cache; the split lets the panel show what the cache saved.
    pub fn estimate(
        &self,
        prompt_tokens: u64,
        cached_prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Cost {
        let per_million = |tokens: u64, rate: f64| (tokens as f64) / 1_000_000.0 * rate;
        let fresh_input = prompt_tokens.saturating_sub(cached_prompt_tokens);
        let input = per_million(fresh_input, self.input_per_m);
        let cached_input = per_million(cached_prompt_tokens, self.cached_input_per_m);
        let output = per_million(completion_tokens, self.output_per_m);
        Cost {
            currency: self.currency.clone(),
            input,
            cached_input,
            output,
            total: input + cached_input + output,
        }
    }
}

/// The LLM provider configuration: everything that used to be an
/// `AGORALUME_LLM_*` / `AGORALUME_COMPRESS_*` environment variable. Read from
/// (and written to) `llm.toml` under the data directory by [`LlmConfigStore`];
/// snake_case, matching TOML convention (the wire shape frontend code sees is
/// the separate, camelCase [`crate::models::LlmSettingsView`] /
/// [`crate::models::LlmSettingsPatch`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    /// Whether to leave mock mode and drive agents with a real LLM. Off by
    /// default so a plain run never spends API budget.
    pub enabled: bool,
    /// OpenAI-compatible API root, e.g. `https://api.openai.com/v1` or a local
    /// `http://localhost:11434/v1` (Ollama). Pointing this at Gemini's
    /// OpenAI-compat shim is detected and redirected to rig's native Gemini
    /// provider, since the compat wire format can't carry Gemini's
    /// `thoughtSignature` — see `agent::llm`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Model name to request, e.g. `gpt-4o-mini` or `llama3.1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Bearer key for the endpoint. Optional: local endpoints (Ollama,
    /// llama.cpp) usually need no key. Never sent back to a client once set —
    /// see [`crate::models::LlmSettingsView`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Upper bound on tokens per reply.
    pub max_tokens: u64,
    /// Server-wide cap on LLM requests per rolling minute, so a free-tier quota
    /// isn't blown. `0` disables throttling.
    pub max_rpm: u64,
    /// Automatic retries a retryable failure (429/5xx/transport) gets before the
    /// error is surfaced to the chat. `0` disables auto-retry.
    pub max_retries: u32,
    /// Base backoff (ms) before the first retry; each further retry doubles it.
    pub retry_base_ms: u64,
    /// Context-compression high-water mark: once a group's un-summarized
    /// conversation tail exceeds this many lines, the oldest ones are folded
    /// into a running summary so the prompt stops growing without bound. `0`
    /// disables compression. Only ever acts when `enabled` — the mock has no
    /// model to summarize with.
    pub compress_after: usize,
    /// How many of the most recent conversation lines stay verbatim when
    /// compressing. Kept below `compress_after` so a compression always has
    /// older lines to fold.
    pub compress_keep: usize,
    /// Output-token ceiling for one summarization call — larger than
    /// `max_tokens`, since a running summary is a cumulative digest of a whole
    /// multi-persona history, not a single short reply.
    pub compress_max_tokens: u64,
    /// Optional token pricing for the estimated-cost readout. `None` shows
    /// token counts only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<Pricing>,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: None,
            model: None,
            api_key: None,
            max_tokens: 512,
            max_rpm: 15,
            max_retries: 2,
            retry_base_ms: 1000,
            compress_after: 50,
            compress_keep: 20,
            compress_max_tokens: 1024,
            pricing: None,
        }
    }
}

impl LlmSettings {
    /// Builds the runtime pieces this configuration describes: the brain, the
    /// loop's compression tuning, and whether it's the mock. `enabled = false`
    /// always succeeds with the rule-based mock. `enabled = true` requires
    /// `base_url` and `model` and a client that can actually be constructed;
    /// failure is returned rather than panicking, so a caller — startup, or the
    /// settings API validating a `PATCH` — can fall back or reject instead of
    /// bringing the whole server down over one bad field.
    pub(crate) fn build_parts(&self) -> Result<(Arc<dyn AgentBrain>, LoopConfig, bool), String> {
        if !self.enabled {
            return Ok((Arc::new(RuleBrain::new()), LoopConfig::default(), true));
        }
        let (Some(base_url), Some(model)) = (self.base_url.as_deref(), self.model.as_deref())
        else {
            return Err("enabled is true but base_url and model are not both set".to_string());
        };
        let brain = LlmBrain::new(LlmConfig {
            base_url,
            model,
            api_key: self.api_key.as_deref().unwrap_or(""),
            max_tokens: self.max_tokens,
            summary_max_tokens: self.compress_max_tokens,
            max_rpm: self.max_rpm,
            max_retries: self.max_retries,
            retry_base_ms: self.retry_base_ms,
        })?;
        let config = LoopConfig {
            compress_after: self.compress_after,
            compress_keep: self.compress_keep,
            ..LoopConfig::default()
        };
        Ok((Arc::new(brain), config, false))
    }

    /// Builds a fresh, standalone [`AgentRuntime`] from this configuration. Used
    /// at startup; a live update instead calls [`Self::build_parts`] directly
    /// and swaps the pieces into the existing runtime (see
    /// `AppState::apply_llm_settings`), so an in-flight turn is never split
    /// across two runtimes.
    pub fn build_runtime(&self) -> Result<AgentRuntime, String> {
        let (brain, config, mock) = self.build_parts()?;
        Ok(AgentRuntime::new(brain, config, mock))
    }
}

/// A handle to `llm.toml` under the data directory. Independent of
/// `AGORALUME_PERSIST` / [`crate::persist::Persistence`] — losing the LLM
/// endpoint and key on a throwaway mock run would defeat the point of moving
/// off environment variables, so this file always exists once the server has
/// started once, whether or not chat history is being kept.
pub struct LlmConfigStore {
    path: PathBuf,
}

impl LlmConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join("llm.toml"),
        }
    }

    /// Loads the configuration, in three cases:
    /// - **file exists and parses** — returned as-is.
    /// - **file exists but is corrupt** — quarantined (moved aside, never
    ///   discarded) and the defaults (mock mode) are returned.
    /// - **no file yet** — a one-time migration from any `AGORALUME_LLM*` /
    ///   `AGORALUME_COMPRESS_*` environment variables still set (so an existing
    ///   `.env` setup doesn't silently drop to mock mode on upgrade), or, absent
    ///   those, a commented scaffold the user can edit by hand. Either way the
    ///   result is written to `llm.toml` and those variables are never read
    ///   again after this call.
    pub fn load(&self) -> LlmSettings {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match toml::from_str::<LlmSettings>(&text) {
                Ok(settings) => settings,
                Err(e) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        error = %e,
                        "llm.toml is unreadable; moving it aside and starting from defaults (mock mode)"
                    );
                    quarantine(&self.path);
                    LlmSettings::default()
                }
            },
            Err(_) => match migrate_from_env() {
                Some(settings) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        "migrated AGORALUME_LLM* / AGORALUME_COMPRESS_* environment variables into \
                         llm.toml on first run; those variables are no longer read — remove them \
                         from your .env or shell, and edit llm.toml (or the Settings page) instead"
                    );
                    self.write(&render(&settings, MIGRATED_HEADER));
                    settings
                }
                None => {
                    self.write(SCAFFOLD);
                    LlmSettings::default()
                }
            },
        }
    }

    /// Writes the configuration out. Called after every successful
    /// `PATCH /llm/settings`; failures are logged, not fatal — a disk hiccup
    /// must not break the (already-applied, in-memory) live update.
    pub fn save(&self, settings: &LlmSettings) {
        self.write(&render(settings, SAVED_HEADER));
    }

    fn write(&self, contents: &str) {
        if let Err(e) = write_atomic(&self.path, contents) {
            tracing::warn!(path = %self.path.display(), error = %e, "failed to write llm.toml");
        }
    }
}

const MIGRATED_HEADER: &str = "# Migrated automatically from AGORALUME_LLM* / AGORALUME_COMPRESS_* environment\n\
# variables. Those are no longer read — edit this file directly, or use the\n\
# Settings page in the running app (either one writes here). Keep this file\n\
# out of version control if it holds a real key (already covered by\n\
# backend/.gitignore's /data/* rule).\n";

const SAVED_HEADER: &str = "# Written by the Settings page. Hand edits work too, but only take effect on\n\
# the next restart (the running server doesn't watch this file). Keep this\n\
# file out of version control if it holds a real key (already covered by\n\
# backend/.gitignore's /data/* rule).\n";

/// Renders a settings snapshot as TOML with an explanatory header comment.
fn render(settings: &LlmSettings, header: &str) -> String {
    format!(
        "{header}\n{}",
        toml::to_string_pretty(settings).unwrap_or_default()
    )
}

/// The commented, all-defaults template written the first time the server
/// starts with no prior `.env` LLM configuration — the "settings file good
/// enough to hand-edit" the file format exists for. Every line is a comment so
/// it parses to [`LlmSettings::default`] (mock mode) until uncommented.
const SCAFFOLD: &str = r#"# AgoraLume LLM configuration.
#
# Replaces the old AGORALUME_LLM* / AGORALUME_COMPRESS_* environment variables.
# Edit this file directly, or use the Settings page in the running app —
# either one writes here. Every line is optional; commented lines show the
# default. This file can hold a real API key: keep it out of version control
# (already covered by backend/.gitignore's /data/* rule) and don't share it.

# Off by default (simulated replies). Set true to drive agents with a real
# OpenAI-compatible model, then fill in base_url + model below.
#enabled = true

# OpenAI-compatible API root. Examples:
#   https://api.openai.com/v1     (OpenAI)
#   https://openrouter.ai/api/v1  (OpenRouter)
#   http://localhost:11434/v1     (Ollama, local, no key)
#   https://generativelanguage.googleapis.com/v1beta/openai/  (Gemini — auto-
#     detected and routed to the native Gemini provider so thought signatures
#     round-trip; every other URL is treated as plain OpenAI-compatible)
#base_url = "http://localhost:11434/v1"

# Model to request. Nothing is hard-coded — you choose it.
#model = "llama3.1"

# Bearer key. Leave unset for local endpoints that need no auth. The backend
# never sends this back to a client once set.
#api_key = "sk-..."

# Max tokens per reply (default 512).
#max_tokens = 512

# Server-wide cap on LLM requests per rolling minute, so a free-tier quota
# isn't blown (default 15, matching Gemini's free flash-lite tier). 0 disables
# throttling, higher makes sense on a paid tier.
#max_rpm = 15

# Automatic retries on a transient failure (429 / 5xx / transport) before the
# error surfaces to the chat (default 2). 0 disables auto-retry.
#max_retries = 2

# Base backoff before the first retry, in ms; each further retry doubles it
# (default 1000). A server-supplied hint (e.g. a 429's retryDelay) wins over it.
#retry_base_ms = 1000

# --- Context compression ---------------------------------------------------
# Older conversation folds into a running per-group summary so the prompt
# every agent reads stays bounded. Only runs with a real model.
#compress_after = 50
#compress_keep = 20
#compress_max_tokens = 1024

# --- Cost estimate (Settings page) ------------------------------------------
# Optional token pricing, per 1,000,000 tokens, to show an estimated cost
# (always "for reference only"). Set inputPerM + outputPerM; cachedInputPerM
# defaults to inputPerM when omitted, so a discounted cache rate is what
# reveals the savings.
#[pricing]
#inputPerM = 0.15
#outputPerM = 0.60
#cachedInputPerM = 0.0375
#currency = "USD"
"#;

/// One-time migration from the environment variables this file replaces —
/// read only when `llm.toml` doesn't exist yet. `None` when nothing relevant is
/// set, in which case [`LlmConfigStore::load`] seeds the commented scaffold
/// instead of a real (if empty) configuration.
fn migrate_from_env() -> Option<LlmSettings> {
    let enabled = env_flag("AGORALUME_LLM");
    let base_url = env_nonempty("AGORALUME_LLM_BASE_URL");
    let model = env_nonempty("AGORALUME_LLM_MODEL");
    let api_key = env_nonempty("AGORALUME_LLM_API_KEY");
    if !enabled && base_url.is_none() && model.is_none() && api_key.is_none() {
        return None;
    }
    let mut settings = LlmSettings {
        enabled,
        base_url,
        model,
        api_key,
        ..LlmSettings::default()
    };
    if let Some(v) = env_parse("AGORALUME_LLM_MAX_TOKENS") {
        settings.max_tokens = v;
    }
    if let Some(v) = env_parse("AGORALUME_LLM_MAX_RPM") {
        settings.max_rpm = v;
    }
    if let Some(v) = env_parse("AGORALUME_LLM_MAX_RETRIES") {
        settings.max_retries = v;
    }
    if let Some(v) = env_parse("AGORALUME_LLM_RETRY_BASE_MS") {
        settings.retry_base_ms = v;
    }
    if let Some(v) = env_parse("AGORALUME_COMPRESS_AFTER") {
        settings.compress_after = v;
    }
    if let Some(v) = env_parse("AGORALUME_COMPRESS_KEEP") {
        settings.compress_keep = v;
    }
    if let Some(v) = env_parse("AGORALUME_COMPRESS_MAX_TOKENS") {
        settings.compress_max_tokens = v;
    }
    let input = env_parse::<f64>("AGORALUME_LLM_PRICE_INPUT");
    let output = env_parse::<f64>("AGORALUME_LLM_PRICE_OUTPUT");
    if input.is_some() || output.is_some() {
        let input_per_m = input.unwrap_or(0.0);
        settings.pricing = Some(Pricing {
            input_per_m,
            cached_input_per_m: env_parse("AGORALUME_LLM_PRICE_CACHED_INPUT")
                .unwrap_or(input_per_m),
            output_per_m: output.unwrap_or(0.0),
            currency: env_nonempty("AGORALUME_LLM_PRICE_CURRENCY")
                .unwrap_or_else(|| "USD".to_string()),
        });
    }
    Some(settings)
}

/// Reads an environment variable, treating blank/whitespace as unset.
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Reads and parses an environment variable, treating blank or unparsable as
/// unset.
fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env_nonempty(name).and_then(|v| v.parse().ok())
}

/// Reads a boolean environment flag: `1`/`true`/`yes`/`on` (any case) is true,
/// anything else (including unset) is false.
fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Writes `contents` atomically: a sibling temp file is written and flushed,
/// then renamed over the target, so a reader never sees a partial file.
/// Creates the parent directory on first use. Mirrors `persist::write_atomic`,
/// kept separate since this file is plain TOML text, not serialized JSON.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Moves an unreadable file aside — appending `.corrupt-<epoch>` — instead of
/// letting the next save overwrite it, so its data is preserved for manual
/// recovery. Best-effort: a rename failure is logged and swallowed (the caller
/// falls back to defaults regardless). Mirrors `persist::quarantine`.
fn quarantine(path: &Path) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut dest = path.as_os_str().to_owned();
    dest.push(format!(".corrupt-{stamp}"));
    let dest = PathBuf::from(dest);
    match std::fs::rename(path, &dest) {
        Ok(()) => {
            tracing::warn!(from = %path.display(), to = %dest.display(), "preserved an unreadable llm.toml");
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not preserve an unreadable llm.toml");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agoralume-llm-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn missing_file_seeds_a_scaffold_and_returns_defaults() {
        let dir = temp_dir();
        let store = LlmConfigStore::new(&dir);

        let settings = store.load();

        assert!(!settings.enabled);
        assert_eq!(settings.max_tokens, 512);
        let on_disk = std::fs::read_to_string(dir.join("llm.toml")).expect("scaffold written");
        assert!(
            on_disk.contains("#enabled = true"),
            "scaffold is commented out"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn round_trips_through_save_and_load() {
        let dir = temp_dir();
        let store = LlmConfigStore::new(&dir);

        let settings = LlmSettings {
            enabled: true,
            base_url: Some("http://localhost:11434/v1".to_string()),
            model: Some("llama3.1".to_string()),
            api_key: Some("sk-secret".to_string()),
            pricing: Some(Pricing {
                input_per_m: 0.15,
                cached_input_per_m: 0.0375,
                output_per_m: 0.60,
                currency: "USD".to_string(),
            }),
            ..LlmSettings::default()
        };
        store.save(&settings);

        let loaded = store.load();
        assert!(loaded.enabled);
        assert_eq!(
            loaded.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(loaded.api_key.as_deref(), Some("sk-secret"));
        assert_eq!(loaded.pricing.unwrap().input_per_m, 0.15);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_is_quarantined_not_fatal() {
        let dir = temp_dir();
        let store = LlmConfigStore::new(&dir);
        std::fs::write(dir.join("llm.toml"), b"not valid = [toml").unwrap();

        let settings = store.load();

        assert!(!settings.enabled, "falls back to defaults");
        assert!(
            !dir.join("llm.toml").exists(),
            "the corrupt file was moved aside"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_partial_file_fills_missing_fields_with_defaults() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("llm.toml"),
            b"enabled = true\nmodel = \"llama3.1\"\n",
        )
        .unwrap();
        let store = LlmConfigStore::new(&dir);

        let settings = store.load();

        assert!(settings.enabled);
        assert_eq!(settings.model.as_deref(), Some("llama3.1"));
        assert_eq!(
            settings.max_tokens, 512,
            "missing field falls back to the struct default"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn disabled_settings_build_the_mock() {
        let settings = LlmSettings::default();
        let (_, _, mock) = settings.build_parts().expect("mock always builds");
        assert!(mock);
    }

    #[test]
    fn enabled_without_base_url_fails_validation_instead_of_panicking() {
        let settings = LlmSettings {
            enabled: true,
            model: Some("llama3.1".into()),
            ..LlmSettings::default()
        };
        assert!(settings.build_parts().is_err());
    }

    #[test]
    fn enabled_with_endpoint_builds_a_real_brain() {
        let settings = LlmSettings {
            enabled: true,
            base_url: Some("http://localhost:11434/v1".to_string()),
            model: Some("llama3.1".to_string()),
            ..LlmSettings::default()
        };
        let (_, _, mock) = settings.build_parts().expect("a valid endpoint builds");
        assert!(!mock);
    }
}
