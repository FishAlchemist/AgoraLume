//! Optional on-disk persistence for the workspace and per-group chat logs.
//!
//! Layout under the data directory (`AGORALUME_DATA_DIR`):
//!
//! ```text
//! workspace.json               the whole editable workspace (small; loaded at startup)
//! messages/<group_id>.json     one group's chat log (loaded lazily, saved on change)
//! summaries/<group_id>.json    one group's compressed older history (loaded with its log)
//! suggestions/<group_id>.json  one group's cached conversation starters (loaded with its log)
//! usage.json                   cumulative LLM usage counters + accrued cost, across every group (loaded at startup)
//! usage/<group_id>.json        one group's own usage counters + accrued cost (loaded with its log)
//! ```
//!
//! Splitting the message logs off the workspace means a running server only
//! reads a group's history the first time that group is touched — the "read on
//! demand" the design calls for — instead of loading everything up front.
//!
//! Everything degrades safely: a missing or corrupt file falls back to a seed
//! (or an empty log) rather than crashing, and writes are atomic (temp file +
//! rename) so a crash mid-write can never leave a half-written file behind.
//!
//! `workspace.json` — the one irreplaceable file — is additionally wrapped in a
//! [`Versioned`] envelope (`{ "version": N, "data": … }`). Additive fields still
//! evolve freely via `#[serde(default)]`; the version guards *breaking* shape
//! changes. On a version the running build doesn't recognise, or an unreadable
//! file, the workspace is moved aside (`.corrupt-<epoch>`) rather than silently
//! overwritten by the next save, so nothing is ever lost without a trace. The
//! cheap-to-regenerate files (messages, summaries, suggestions) stay bare.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::models::{GroupSuggestions, Message};
use crate::state::{DebugTotals, GroupSummary};
use crate::workspace::WorkspaceSnapshot;

/// On-disk format version for `workspace.json`. Bump this **only** on a breaking
/// change to [`WorkspaceSnapshot`]'s shape (a renamed or retyped field, a
/// removed variant); additive fields ride `#[serde(default)]` and need no bump.
/// A file whose version this build doesn't recognise is preserved, never
/// overwritten — see [`Persistence::load_workspace`].
const WORKSPACE_FORMAT_VERSION: u32 = 1;

/// A versioned envelope around persisted data: `{ "version": N, "data": … }`.
/// Only `workspace.json` — the one irreplaceable file — is written this way;
/// message logs, summaries, and suggestion caches stay bare, being cheap to lose
/// and regenerate. Generic over `T` so a write can borrow
/// (`Versioned<&WorkspaceSnapshot>`) while a read owns
/// (`Versioned<WorkspaceSnapshot>`), with no clone of the whole workspace.
#[derive(Serialize, Deserialize)]
struct Versioned<T> {
    version: u32,
    data: T,
}

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

    fn suggestions_path(&self, group_id: &str) -> PathBuf {
        self.dir.join("suggestions").join(format!("{}.json", sanitize(group_id)))
    }

    fn usage_path(&self) -> PathBuf {
        self.dir.join("usage.json")
    }

    fn group_usage_path(&self, group_id: &str) -> PathBuf {
        self.dir.join("usage").join(format!("{}.json", sanitize(group_id)))
    }

    /// Loads the persisted workspace, or `None` when there is no saved file yet
    /// (so the caller seeds a fresh one).
    ///
    /// The file is a [`Versioned`] envelope, and three cases are handled
    /// explicitly — **none of which ever overwrites data**:
    /// - **current version** — returned as-is.
    /// - **legacy, unversioned** (written before the envelope existed) — a bare
    ///   [`WorkspaceSnapshot`] is accepted and re-wrapped on the next save.
    /// - **unknown version, or corrupt** — the file is [`quarantine`]d (moved
    ///   aside, never discarded) and a fresh workspace is seeded, so an
    ///   unreadable or newer-format file survives for manual recovery instead of
    ///   being silently replaced.
    pub fn load_workspace(&self) -> Option<WorkspaceSnapshot> {
        let path = self.workspace_path();
        let bytes = std::fs::read(&path).ok()?;

        // Current format: a versioned envelope.
        match serde_json::from_slice::<Versioned<WorkspaceSnapshot>>(&bytes) {
            Ok(file) if file.version == WORKSPACE_FORMAT_VERSION => return Some(file.data),
            Ok(file) => {
                // A version this build doesn't understand (e.g. one written by a
                // newer release). Preserve it rather than overwrite it.
                tracing::warn!(
                    path = %path.display(),
                    found = file.version,
                    expected = WORKSPACE_FORMAT_VERSION,
                    "workspace.json has an unsupported format version; moving it aside and seeding a fresh workspace"
                );
                quarantine(&path);
                return None;
            }
            Err(_) => {}
        }

        // Legacy: a bare, unversioned snapshot from before the envelope existed.
        // Accept it; the next save re-writes it inside the current envelope.
        match serde_json::from_slice::<WorkspaceSnapshot>(&bytes) {
            Ok(snapshot) => {
                tracing::info!(
                    path = %path.display(),
                    "migrating an unversioned workspace.json to the versioned format on the next save"
                );
                Some(snapshot)
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "workspace.json is unreadable; moving it aside and seeding a fresh workspace"
                );
                quarantine(&path);
                None
            }
        }
    }

    /// Writes the workspace out inside the current [`Versioned`] envelope.
    /// Failures are logged, not fatal: a persistence hiccup must not break the
    /// live API.
    pub fn save_workspace(&self, snapshot: &WorkspaceSnapshot) {
        let file = Versioned { version: WORKSPACE_FORMAT_VERSION, data: snapshot };
        if let Err(e) = write_atomic(&self.workspace_path(), &file) {
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

    /// Loads one group's cached suggestions, or `None` when none have been
    /// generated yet (or the file is unreadable/corrupt) — in which case the group
    /// starts with no suggestions and regenerates on first request.
    pub fn load_suggestions(&self, group_id: &str) -> Option<GroupSuggestions> {
        let path = self.suggestions_path(group_id);
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(suggestions) => Some(suggestions),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "ignoring corrupt cached suggestions; regenerating on next request"
                );
                None
            }
        }
    }

    /// Writes one group's cached suggestions out. Called after each successful
    /// generation; failures are logged, not fatal.
    pub fn save_suggestions(&self, group_id: &str, suggestions: &GroupSuggestions) {
        if let Err(e) = write_atomic(&self.suggestions_path(group_id), suggestions) {
            tracing::warn!(group = %group_id, error = %e, "failed to persist cached suggestions");
        }
    }

    /// Loads the cumulative LLM usage counters and accrued cost, or `None` when
    /// there is no saved file yet (a fresh install) or it is unreadable/corrupt —
    /// in which case the caller starts the counters at zero. Unlike
    /// `workspace.json`, a lost or corrupt file is not quarantined: it just
    /// resets a readout, not user-authored data.
    pub fn load_usage(&self) -> Option<DebugTotals> {
        let path = self.usage_path();
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(totals) => Some(totals),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "ignoring corrupt usage totals; starting the counters at zero"
                );
                None
            }
        }
    }

    /// Writes the cumulative usage counters and accrued cost out. Called after
    /// every recorded trace; failures are logged, not fatal.
    pub fn save_usage(&self, totals: &DebugTotals) {
        if let Err(e) = write_atomic(&self.usage_path(), totals) {
            tracing::warn!(error = %e, "failed to persist usage totals");
        }
    }

    /// Loads one group's own cumulative usage and accrued cost — independent of
    /// every other group's, unlike [`Self::load_usage`]. `None` when the group
    /// has recorded no traces yet, or its file is unreadable/corrupt, in which
    /// case the caller starts that group's counters at zero.
    pub fn load_group_usage(&self, group_id: &str) -> Option<DebugTotals> {
        let path = self.group_usage_path(group_id);
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(totals) => Some(totals),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "ignoring corrupt per-group usage totals; starting the group's counters at zero"
                );
                None
            }
        }
    }

    /// Writes one group's own usage counters and accrued cost out. Called after
    /// every recorded trace; failures are logged, not fatal.
    pub fn save_group_usage(&self, group_id: &str, totals: &DebugTotals) {
        if let Err(e) = write_atomic(&self.group_usage_path(group_id), totals) {
            tracing::warn!(group = %group_id, error = %e, "failed to persist per-group usage totals");
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

/// Moves an unreadable or unsupported file aside — appending `.corrupt-<epoch>`
/// — instead of letting the next save overwrite it, so its data is preserved
/// for manual recovery. Best-effort: a rename failure is logged and swallowed
/// (the caller seeds fresh regardless).
fn quarantine(path: &Path) {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut dest = path.as_os_str().to_owned();
    dest.push(format!(".corrupt-{stamp}"));
    let dest = PathBuf::from(dest);
    match std::fs::rename(path, &dest) {
        Ok(()) => {
            tracing::warn!(from = %path.display(), to = %dest.display(), "preserved an unreadable file");
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not preserve an unreadable file");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Cost;
    use crate::state::ModelTotals;
    use crate::workspace::Workspace;

    /// A fresh, unique scratch directory under the system temp dir.
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agoralume-persist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn usage_round_trips_and_survives_a_restart() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);

        assert!(store.load_usage().is_none(), "nothing saved yet");

        let mut totals = DebugTotals::default();
        totals.models.insert(
            "gpt-4o-mini".to_string(),
            ModelTotals {
                requests: 3,
                prompt_tokens: 100,
                completion_tokens: 40,
                total_tokens: 140,
                cached_prompt_tokens: 10,
                cost: Some(Cost {
                    currency: "USD".to_string(),
                    input: 0.001,
                    cached_input: 0.0001,
                    output: 0.002,
                    total: 0.0031,
                }),
            },
        );
        store.save_usage(&totals);

        let loaded = store.load_usage().expect("load");
        let model = loaded.models.get("gpt-4o-mini").expect("the model entry round-trips");
        assert_eq!(model.requests, 3);
        assert_eq!(model.total_tokens, 140);
        assert_eq!(model.cost.as_ref().unwrap().total, 0.0031);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_usage_file_is_ignored_not_fatal() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);
        std::fs::write(store.usage_path(), b"not json").unwrap();

        assert!(store.load_usage().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn group_usage_round_trips_independently_per_group() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);

        assert!(store.load_group_usage("g1").is_none(), "nothing saved yet");

        let mut g1 = DebugTotals::default();
        g1.models.insert("gpt-4o-mini".to_string(), ModelTotals { requests: 2, ..Default::default() });
        let mut g2 = DebugTotals::default();
        g2.models.insert("gpt-4o-mini".to_string(), ModelTotals { requests: 9, ..Default::default() });

        store.save_group_usage("g1", &g1);
        store.save_group_usage("g2", &g2);

        assert_eq!(store.load_group_usage("g1").unwrap().models["gpt-4o-mini"].requests, 2);
        assert_eq!(store.load_group_usage("g2").unwrap().models["gpt-4o-mini"].requests, 9);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_group_usage_file_is_ignored_not_fatal() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);
        let path = store.group_usage_path("g1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"not json").unwrap();

        assert!(store.load_group_usage("g1").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workspace_round_trips_through_the_versioned_envelope() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);
        let snapshot = Workspace::seeded().to_snapshot();

        store.save_workspace(&snapshot);

        // The file on disk carries the version tag, not a bare snapshot.
        let raw = std::fs::read_to_string(store.workspace_path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["version"], WORKSPACE_FORMAT_VERSION);
        assert!(value["data"]["personas"].is_array());

        let loaded = store.load_workspace().expect("load");
        assert_eq!(loaded.personas.len(), snapshot.personas.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_unversioned_workspace_is_accepted() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);
        let snapshot = Workspace::seeded().to_snapshot();

        // Write the pre-envelope format: a bare snapshot with no version tag.
        write_atomic(&store.workspace_path(), &snapshot).unwrap();

        let loaded = store.load_workspace().expect("accept legacy file");
        assert_eq!(loaded.personas.len(), snapshot.personas.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_version_is_preserved_not_overwritten() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);
        let snapshot = Workspace::seeded().to_snapshot();

        // A file from a hypothetical newer build.
        let future = Versioned { version: WORKSPACE_FORMAT_VERSION + 1, data: &snapshot };
        write_atomic(&store.workspace_path(), &future).unwrap();

        assert!(store.load_workspace().is_none(), "unknown version seeds fresh");
        // The original file is gone from its path but preserved beside it, so a
        // subsequent save can't clobber it.
        assert!(!store.workspace_path().exists());
        let preserved: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
            .collect();
        assert_eq!(preserved.len(), 1, "the newer-format file was quarantined");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_workspace_is_quarantined() {
        let dir = temp_dir();
        let store = Persistence::new(&dir);
        std::fs::write(store.workspace_path(), b"{ this is not json").unwrap();

        assert!(store.load_workspace().is_none());
        assert!(!store.workspace_path().exists(), "the corrupt file was moved aside");

        std::fs::remove_dir_all(&dir).ok();
    }
}
