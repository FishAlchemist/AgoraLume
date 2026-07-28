//! In-memory server state.
//!
//! Everything lives in process memory: the workspace (the single source of
//! truth for personas/groups/etc.), per-group message logs, and a broadcast
//! channel per group that fans live events out to every open SSE stream. The
//! in-memory store is provisional — a database will replace it without changing
//! the API — just as the simulated turn will give way to a real LLM.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::agent::event::Event;
use crate::agent::turn::{AgentRuntime, coordinator_loop, current_time_of_day, generate_suggestions};
use crate::config::Pricing;
use crate::models::{AgentTrace, GroupSuggestions, Message, ReadReceipt};
use crate::persist::Persistence;
use crate::workspace::Workspace;

/// How many pending commands a group's coordinator buffers before senders back
/// off. Turns are infrequent, so this is generous.
const COMMAND_CAPACITY: usize = 64;

/// How many live events a group's channel buffers for slow subscribers before
/// they start lagging. Generous — turns are tiny and infrequent.
const CHANNEL_CAPACITY: usize = 256;

/// How many recent agent traces to keep per group for the debug panel. Old
/// traces fall off the front; the running usage totals are unaffected.
const DEBUG_TRACE_CAP: usize = 50;

/// Minimum gap between suggestion generations for one group. A GET that finds the
/// suggestions stale, or an explicit regenerate, is dropped inside this window —
/// the server-side rate limit that stops a client hammering the (LLM-costing)
/// generator. Paired with a single-flight guard so overlapping requests never run
/// two generations at once.
const SUGGEST_COOLDOWN_MS: i64 = 6_000;

/// A live event pushed to a group's SSE subscribers.
#[derive(Clone, Debug)]
pub enum StreamEvent {
    /// A new message (AI reply or mood). The default SSE `message` event.
    Message(Message),
    /// A read receipt. Delivered as a named `read` SSE event.
    Read(ReadReceipt),
    /// The group's coordinator started (`true`) or finished (`false`) a turn.
    /// Delivered as a named `activity` SSE event; drives the composer lock so a
    /// user message can never interleave with an in-flight turn.
    Activity(bool),
    /// A debug trace of one agent inference (the prompt it saw + its decision +
    /// token usage). Delivered as a named `debug` SSE event; drives the debug
    /// panel. Never affects the chat itself.
    Debug(AgentTrace),
    /// Freshly generated conversation suggestions for the group. Delivered as a
    /// named `suggestions` SSE event so an open composer updates its starter chips
    /// the moment a background generation finishes.
    Suggestions(GroupSuggestions),
}

/// Per-group generation gate for suggestions: enforces both the cooldown (via
/// `last_started_ms`) and single-flight (via `in_flight`), so a burst of GETs or
/// regenerate calls can't launch overlapping or too-frequent generations.
#[derive(Default)]
struct SuggestGate {
    in_flight: bool,
    last_started_ms: i64,
}

/// Cumulative LLM usage since startup, plus recent per-group traces — the data
/// behind the debug/usage panel.
#[derive(Default)]
struct DebugState {
    requests: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    cached_prompt_tokens: u64,
    /// Recent traces per group, oldest first, capped at [`DEBUG_TRACE_CAP`].
    traces: HashMap<String, VecDeque<AgentTrace>>,
}

/// A group's compressed older history. `text` is the running summary the
/// orchestrator prepends to the (recent, verbatim) transcript; `through_id` is
/// the id of the last conversation message already folded into it, so the loop
/// knows which lines are still live and must be sent verbatim. An empty summary
/// with no `through_id` (the default) means nothing has been compressed yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GroupSummary {
    pub text: String,
    pub through_id: Option<String>,
}

/// A snapshot of the cumulative usage counters, for building the `/debug/usage`
/// response outside the lock.
#[derive(Clone, Copy, Default)]
pub struct DebugTotals {
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_prompt_tokens: u64,
}

pub struct AppState {
    workspace: Mutex<Workspace>,
    messages: Mutex<HashMap<String, Vec<Message>>>,
    /// Per-group compressed older history (the running summary + how far it
    /// reaches). Loaded from disk alongside a group's messages on first touch.
    summaries: Mutex<HashMap<String, GroupSummary>>,
    /// Per-group cached conversation suggestions, generated server-side and
    /// persisted so they survive a restart. Loaded alongside a group's messages
    /// on first touch; regenerated only when stale (see [`AppState::request_suggestions`]).
    suggestions: Mutex<HashMap<String, GroupSuggestions>>,
    /// Per-group suggestion-generation gate (cooldown + single-flight). Purely
    /// in-memory: a restart starts with a clean gate, which at worst allows one
    /// early regeneration.
    suggest_gates: Mutex<HashMap<String, SuggestGate>>,
    channels: Mutex<HashMap<String, broadcast::Sender<StreamEvent>>>,
    /// The swappable agent runtime (brain + memory + loop config).
    pub runtime: AgentRuntime,
    /// One command channel per group, feeding its coordinator task. Created
    /// lazily on first dispatch so idle groups run nothing.
    coordinators: Mutex<HashMap<String, mpsc::Sender<Event>>>,
    /// The on-disk store when persistence is enabled, else `None` (pure
    /// in-memory). Workspace mutations and message mutations write through it.
    persistence: Option<Persistence>,
    /// Group ids whose message log has been loaded from disk. Persisted logs
    /// load lazily on first touch; this marks the ones already pulled in so a
    /// second access doesn't re-read (and clobber in-memory) the file.
    loaded_groups: Mutex<HashSet<String>>,
    /// Group ids whose coordinator is currently running a turn. A stream reads
    /// this on (re)connect to seed the composer lock, so a reconnect that missed
    /// the broadcast `activity` frames still recovers the right state.
    active_groups: Mutex<HashSet<String>>,
    /// LLM usage counters and recent traces for the debug panel.
    debug: Mutex<DebugState>,
    /// Token pricing for the estimated-cost readout, or `None` to show tokens
    /// only. Set once at startup from config.
    pricing: Option<Pricing>,
}

impl AppState {
    /// Builds the app state with a specific runtime and no persistence — a pure
    /// in-memory run. Tests use this to inject a scripted brain or deterministic
    /// loop config.
    pub fn with_runtime(runtime: AgentRuntime) -> Self {
        Self::build(runtime, None)
    }

    /// Builds the app state backed by on-disk persistence: the workspace is
    /// loaded from `workspace.json` (or seeded on first run) and message logs
    /// load lazily per group. `main` uses this when persistence is enabled.
    pub fn with_persistence(runtime: AgentRuntime, persistence: Persistence) -> Self {
        Self::build(runtime, Some(persistence))
    }

    fn build(runtime: AgentRuntime, persistence: Option<Persistence>) -> Self {
        // A persisted run starts from disk (or a fresh seed the first time);
        // an in-memory run seeds every time.
        let workspace = persistence
            .as_ref()
            .and_then(Persistence::load_workspace)
            .map_or_else(Workspace::seeded, Workspace::from_snapshot);
        // The demo history only makes sense for a throwaway in-memory run; a
        // persisted server starts each group empty and fills it from disk on
        // first access, so nothing is seeded over the saved logs.
        let messages = if persistence.is_some() { HashMap::new() } else { seed_messages() };
        Self {
            workspace: Mutex::new(workspace),
            messages: Mutex::new(messages),
            summaries: Mutex::new(HashMap::new()),
            suggestions: Mutex::new(HashMap::new()),
            suggest_gates: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
            runtime,
            coordinators: Mutex::new(HashMap::new()),
            persistence,
            loaded_groups: Mutex::new(HashSet::new()),
            active_groups: Mutex::new(HashSet::new()),
            debug: Mutex::new(DebugState::default()),
            pricing: None,
        }
    }

    /// Sets the token pricing used for the estimated-cost readout. Called once
    /// at startup, before the state is shared, so it takes `&mut self`.
    pub fn set_pricing(&mut self, pricing: Option<Pricing>) {
        self.pricing = pricing;
    }

    /// The configured token pricing, if any.
    pub fn pricing(&self) -> Option<&Pricing> {
        self.pricing.as_ref()
    }

    /// Whether state is persisted to disk (survives a restart). Surfaced through
    /// `/meta`.
    pub fn persistent(&self) -> bool {
        self.persistence.is_some()
    }

    /// Records a debug trace of one agent inference: accumulate its usage into
    /// the running totals, keep it in the group's recent-trace ring, and stream
    /// it to any open debug panels. Every inference is one request.
    pub fn record_trace(&self, group_id: &str, trace: AgentTrace) {
        {
            let mut debug = self.debug.lock().unwrap();
            debug.requests += 1;
            if let Some(usage) = &trace.usage {
                debug.prompt_tokens += usage.prompt_tokens;
                debug.completion_tokens += usage.completion_tokens;
                debug.total_tokens += usage.total_tokens;
                debug.cached_prompt_tokens += usage.cached_prompt_tokens;
            }
            let ring = debug.traces.entry(group_id.to_string()).or_default();
            ring.push_back(trace.clone());
            while ring.len() > DEBUG_TRACE_CAP {
                ring.pop_front();
            }
        }
        let _ = self.channel(group_id).send(StreamEvent::Debug(trace));
    }

    /// A snapshot of the cumulative usage counters.
    pub fn debug_totals(&self) -> DebugTotals {
        let debug = self.debug.lock().unwrap();
        DebugTotals {
            requests: debug.requests,
            prompt_tokens: debug.prompt_tokens,
            completion_tokens: debug.completion_tokens,
            total_tokens: debug.total_tokens,
            cached_prompt_tokens: debug.cached_prompt_tokens,
        }
    }

    /// The recent traces for a group, oldest first.
    pub fn debug_traces(&self, group_id: &str) -> Vec<AgentTrace> {
        self.debug
            .lock()
            .unwrap()
            .traces
            .get(group_id)
            .map(|ring| ring.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Loads a group's message log from disk the first time it is touched, so
    /// persisted history is read on demand rather than all at once. A no-op when
    /// persistence is off or the group is already loaded.
    fn ensure_loaded(&self, group_id: &str) {
        let Some(persistence) = &self.persistence else {
            return;
        };
        // Lock order: `loaded_groups` before `messages`. This is the only place
        // that holds both, so no other path can deadlock against it.
        let mut loaded = self.loaded_groups.lock().unwrap();
        if loaded.contains(group_id) {
            return;
        }
        let disk = persistence.load_messages(group_id);
        self.messages.lock().unwrap().entry(group_id.to_string()).or_insert(disk);
        // The group's compressed history rides alongside its log — pulled in the
        // same first touch so a restart resumes from the saved summary rather than
        // re-summarizing (or worse, re-sending) the whole history.
        if let Some(summary) = persistence.load_summary(group_id) {
            self.summaries.lock().unwrap().entry(group_id.to_string()).or_insert(summary);
        }
        // Cached suggestions likewise — so a restart shows the stored openers
        // immediately instead of regenerating (and spending the LLM) on first open.
        if let Some(suggestions) = persistence.load_suggestions(group_id) {
            self.suggestions.lock().unwrap().entry(group_id.to_string()).or_insert(suggestions);
        }
        loaded.insert(group_id.to_string());
    }

    /// Persists the whole workspace after a mutation. No-op without persistence.
    /// Called by the workspace CRUD handlers once their change is applied.
    pub fn persist_workspace(&self) {
        let Some(persistence) = &self.persistence else {
            return;
        };
        let snapshot = self.workspace().to_snapshot();
        persistence.save_workspace(&snapshot);
    }

    /// Persists a single group's message log after a mutation. No-op without
    /// persistence.
    fn persist_messages(&self, group_id: &str) {
        let Some(persistence) = &self.persistence else {
            return;
        };
        let snapshot = self.messages.lock().unwrap().get(group_id).cloned().unwrap_or_default();
        persistence.save_messages(group_id, &snapshot);
    }

    /// A group's compressed older history (empty when nothing has been compressed
    /// yet). The orchestrator prepends `text` to the recent transcript and drops
    /// the lines up to and including `through_id`.
    pub fn summary(&self, group_id: &str) -> GroupSummary {
        self.ensure_loaded(group_id);
        self.summaries.lock().unwrap().get(group_id).cloned().unwrap_or_default()
    }

    /// Replaces a group's compressed history after a compression pass, persisting
    /// it so the summary survives a restart.
    pub fn set_summary(&self, group_id: &str, summary: GroupSummary) {
        self.ensure_loaded(group_id);
        self.summaries.lock().unwrap().insert(group_id.to_string(), summary.clone());
        if let Some(persistence) = &self.persistence {
            persistence.save_summary(group_id, &summary);
        }
    }

    /// A group's cached conversation suggestions (empty — `generatedAt == 0` —
    /// before the first generation). The GET handler returns this immediately;
    /// generation, when needed, happens in the background via
    /// [`Self::request_suggestions`].
    pub fn suggestions(&self, group_id: &str) -> GroupSuggestions {
        self.ensure_loaded(group_id);
        self.suggestions.lock().unwrap().get(group_id).cloned().unwrap_or_default()
    }

    /// Kicks a background suggestion generation for a group when warranted, and
    /// returns at once — the caller reads the current cache with [`Self::suggestions`].
    ///
    /// Generation is gated three ways: it never overlaps a run already in flight;
    /// it is rate-limited to one start per [`SUGGEST_COOLDOWN_MS`] (so a client
    /// can't hammer the LLM by spamming); and, unless `force` is set, it only runs
    /// when the cache is *stale* — no suggestions yet, the conversation has moved
    /// on since they were made, or the part of day has changed (so morning openers
    /// don't linger into the evening). `force` (the explicit regenerate) still
    /// respects the cooldown and single-flight guard.
    pub fn request_suggestions(self: &Arc<Self>, group_id: &str, force: bool) {
        // Read the inputs to the staleness check outside the gate lock (each takes
        // its own mutex); the gate then guards the actual decision to launch.
        let last_id = self.last_conversation_id(group_id);
        let bucket = current_time_of_day();
        let cached = self.suggestions(group_id);
        let now = crate::models::now_ms();

        let mut gates = self.suggest_gates.lock().unwrap();
        let gate = gates.entry(group_id.to_string()).or_default();
        if gate.in_flight || now - gate.last_started_ms < SUGGEST_COOLDOWN_MS {
            return;
        }
        let stale = cached.generated_at == 0
            || cached.through_id.as_deref() != last_id.as_deref()
            || cached.time_of_day != bucket;
        if !force && !stale {
            return;
        }
        gate.in_flight = true;
        gate.last_started_ms = now;
        drop(gates);

        tokio::spawn(generate_suggestions(self.clone(), group_id.to_string()));
    }

    /// Releases the generation gate for a group and, when `result` is `Some`,
    /// stores the fresh suggestions, persists them, and pushes them on the group's
    /// `suggestions` SSE frame. `None` (a failed or empty pass) keeps the previous
    /// cache and only clears the in-flight flag. Called by [`generate_suggestions`].
    pub fn finish_suggestions(&self, group_id: &str, result: Option<GroupSuggestions>) {
        if let Some(gate) = self.suggest_gates.lock().unwrap().get_mut(group_id) {
            gate.in_flight = false;
        }
        let Some(suggestions) = result else {
            return;
        };
        self.ensure_loaded(group_id);
        self.suggestions.lock().unwrap().insert(group_id.to_string(), suggestions.clone());
        if let Some(persistence) = &self.persistence {
            persistence.save_suggestions(group_id, &suggestions);
        }
        let _ = self.channel(group_id).send(StreamEvent::Suggestions(suggestions));
    }

    /// The id of the most recent conversation line in a group's log (skipping
    /// moods and system notices), or `None` when the group has no chat yet. The
    /// suggestion staleness key: when it changes, cached openers no longer follow
    /// the latest message.
    fn last_conversation_id(&self, group_id: &str) -> Option<String> {
        self.ensure_loaded(group_id);
        self.messages.lock().unwrap().get(group_id).and_then(|list| {
            list.iter().rev().find_map(|m| match m {
                Message::Conversation { id, .. } => Some(id.clone()),
                _ => None,
            })
        })
    }

    /// Hands an event to a group's coordinator, spawning the coordinator task on
    /// first use. Returns immediately; the turn runs in the background and its
    /// replies, moods, and read receipts arrive on the group's stream.
    pub fn dispatch(self: &Arc<Self>, group_id: &str, event: Event) {
        let sender = self.coordinator(group_id);
        // A full buffer means a burst of unserviced commands; dropping the
        // newest is acceptable back-pressure for a chat turn.
        let _ = sender.try_send(event);
    }

    /// The command sender for a group's coordinator, creating (and spawning) it
    /// on first use.
    fn coordinator(self: &Arc<Self>, group_id: &str) -> mpsc::Sender<Event> {
        self.coordinators
            .lock()
            .unwrap()
            .entry(group_id.to_string())
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
                tokio::spawn(coordinator_loop(self.clone(), group_id.to_string(), receiver));
                sender
            })
            .clone()
    }

    /// Locks the workspace for reading or mutation. All CRUD invariants live on
    /// `Workspace`, so callers just take the guard and call its methods.
    pub fn workspace(&self) -> MutexGuard<'_, Workspace> {
        self.workspace.lock().unwrap()
    }

    /// A snapshot copy of a group's message log (empty if never used).
    pub fn list(&self, group_id: &str) -> Vec<Message> {
        self.ensure_loaded(group_id);
        self.messages
            .lock()
            .unwrap()
            .get(group_id)
            .cloned()
            .unwrap_or_default()
    }

    /// The broadcast sender for a group, creating it on first use so late
    /// subscribers and the first emit share the same channel.
    pub fn channel(&self, group_id: &str) -> broadcast::Sender<StreamEvent> {
        self.channels
            .lock()
            .unwrap()
            .entry(group_id.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }

    /// Appends a message to the log without broadcasting it. Used for the user's
    /// own message: the client already shows it from the POST response, so
    /// re-broadcasting would duplicate it.
    pub fn store(&self, group_id: &str, message: Message) {
        self.ensure_loaded(group_id);
        self.messages
            .lock()
            .unwrap()
            .entry(group_id.to_string())
            .or_default()
            .push(message);
        self.persist_messages(group_id);
    }

    /// Records and broadcasts whether the group's coordinator is actively running
    /// a turn. The frontend keeps the composer locked while active so users send
    /// only when the loop is idle. The recorded flag lets a stream that connects
    /// (or reconnects) mid-turn seed its lock from [`is_active`].
    pub fn set_active(&self, group_id: &str, active: bool) {
        {
            let mut groups = self.active_groups.lock().unwrap();
            if active {
                groups.insert(group_id.to_string());
            } else {
                groups.remove(group_id);
            }
        }
        let _ = self.channel(group_id).send(StreamEvent::Activity(active));
    }

    /// Whether the group's coordinator is currently running a turn. A stream
    /// seeds the composer lock with this on connect, so a reconnect (common
    /// through a tunnel) that missed the broadcast `activity` frames recovers the
    /// correct lock state without a page refresh.
    pub fn is_active(&self, group_id: &str) -> bool {
        self.active_groups.lock().unwrap().contains(group_id)
    }

    /// Appends a message and broadcasts it to live subscribers.
    pub fn emit(&self, group_id: &str, message: Message) {
        self.store(group_id, message.clone());
        // A send with zero receivers is fine — the log still has the message,
        // and any fresh subscriber picks it up via the initial list fetch.
        let _ = self.channel(group_id).send(StreamEvent::Message(message));
    }

    /// Records that one AI persona processed a message and notifies subscribers.
    /// De-duplicated: a repeated read for the same persona is a no-op.
    pub fn mark_read(&self, group_id: &str, message_id: &str, persona_id: &str) {
        self.ensure_loaded(group_id);
        {
            let mut store = self.messages.lock().unwrap();
            let Some(list) = store.get_mut(group_id) else {
                return;
            };
            let Some(Message::Conversation { read_by, .. }) =
                list.iter_mut().find(|m| m.id() == message_id)
            else {
                return;
            };
            let readers = read_by.get_or_insert_with(Vec::new);
            if readers.iter().any(|id| id == persona_id) {
                return;
            }
            readers.push(persona_id.to_string());
        }
        self.persist_messages(group_id);
        let _ = self.channel(group_id).send(StreamEvent::Read(ReadReceipt {
            group_id: group_id.to_string(),
            message_id: message_id.to_string(),
            persona_id: persona_id.to_string(),
        }));
    }

    /// Whether an AI persona has already read a given message. Drives the resume
    /// sweep: an agent whose inference failed never marked the trigger read, so
    /// "not read" is exactly the set of agents a retry re-runs.
    pub fn has_read(&self, group_id: &str, message_id: &str, persona_id: &str) -> bool {
        self.ensure_loaded(group_id);
        let store = self.messages.lock().unwrap();
        let Some(list) = store.get(group_id) else {
            return false;
        };
        matches!(
            list.iter().find(|m| m.id() == message_id),
            Some(Message::Conversation { read_by: Some(readers), .. })
                if readers.iter().any(|id| id == persona_id)
        )
    }
}

/// Opening history so a freshly pointed-at backend shows content immediately.
fn seed_messages() -> HashMap<String, Vec<Message>> {
    HashMap::from([
        (
            "lounge".to_string(),
            vec![
                Message::mood(
                    "lounge",
                    "aria",
                    "😌 relaxed",
                    Some("settling into the lounge".into()),
                ),
                Message::conversation(
                    "lounge",
                    "aria",
                    "Welcome to AgoraLume! Ask us anything — Nox and Sol are here too.",
                    None,
                ),
                Message::conversation(
                    "lounge",
                    "nox",
                    "A multi-persona group chat. Efficient. I approve.",
                    None,
                ),
            ],
        ),
        (
            "lab".to_string(),
            vec![Message::mood("lab", "nox", "🤔 focused", None)],
        ),
    ])
}
