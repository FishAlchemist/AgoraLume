//! Per-agent private memory: retrieval, threshold writes, and consolidation.
//!
//! Each agent has its own store of notes it chose to keep (facts, motives,
//! impressions, relationships). The [`MemoryStore`] trait is the seam a
//! vector- or DB-backed store slots behind later; [`InMemoryStore`] is the
//! provisional in-process implementation, matching the rest of this build.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::models::now_ms;

/// One private memory entry an agent has recorded.
#[derive(Clone, Debug)]
pub struct MemoryEntry {
    pub note: String,
    /// Salience in `[0, 1]` — how much this mattered when written.
    pub weight: f32,
    /// When it was written (ms since the Unix epoch).
    pub ts: i64,
}

/// Relevance for retrieval and consolidation: salient and recent rank higher.
/// Weight is the dominant term; recency gently decays over hours so an old but
/// important memory still beats a fresh trivial one.
fn relevance(entry: &MemoryEntry, now: i64) -> f32 {
    let age_hours = ((now - entry.ts).max(0) as f32) / 3_600_000.0;
    entry.weight / (1.0 + age_hours)
}

/// A per-agent memory store. Retrieval feeds the agent's prompt; writes are
/// gated by the caller's salience threshold; consolidation keeps the store
/// bounded, approximating memory consolidation.
pub trait MemoryStore: Send + Sync {
    /// The `k` most relevant memories for a persona, most relevant first.
    fn retrieve(&self, persona_id: &str, k: usize) -> Vec<MemoryEntry>;
    /// Records a memory. Callers apply the salience threshold before calling.
    fn write(&self, persona_id: &str, entry: MemoryEntry);
    /// Compacts a persona's memories when they exceed `cap`, folding the least
    /// relevant overflow into a single consolidated note.
    fn consolidate(&self, persona_id: &str, cap: usize);
}

/// In-process memory: a map of persona id → its notes. Provisional, like the
/// rest of the store; the trait above lets persistence replace it untouched.
#[derive(Default)]
pub struct InMemoryStore {
    inner: Mutex<HashMap<String, Vec<MemoryEntry>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of stored entries for a persona — for tests and diagnostics.
    #[cfg(test)]
    pub fn len(&self, persona_id: &str) -> usize {
        self.inner.lock().unwrap().get(persona_id).map_or(0, Vec::len)
    }
}

impl MemoryStore for InMemoryStore {
    fn retrieve(&self, persona_id: &str, k: usize) -> Vec<MemoryEntry> {
        let now = now_ms();
        let guard = self.inner.lock().unwrap();
        let Some(entries) = guard.get(persona_id) else {
            return Vec::new();
        };
        let mut ranked = entries.clone();
        ranked.sort_by(|a, b| relevance(b, now).total_cmp(&relevance(a, now)));
        ranked.truncate(k);
        ranked
    }

    fn write(&self, persona_id: &str, entry: MemoryEntry) {
        self.inner.lock().unwrap().entry(persona_id.to_string()).or_default().push(entry);
    }

    fn consolidate(&self, persona_id: &str, cap: usize) {
        let now = now_ms();
        let mut guard = self.inner.lock().unwrap();
        let Some(entries) = guard.get_mut(persona_id) else {
            return;
        };
        if entries.len() <= cap {
            return;
        }
        // Keep the most relevant `cap - 1` entries verbatim and fold the rest
        // into one summary, so the store stays bounded while the gist of older,
        // lesser memories survives. A real summariser can replace this join.
        entries.sort_by(|a, b| relevance(b, now).total_cmp(&relevance(a, now)));
        let overflow = entries.split_off(cap.saturating_sub(1).max(1));
        if overflow.is_empty() {
            return;
        }
        let weight = overflow.iter().map(|e| e.weight).fold(0.0_f32, f32::max);
        let joined =
            overflow.iter().map(|e| e.note.as_str()).collect::<Vec<_>>().join("; ");
        entries.push(MemoryEntry { note: format!("(earlier) {joined}"), weight, ts: now });
    }
}
