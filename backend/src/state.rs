//! In-memory server state.
//!
//! Everything lives in process memory: the workspace (the single source of
//! truth for personas/groups/etc.), per-group message logs, and a broadcast
//! channel per group that fans live events out to every open SSE stream. The
//! in-memory store is provisional — a database will replace it without changing
//! the API — just as the simulated turn will give way to a real LLM.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::{broadcast, mpsc};

use crate::agent::event::Event;
use crate::agent::turn::{AgentRuntime, coordinator_loop};
use crate::models::{Message, ReadReceipt};
use crate::workspace::Workspace;

/// How many pending commands a group's coordinator buffers before senders back
/// off. Turns are infrequent, so this is generous.
const COMMAND_CAPACITY: usize = 64;

/// How many live events a group's channel buffers for slow subscribers before
/// they start lagging. Generous — turns are tiny and infrequent.
const CHANNEL_CAPACITY: usize = 256;

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
}

pub struct AppState {
    workspace: Mutex<Workspace>,
    messages: Mutex<HashMap<String, Vec<Message>>>,
    channels: Mutex<HashMap<String, broadcast::Sender<StreamEvent>>>,
    /// The swappable agent runtime (brain + memory + loop config).
    pub runtime: AgentRuntime,
    /// One command channel per group, feeding its coordinator task. Created
    /// lazily on first dispatch so idle groups run nothing.
    coordinators: Mutex<HashMap<String, mpsc::Sender<Event>>>,
}

impl AppState {
    /// Builds the seeded workspace with a specific runtime. `main` passes the
    /// runtime the config selected; tests inject a scripted brain, a shared
    /// memory store, or deterministic loop config.
    pub fn with_runtime(runtime: AgentRuntime) -> Self {
        Self {
            workspace: Mutex::new(Workspace::seeded()),
            messages: Mutex::new(seed_messages()),
            channels: Mutex::new(HashMap::new()),
            runtime,
            coordinators: Mutex::new(HashMap::new()),
        }
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
        self.messages
            .lock()
            .unwrap()
            .entry(group_id.to_string())
            .or_default()
            .push(message);
    }

    /// Broadcasts whether the group's coordinator is actively running a turn.
    /// The frontend keeps the composer locked while active so users send only
    /// when the loop is idle.
    pub fn set_active(&self, group_id: &str, active: bool) {
        let _ = self.channel(group_id).send(StreamEvent::Activity(active));
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
        let _ = self.channel(group_id).send(StreamEvent::Read(ReadReceipt {
            group_id: group_id.to_string(),
            message_id: message_id.to_string(),
            persona_id: persona_id.to_string(),
        }));
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
