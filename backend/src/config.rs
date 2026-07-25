//! Runtime configuration, read from the environment.

use std::net::SocketAddr;

use crate::models::Cost;

/// Token pricing used to turn usage into an estimated cost. Rates are per
/// 1,000,000 tokens, in `currency`. A rough operator-supplied estimate — models
/// and providers differ — so the UI always labels the result "for reference".
#[derive(Clone, Debug)]
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

pub struct Config {
    /// Address the HTTP server binds to.
    pub bind: SocketAddr,
    /// Directory for on-disk persistence (`workspace.json` + `messages/`), read
    /// from `AGORALUME_DATA_DIR`. Only used when `persist` is on.
    pub data_dir: String,
    /// Whether to persist the workspace and chat logs to `data_dir` so they
    /// survive a restart. Read from `AGORALUME_PERSIST`; when unset it defaults
    /// to `llm` — a real-model run keeps its data, a throwaway mock run doesn't.
    /// Persistence and the LLM are independent facts, so either can be forced.
    pub persist: bool,
    /// Whether to leave mock mode and drive agents with a real LLM. Off by
    /// default so a plain run never spends API budget; set `AGORALUME_LLM` to
    /// opt in. When on, the `llm_*` fields below configure the OpenAI-compatible
    /// endpoint; a missing base URL or model fails fast at startup.
    pub llm: bool,
    /// OpenAI-compatible API root, e.g. `https://api.openai.com/v1` or a local
    /// `http://localhost:11434/v1` (Ollama). Read from `AGORALUME_LLM_BASE_URL`.
    pub llm_base_url: Option<String>,
    /// Model name to request, e.g. `gpt-4o-mini` or `llama3.1`. Read from
    /// `AGORALUME_LLM_MODEL`. Nothing is hard-coded — you choose the model.
    pub llm_model: Option<String>,
    /// Bearer key for the endpoint. Read from `AGORALUME_LLM_API_KEY`. Optional:
    /// local endpoints (Ollama, llama.cpp) usually need no key.
    pub llm_api_key: Option<String>,
    /// Upper bound on tokens per reply. Read from `AGORALUME_LLM_MAX_TOKENS`;
    /// defaults to 512 — enough for a chat turn without runaway cost.
    pub llm_max_tokens: u64,
    /// Server-wide cap on LLM requests per rolling minute, so a free-tier quota
    /// isn't blown. Read from `AGORALUME_LLM_MAX_RPM`; defaults to 15 (Gemini's
    /// free `flash-lite` tier). `0` disables throttling — set it higher on a
    /// paid tier. Agents that would exceed the cap simply wait their turn.
    pub llm_max_rpm: u64,
    /// Optional token pricing for the estimated-cost readout. When unset, the
    /// debug panel shows token counts only. Rates are per 1,000,000 tokens.
    pub pricing: Option<Pricing>,
    /// Explicit path to the built frontend to serve. Normally left unset — the
    /// bundle ships the SPA in a `web/` directory next to the executable, which
    /// is discovered automatically. Set `AGORALUME_WEB_DIR` to override.
    pub web_dir: Option<String>,
    /// Whether to open the site in a browser once the server is up. Only acts
    /// when the SPA is actually being served (bundle mode); a plain API run
    /// never launches a browser. On by default; set `AGORALUME_OPEN=0` to skip.
    pub open_browser: bool,
}

impl Config {
    /// Loads a `.env` file so bundle users can drop settings in a file instead
    /// of exporting a dozen environment variables by hand. Looks beside the
    /// executable first (the bundle layout: `exe` + `.env` + `web/`), then falls
    /// back to the working directory (handy in development). Variables already
    /// present in the real environment always win — the file only fills gaps —
    /// and a missing file is not an error. Call this before reading any config.
    pub fn load_dotenv() {
        if let Some(beside_exe) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(".env")))
            && dotenvy::from_path(&beside_exe).is_ok()
        {
            return;
        }
        // Dev fallback: `.env` in (or above) the working directory.
        let _ = dotenvy::dotenv();
    }

    pub fn from_env() -> Self {
        let bind = std::env::var("AGORALUME_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let bind = bind
            .parse()
            .unwrap_or_else(|_| panic!("invalid AGORALUME_BIND `{bind}` (want host:port)"));
        let data_dir = std::env::var("AGORALUME_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
        let llm = env_flag("AGORALUME_LLM");
        // Default persistence to whether a real model is driving: a mock run is
        // throwaway, an LLM run is worth keeping. Either can override explicitly.
        let persist = env_flag_opt("AGORALUME_PERSIST").unwrap_or(llm);
        let web_dir = std::env::var("AGORALUME_WEB_DIR").ok();
        // Default on, so double-clicking the bundle "just works"; only an
        // explicit unset-like value opts out.
        let open_browser = std::env::var("AGORALUME_OPEN")
            .map(|v| {
                !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")
            })
            .unwrap_or(true);
        let llm_max_tokens = std::env::var("AGORALUME_LLM_MAX_TOKENS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(512);
        let llm_max_rpm = std::env::var("AGORALUME_LLM_MAX_RPM")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(15);
        Self {
            bind,
            data_dir,
            persist,
            llm,
            llm_base_url: env_nonempty("AGORALUME_LLM_BASE_URL"),
            llm_model: env_nonempty("AGORALUME_LLM_MODEL"),
            llm_api_key: env_nonempty("AGORALUME_LLM_API_KEY"),
            llm_max_tokens,
            llm_max_rpm,
            pricing: read_pricing(),
            web_dir,
            open_browser,
        }
    }
}

/// Builds the optional [`Pricing`] from the environment. Present when either the
/// input or output rate is set; the cached-input rate defaults to the input rate
/// so cost is never under-reported before a discounted cache rate is supplied.
fn read_pricing() -> Option<Pricing> {
    let input = env_f64("AGORALUME_LLM_PRICE_INPUT");
    let output = env_f64("AGORALUME_LLM_PRICE_OUTPUT");
    if input.is_none() && output.is_none() {
        return None;
    }
    let input_per_m = input.unwrap_or(0.0);
    Some(Pricing {
        input_per_m,
        cached_input_per_m: env_f64("AGORALUME_LLM_PRICE_CACHED_INPUT").unwrap_or(input_per_m),
        output_per_m: output.unwrap_or(0.0),
        currency: env_nonempty("AGORALUME_LLM_PRICE_CURRENCY").unwrap_or_else(|| "USD".to_string()),
    })
}

/// Reads a floating-point environment variable, treating blank as unset.
fn env_f64(name: &str) -> Option<f64> {
    env_nonempty(name).and_then(|v| v.parse().ok())
}

/// Reads an environment variable, treating blank/whitespace as unset.
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Reads a boolean environment flag. Absent or an unset-like value is false;
/// `1`/`true`/`yes`/`on` (any case) is true.
fn env_flag(name: &str) -> bool {
    env_flag_opt(name).unwrap_or(false)
}

/// Reads a tri-state boolean flag: `None` when unset, else `Some(true)` for
/// `1`/`true`/`yes`/`on` and `Some(false)` for anything else. Lets a caller tell
/// "left at its default" apart from an explicit off.
fn env_flag_opt(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}
