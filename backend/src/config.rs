//! Server-level runtime configuration, read from the environment: where to
//! bind, where data lives, and whether to persist it. LLM provider
//! configuration (endpoint, key, tuning, pricing) lives separately, in
//! `llm.toml` — see [`crate::llm_config`] — since it needs to be editable
//! without a restart and shouldn't round-trip a key through a client.

use std::net::SocketAddr;

pub struct Config {
    /// Address the HTTP server binds to.
    pub bind: SocketAddr,
    /// Directory for on-disk state (`workspace.json`, `messages/`, `usage.json`,
    /// `llm.toml`), read from `AGORALUME_DATA_DIR`.
    pub data_dir: String,
    /// Explicit override for whether the workspace and chat logs persist to
    /// `data_dir` (`AGORALUME_PERSIST`). `None` means "use the default", which
    /// `main` resolves once `llm.toml` is loaded — a real-model run defaults to
    /// persisted, a mock run to in-memory — since persistence and the LLM are
    /// otherwise independent facts (either can still override explicitly).
    /// `llm.toml` itself always persists regardless: it's the reason a real
    /// model's API key doesn't need this to survive a restart.
    pub persist_override: Option<bool>,
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
    /// of exporting environment variables by hand. Looks beside the executable
    /// first (the bundle layout: `exe` + `.env` + `web/`), then falls back to
    /// the working directory (handy in development). Variables already present
    /// in the real environment always win — the file only fills gaps — and a
    /// missing file is not an error. Call this before reading any config.
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
        let persist_override = env_flag_opt("AGORALUME_PERSIST");
        let web_dir = std::env::var("AGORALUME_WEB_DIR").ok();
        // Default on, so double-clicking the bundle "just works"; only an
        // explicit unset-like value opts out.
        let open_browser = std::env::var("AGORALUME_OPEN")
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true);
        Self {
            bind,
            data_dir,
            persist_override,
            web_dir,
            open_browser,
        }
    }
}

/// Reads a tri-state boolean flag: `None` when unset, else `Some(true)` for
/// `1`/`true`/`yes`/`on` and `Some(false)` for anything else. Lets a caller tell
/// "left at its default" apart from an explicit off.
fn env_flag_opt(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}
