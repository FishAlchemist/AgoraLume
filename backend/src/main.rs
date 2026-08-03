//! AgoraLume backend — the API server.
//!
//! An axum server exposing the production AgoraLume HTTP + SSE API (the same
//! contract the frontend's `HttpChatApi` targets). The API is real; only the
//! data behind it is provisional in this build — an in-memory store and
//! simulated agent replies stand in until real persistence and an LLM are
//! connected. Nothing about the API surface changes when they are.
//!
//! Point the frontend at it with `VITE_API_BASE_URL=http://127.0.0.1:8080`.

mod agent;
mod api_error;
mod auth;
mod config;
mod llm_config;
mod models;
mod persist;
mod routes;
mod state;
mod workspace;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

use crate::agent::turn::AgentRuntime;
use crate::auth::AdminConfigStore;
use crate::config::Config;
use crate::llm_config::LlmConfigStore;
use crate::state::{AppState, OperatorState};

#[tokio::main]
async fn main() {
    // One-shot mode: `--dump-openapi [path]` writes the OpenAPI document (YAML)
    // and exits. The frontend's type generation reads this file. Handled before
    // any server setup so it needs nothing running.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--dump-openapi") {
        let path = args.next().unwrap_or_else(|| "openapi.yml".to_string());
        let yaml = routes::openapi()
            .to_yaml()
            .expect("serialize OpenAPI document to YAML");
        std::fs::write(&path, yaml).unwrap_or_else(|e| panic!("failed to write {path}: {e}"));
        println!("wrote {path}");
        return;
    }

    // Load a `.env` beside the executable (or the working dir in dev) before
    // anything reads the environment, so bundle users configure with a file
    // instead of a dozen shell exports. Real env vars still take precedence.
    Config::load_dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let config = Config::from_env();

    // The LLM provider configuration lives in `llm.toml` under the data dir now,
    // not environment variables — see `llm_config` for why (hand-editable,
    // hot-reloadable, and the one place that knows to never echo the API key
    // back to a client). Always loaded (and, on first run, seeded) regardless of
    // whether chat history persists below: losing a configured endpoint and key
    // on a throwaway mock run would defeat the point of moving off env vars.
    let llm_store = LlmConfigStore::new(&config.data_dir);
    let llm_settings = llm_store.load();

    // Building the runtime can fail (e.g. `enabled = true` with an incomplete
    // endpoint, or a client that fails to construct) without taking the whole
    // server down over one bad field: fall back to mock replies and say why, so
    // the operator can fix it from the Settings page (applies immediately) or
    // the file (takes effect on the next restart) rather than staring at a
    // process that just exited.
    let runtime = match llm_settings.build_runtime() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "llm.toml has `enabled = true` but couldn't build a working LLM brain; \
                 starting in mock mode instead"
            );
            AgentRuntime::mock()
        }
    };

    // Persistence is optional: on, the workspace and chat logs live under the
    // data dir and survive a restart; off, everything is in-memory. Defaults to
    // whether a real model is configured — a real run is worth keeping, a mock
    // demo is throwaway — but either can be forced via `AGORALUME_PERSIST`.
    let persist = config.persist_override.unwrap_or(llm_settings.enabled);
    let operator = Arc::new(OperatorState::new(runtime).with_llm_config(llm_settings, llm_store));
    let state = if persist {
        // One-time upgrade from the pre-account flat layout, if this data dir
        // still has one — a no-op on every later run once nothing legacy is
        // left. See `persist::migrate_legacy_layout`.
        let account_dir = config.account_data_dir();
        persist::migrate_legacy_layout(Path::new(&config.data_dir), &account_dir);
        AppState::new(operator, Some(PathBuf::from(&config.data_dir)), &config.account_id)
    } else {
        AppState::new(operator, None, &config.account_id)
    };
    // The admin's password hash, like `llm.toml`, always loads regardless of
    // whether accounts persist — the admin role isn't an account, so its
    // identity doesn't follow that flag either. `None` (no admin.json, or one
    // without a fixed password yet) makes `with_admin_auth` generate and log
    // a fresh one for this boot.
    let admin_password_hash = AdminConfigStore::new(&config.data_dir).load_password_hash();
    let state = state.with_admin_auth(admin_password_hash, config.auth_disabled);
    let state = Arc::new(state);
    let app = routes::router(state.clone(), config.cors_allowed_origins.as_deref());

    // Bundle mode: if a built frontend sits next to us, serve it from the same
    // origin as the API so one executable is the whole site. Unknown paths fall
    // back to index.html for the SPA's client-side routes. A plain `cargo run`
    // has no `web/` dir, so this is skipped and the server is API-only.
    let web_dir = resolve_web_dir(&config);
    let app = match &web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            // `.fallback` (not `.not_found_service`) serves index.html with its
            // natural 200 for unknown paths, so a hard refresh on a client-side
            // route loads the SPA instead of a 404.
            let serve = ServeDir::new(dir).fallback(ServeFile::new(index));
            app.fallback_service(serve)
        }
        None => app,
    };

    // Prefer the configured address; if it's taken, fall back to an OS-assigned
    // port so double-clicking the bundle never dies on a port clash.
    let listener = match TcpListener::bind(config.bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                address = %config.bind,
                error = %e,
                "could not bind the configured address; falling back to an OS-assigned port"
            );
            let fallback = SocketAddr::new(config.bind.ip(), 0);
            TcpListener::bind(fallback)
                .await
                .unwrap_or_else(|e| panic!("failed to bind fallback port: {e}"))
        }
    };
    let addr = listener.local_addr().unwrap_or(config.bind);

    // A wildcard bind (0.0.0.0) isn't a browsable host; point the browser and
    // the log at loopback in that case.
    let host = if addr.ip().is_unspecified() {
        std::net::Ipv4Addr::LOCALHOST.to_string()
    } else {
        addr.ip().to_string()
    };
    let url = format!("http://{host}:{}", addr.port());

    // Reflect the actual reply source: a real model, or the rule-based mock.
    let replies = if state.runtime().is_mock() {
        "simulated replies (mock)"
    } else {
        "LLM-backed replies"
    };
    let store = if persist {
        "persisted store"
    } else {
        "in-memory store"
    };
    tracing::info!(
        %url,
        data_dir = %config.data_dir,
        account_id = %config.account_id,
        persist,
        serving_web = web_dir.is_some(),
        "AgoraLume backend listening ({store}; {replies})"
    );

    // Only pop a browser when we're actually the site (bundle mode) and the
    // operator hasn't opted out. `webbrowser::open` blocks, so keep it off the
    // async runtime's worker.
    if web_dir.is_some() && config.open_browser {
        let url = url.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = webbrowser::open(&url) {
                tracing::warn!(error = %e, "could not open a browser; open {url} manually");
            }
        });
    }

    axum::serve(listener, app).await.expect("server error");
}

/// Finds the built frontend to serve, or `None` to run API-only.
///
/// Checks, in order: an explicit `AGORALUME_WEB_DIR`, a `web/` directory beside
/// the executable (the bundle layout), then `web/` in the working directory.
fn resolve_web_dir(config: &Config) -> Option<PathBuf> {
    if let Some(dir) = &config.web_dir {
        let path = PathBuf::from(dir);
        return path.is_dir().then_some(path);
    }
    if let Some(beside_exe) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("web")))
        && beside_exe.is_dir()
    {
        return Some(beside_exe);
    }
    let cwd = PathBuf::from("web");
    cwd.is_dir().then_some(cwd)
}
