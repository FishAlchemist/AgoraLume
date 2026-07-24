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
    /// opt in. No adapter is wired yet, so opting in fails fast at startup.
    pub llm: bool,
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
        Self { bind, data_dir, llm: env_flag("AGORALUME_LLM"), web_dir, open_browser }
    }
}

/// Reads a boolean environment flag. Absent or an unset-like value is false;
/// `1`/`true`/`yes`/`on` (any case) is true.
fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
    })
}
