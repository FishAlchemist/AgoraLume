//! Runtime configuration, read from the environment.

use std::net::SocketAddr;

pub struct Config {
    /// Address the HTTP server binds to.
    pub bind: SocketAddr,
    /// Where runtime state will live once persistence exists. Reserved for now —
    /// this build keeps everything in memory — but honoured so deployment
    /// configs can be wired up ahead of time.
    pub data_dir: String,
}

impl Config {
    pub fn from_env() -> Self {
        let bind = std::env::var("AGORALUME_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let bind = bind
            .parse()
            .unwrap_or_else(|_| panic!("invalid AGORALUME_BIND `{bind}` (want host:port)"));
        let data_dir = std::env::var("AGORALUME_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
        Self { bind, data_dir }
    }
}
