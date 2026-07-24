//! Runtime configuration, read from the environment.

use std::net::SocketAddr;

pub struct Config {
    /// Address the HTTP server binds to.
    pub bind: SocketAddr,
    /// Where runtime state will live once persistence exists. Reserved for now —
    /// this build keeps everything in memory — but honoured so deployment
    /// configs can be wired up ahead of time.
    pub data_dir: String,
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
        Self {
            bind,
            data_dir,
            llm: env_flag("AGORALUME_LLM"),
            llm_base_url: env_nonempty("AGORALUME_LLM_BASE_URL"),
            llm_model: env_nonempty("AGORALUME_LLM_MODEL"),
            llm_api_key: env_nonempty("AGORALUME_LLM_API_KEY"),
            llm_max_tokens,
            web_dir,
            open_browser,
        }
    }
}

/// Reads an environment variable, treating blank/whitespace as unset.
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Reads a boolean environment flag. Absent or an unset-like value is false;
/// `1`/`true`/`yes`/`on` (any case) is true.
fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
    })
}
