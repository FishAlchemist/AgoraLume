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
mod config;
mod models;
mod routes;
mod state;
mod workspace;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

use crate::agent::turn::AgentRuntime;
use crate::config::Config;
use crate::state::AppState;

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

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let config = Config::from_env();

    // Pick the agent runtime the config asks for. Mock is the default so a plain
    // run never spends API budget; `AGORALUME_LLM` opts out of it. No LLM adapter
    // is wired yet, so opting in fails fast rather than silently running the mock.
    let runtime = if config.llm {
        eprintln!(
            "AGORALUME_LLM is set, but no LLM adapter is wired yet. \
             Unset it to run the default mock brain."
        );
        std::process::exit(1);
    } else {
        AgentRuntime::mock()
    };
    let state = Arc::new(AppState::with_runtime(runtime));
    let app = routes::router(state);

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

    tracing::info!(
        %url,
        data_dir = %config.data_dir,
        serving_web = web_dir.is_some(),
        "AgoraLume backend listening (in-memory store; simulated replies, no LLM yet)"
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
