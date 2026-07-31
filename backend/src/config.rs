//! Server-level runtime configuration, read from the environment: where to
//! bind, where data lives, and whether to persist it. LLM provider
//! configuration (endpoint, key, tuning, pricing) lives separately, in
//! `llm.toml` — see [`crate::llm_config`] — since it needs to be editable
//! without a restart and shouldn't round-trip a key through a client.

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::persist;

/// The account served when nothing overrides `AGORALUME_ACCOUNT_ID`. There's
/// no create-account flow yet — every install effectively has exactly this
/// one account — so a fixed name is fine for now; see
/// [`Config::account_data_dir`].
const DEFAULT_ACCOUNT_ID: &str = "default";

pub struct Config {
    /// Address the HTTP server binds to.
    pub bind: SocketAddr,
    /// Root directory for on-disk state, read from `AGORALUME_DATA_DIR`.
    /// `llm.toml` lives directly under this root (shared, operator-level);
    /// everything else lives under this account's own subtree — see
    /// [`Self::account_data_dir`].
    pub data_dir: String,
    /// Which account's data this run serves, from `AGORALUME_ACCOUNT_ID`
    /// (default `"default"`). The backend can already keep several accounts'
    /// data apart on disk, but there's no login yet to choose between them
    /// per request — one process still serves exactly one account, picked
    /// here at startup.
    pub account_id: String,
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
    /// Origins allowed to make cross-origin requests, from a comma-separated
    /// `AGORALUME_CORS_ORIGINS` (e.g. `http://localhost:5173,https://chat.example.com`).
    /// `None` (unset) keeps the permissive default — any origin — since the
    /// frontend's origin isn't fixed (a dev server on some port, or wherever
    /// the SPA ends up hosted) and most setups have no fixed origin to name.
    /// An operator who does know theirs can lock this down without rebuilding
    /// the binary; see `routes::router` for what this buys against a request
    /// like `PATCH /llm/settings` from an unrelated page in the same browser.
    pub cors_allowed_origins: Option<Vec<String>>,
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
        let account_id =
            std::env::var("AGORALUME_ACCOUNT_ID").unwrap_or_else(|_| DEFAULT_ACCOUNT_ID.to_string());
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
        let cors_allowed_origins = env_csv_opt("AGORALUME_CORS_ORIGINS");
        Self {
            bind,
            data_dir,
            account_id,
            persist_override,
            web_dir,
            open_browser,
            cors_allowed_origins,
        }
    }

    /// This account's own subtree under `data_dir`:
    /// `<data_dir>/accounts/<sanitized account_id>`. The only thing that
    /// currently reads this is `main`, to point `Persistence` at it —
    /// `llm.toml` stays at `data_dir`'s root regardless of account.
    pub fn account_data_dir(&self) -> PathBuf {
        PathBuf::from(&self.data_dir).join("accounts").join(persist::sanitize(&self.account_id))
    }
}

/// Reads a comma-separated list, trimming whitespace and dropping empty
/// entries. `None` when the variable is unset or every entry was empty.
fn env_csv_opt(name: &str) -> Option<Vec<String>> {
    let raw = std::env::var(name).ok()?;
    let items = parse_csv(&raw);
    if items.is_empty() { None } else { Some(items) }
}

/// Splits a comma-separated string, trimming whitespace and dropping empty
/// entries. Pulled out of [`env_csv_opt`] so the parsing itself is testable
/// without touching process-wide environment state.
fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_trims_and_drops_blanks() {
        let got = parse_csv(" http://localhost:5173 ,, https://chat.example.com ,");
        assert_eq!(
            got,
            vec!["http://localhost:5173", "https://chat.example.com"]
        );
    }

    #[test]
    fn parse_csv_all_blank_is_empty() {
        assert!(parse_csv(" , , ").is_empty());
        assert!(parse_csv("").is_empty());
    }

    fn config_with(data_dir: &str, account_id: &str) -> Config {
        Config {
            bind: "127.0.0.1:8080".parse().unwrap(),
            data_dir: data_dir.to_string(),
            account_id: account_id.to_string(),
            persist_override: None,
            web_dir: None,
            open_browser: false,
            cors_allowed_origins: None,
        }
    }

    #[test]
    fn account_data_dir_nests_the_account_under_the_data_root() {
        let config = config_with("./data", "default");
        assert_eq!(config.account_data_dir(), PathBuf::from("./data/accounts/default"));
    }

    #[test]
    fn account_data_dir_sanitizes_the_account_id() {
        let config = config_with("./data", "../evil");
        assert_eq!(config.account_data_dir(), PathBuf::from("./data/accounts/___evil"));
    }
}
