//! Optional on-disk persistence for the workspace and per-group chat logs.
//!
//! Layout under the data directory (`AGORALUME_DATA_DIR`):
//!
//! ```text
//! workspace.json             the whole editable workspace (small; loaded at startup)
//! messages/<group_id>.json   one group's chat log (loaded lazily, saved on change)
//! summaries/<group_id>.json  one group's compressed older history (loaded with its log)
//! ```
//!
//! Splitting the message logs off the workspace means a running server only
//! reads a group's history the first time that group is touched — the "read on
//! demand" the design calls for — instead of loading everything up front.
//!
//! Everything degrades safely: a missing or corrupt file falls back to a seed
//! (or an empty log) rather than crashing, and writes are atomic (temp file +
//! rename) so a crash mid-write can never leave a half-written file behind.

use std::io;
use std::path::{Path, PathBuf};

use crate::models::Message;
use crate::state::GroupSummary;
use crate::workspace::WorkspaceSnapshot;

/// A handle to the data directory. Held by [`crate::state::AppState`] when
/// persistence is enabled; absent (`None`) for a pure in-memory run.
pub struct Persistence {
    dir: PathBuf,
}

impl Persistence {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn workspace_path(&self) -> PathBuf {
        self.dir.join("workspace.json")
    }

    fn message_path(&self, group_id: &str) -> PathBuf {
        self.dir.join("messages").join(format!("{}.json", sanitize(group_id)))
    }

    fn summary_path(&self, group_id: &str) -> PathBuf {
        self.dir.join("summaries").join(format!("{}.json", sanitize(group_id)))
    }

    /// Loads the persisted workspace, or `None` when there is no saved file yet
    /// (so the caller seeds a fresh one). A corrupt file is logged and treated
    /// as absent rather than taking the server down.
    pub fn load_workspace(&self) -> Option<WorkspaceSnapshot> {
        let path = self.workspace_path();
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(snapshot) => Some(snapshot),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "ignoring corrupt workspace.json; seeding a fresh workspace"
                );
                None
            }
        }
    }

    /// Writes the workspace out. Failures are logged, not fatal: a persistence
    /// hiccup must not break the live API.
    pub fn save_workspace(&self, snapshot: &WorkspaceSnapshot) {
        if let Err(e) = write_atomic(&self.workspace_path(), snapshot) {
            tracing::warn!(error = %e, "failed to persist the workspace");
        }
    }

    /// Loads one group's saved log; empty when the group has none yet or its
    /// file is unreadable/corrupt.
    pub fn load_messages(&self, group_id: &str) -> Vec<Message> {
        let path = self.message_path(group_id);
        let Ok(bytes) = std::fs::read(&path) else {
            return Vec::new();
        };
        match serde_json::from_slice(&bytes) {
            Ok(messages) => messages,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "ignoring a corrupt message log; starting the group empty"
                );
                Vec::new()
            }
        }
    }

    /// Writes one group's log out. Called after every message mutation; failures
    /// are logged, not fatal.
    pub fn save_messages(&self, group_id: &str, messages: &[Message]) {
        if let Err(e) = write_atomic(&self.message_path(group_id), &messages) {
            tracing::warn!(group = %group_id, error = %e, "failed to persist a message log");
        }
    }

    /// Loads one group's compressed history, or `None` when it has never been
    /// compressed (or the file is unreadable/corrupt) — in which case the group
    /// simply starts from its full transcript.
    pub fn load_summary(&self, group_id: &str) -> Option<GroupSummary> {
        let path = self.summary_path(group_id);
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(summary) => Some(summary),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "ignoring a corrupt context summary; starting the group from its full transcript"
                );
                None
            }
        }
    }

    /// Writes one group's compressed history out. Called after each compression
    /// pass; failures are logged, not fatal.
    pub fn save_summary(&self, group_id: &str, summary: &GroupSummary) {
        if let Err(e) = write_atomic(&self.summary_path(group_id), summary) {
            tracing::warn!(group = %group_id, error = %e, "failed to persist a context summary");
        }
    }
}

/// Serializes `value` as pretty JSON and writes it atomically: a sibling temp
/// file is written and flushed, then renamed over the target, so a reader never
/// sees a partial file. Creates the parent directory on first use.
fn write_atomic<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Keeps a group id usable as a filename. Ids are uuids or short slugs, but a
/// hand-supplied id could contain a path separator; mapping anything outside
/// `[A-Za-z0-9_-]` to `_` keeps writes inside the messages directory.
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}
