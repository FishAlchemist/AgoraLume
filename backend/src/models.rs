//! Wire types shared with the frontend.
//!
//! These mirror `frontend/src/types.ts` exactly on the wire: the `Message`
//! discriminated union is internally tagged by `kind`, and every field is
//! camelCased so the JSON round-trips through the client without translation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

/// The two persona roles: a user's own identity vs. an AI agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum PersonaKind {
    User,
    Ai,
}

/// A bucket for classifying personas that also carries shared template
/// variables its members inherit.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blurb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<HashMap<String, String>>,
}

/// A sub-unit inside an organization; its variables sit between the org's and
/// the persona's in the inheritance chain.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Department {
    #[serde(default)]
    pub id: String,
    pub organization_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blurb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<HashMap<String, String>>,
}

/// A user identity or an AI agent. AI personas carry the system prompt and
/// variables the model needs; user personas are just a "you" to speak as.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub kind: PersonaKind,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blurb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub department_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<HashMap<String, String>>,
    /// Content hash of the raw `system_prompt` template — the persona's identity
    /// "version". Server-computed and effectively read-only on the wire: it is
    /// recomputed on every create/update (and on load) and any value a client
    /// sends is ignored. `None` when there is no prompt (user identities). Full
    /// lowercase-hex SHA-256; the UI shows a truncated prefix. See [`prompt_hash`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash: Option<String>,
}

impl Persona {
    /// Recomputes [`Persona::prompt_hash`] from the current `system_prompt`, so
    /// the stored hash is never trusted from client input or a stale snapshot.
    pub fn refresh_prompt_hash(&mut self) {
        self.prompt_hash = prompt_hash(self.system_prompt.as_deref());
    }
}

/// The identity hash of a raw system-prompt template: the unresolved text only —
/// not the resolved variables, not the assembled `<group_members>`/`<directory>`
/// roster — so it changes exactly when the author rewrites who the character is,
/// and not when unrelated context (group membership, inherited variables) shifts.
/// `None` for an empty or absent prompt. Content-addressed, so pasting earlier
/// exact text back resolves to the same hash a counter would treat as new.
pub fn prompt_hash(system_prompt: Option<&str>) -> Option<String> {
    let text = system_prompt?.trim();
    if text.is_empty() {
        return None;
    }
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Some(hex)
}

/// A user-assigned, git-tag-style name for a persona identity hash
/// ([`Persona::prompt_hash`]). Held in a side table so naming a version never
/// mutates the persona itself; content-addressing means reverting to earlier
/// exact prompt text resolves back to the same (possibly already-named) hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptLabel {
    pub hash: String,
    pub label: String,
}

/// Request body for naming a prompt identity hash. A blank label clears the name.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptLabelInput {
    pub label: String,
}

/// One persona-scoped memory: a fact a character chose to remember. Tagged with
/// the persona identity hash ([`Persona::prompt_hash`]) that was in force when it
/// was written, so a later rewrite of the persona never recalls a previous
/// version's memories out of character. Persisted in the workspace snapshot
/// alongside the personas the memories belong to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: String,
    pub persona_id: String,
    /// The persona identity hash in force when this memory was written — the
    /// scope key that keeps recall in-character. Always a real hash: a persona
    /// with no prompt (no hash) can hold no memories.
    pub prompt_hash: String,
    pub content: String,
    /// Milliseconds since the Unix epoch when the memory was written.
    pub created_at: i64,
}

/// Request body for writing a memory: just the text to remember.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInput {
    pub content: String,
}

/// A chat room: the AI personas that may speak, plus the user identity that
/// represents "you" here.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub persona_ids: Vec<String>,
    pub self_persona_id: String,
}

/// Conversation-starter suggestions for a group: a few short first-person
/// messages the user could send next when they aren't sure what to say. Generated
/// server-side and cached (and persisted) so they aren't recomputed on every
/// open — the frontend only fetches and displays them. Time-aware via
/// [`GroupSuggestions::time_of_day`] so an evening opener isn't offered in the
/// morning. `GET /groups/{id}/suggestions`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupSuggestions {
    /// The suggested opener lines, in the user's language. Empty until the first
    /// generation for this group completes.
    pub prompts: Vec<String>,
    /// When these were generated (epoch ms); `0` before the first generation.
    pub generated_at: i64,
    /// The coarse time-of-day bucket the suggestions were tuned for
    /// (`morning` / `afternoon` / `evening` / `night`) — lets the server tell when
    /// they've gone stale against the clock, and the UI hint the framing.
    #[serde(default)]
    pub time_of_day: String,
    /// Id of the last conversation message present when these were generated — the
    /// conversation-staleness key. Server bookkeeping the UI can ignore; absent
    /// when the group had no messages yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_id: Option<String>,
}

/// What the running server offers — lets the client tell a mock build (no LLM,
/// in-memory only) apart from a production one, independently of whether the
/// server is reachable at all.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerMeta {
    /// No LLM and no persistence — the "mock" mode. Equivalent to
    /// `!llm && !persistent`.
    pub mock: bool,
    /// An LLM is wired in to generate replies.
    pub llm: bool,
    /// State is persisted and survives a restart.
    pub persistent: bool,
    /// Server crate version.
    pub version: String,
}

/// User-level preferences.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub ui_language: String,
    pub native_language: String,
    pub chat_font_size: i32,
}

/// Milliseconds since the Unix epoch — the same clock `Date.now()` gives the
/// frontend, so timestamps compare directly on both sides.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// A process-wide, monotonically increasing suffix so ids stay unique even when
/// several messages land inside the same millisecond.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Generates an id in the same `m<millis>-<seq>` shape the frontend's in-browser
/// mock uses, so ids look consistent across both.
pub fn next_id() -> String {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("m{}-{n}", now_ms())
}

/// One chat line. Tagged by `kind`, matching the TypeScript `Message` union.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Message {
    /// A normal, readable message.
    #[serde(rename_all = "camelCase")]
    Conversation {
        id: String,
        group_id: String,
        persona_id: String,
        ts: i64,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        streaming: Option<bool>,
        /// AI persona ids that have successfully processed (read) this message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read_by: Option<Vec<String>>,
    },
    /// A persona broadcasting its current mood/emotion.
    #[serde(rename_all = "camelCase")]
    Mood {
        id: String,
        group_id: String,
        persona_id: String,
        ts: i64,
        mood: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A system notice that an agent's inference failed after exhausting retries.
    /// Surfaces the HTTP status and canonical reason only — never the provider's
    /// raw body, which can leak quota or key details. `personaId` is the agent
    /// that failed. The frontend renders it as an error line with a retry button.
    #[serde(rename_all = "camelCase")]
    System {
        id: String,
        group_id: String,
        persona_id: String,
        ts: i64,
        /// The HTTP status code, when the failure carried one (e.g. 429).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        /// The status's canonical reason (e.g. "Too Many Requests"), or a short
        /// generic label for a transport failure with no status.
        reason: String,
    },
}

impl Message {
    /// A fresh conversation line.
    pub fn conversation(
        group_id: impl Into<String>,
        persona_id: impl Into<String>,
        text: impl Into<String>,
        read_by: Option<Vec<String>>,
    ) -> Self {
        Message::Conversation {
            id: next_id(),
            group_id: group_id.into(),
            persona_id: persona_id.into(),
            ts: now_ms(),
            text: text.into(),
            streaming: None,
            read_by,
        }
    }

    /// A fresh mood line.
    pub fn mood(
        group_id: impl Into<String>,
        persona_id: impl Into<String>,
        mood: impl Into<String>,
        note: Option<String>,
    ) -> Self {
        Message::Mood {
            id: next_id(),
            group_id: group_id.into(),
            persona_id: persona_id.into(),
            ts: now_ms(),
            mood: mood.into(),
            note,
        }
    }

    /// A fresh system notice for a failed agent inference.
    pub fn system(
        group_id: impl Into<String>,
        persona_id: impl Into<String>,
        status: Option<u16>,
        reason: impl Into<String>,
    ) -> Self {
        Message::System {
            id: next_id(),
            group_id: group_id.into(),
            persona_id: persona_id.into(),
            ts: now_ms(),
            status,
            reason: reason.into(),
        }
    }

    /// The stable id used to look a message up in a group's log.
    pub fn id(&self) -> &str {
        match self {
            Message::Conversation { id, .. }
            | Message::Mood { id, .. }
            | Message::System { id, .. } => id,
        }
    }

    /// The message's creation timestamp (epoch millis) — the authoritative order
    /// key, shared with the client, used to page history around the read mark.
    pub fn ts(&self) -> i64 {
        match self {
            Message::Conversation { ts, .. }
            | Message::Mood { ts, .. }
            | Message::System { ts, .. } => *ts,
        }
    }
}

/// A single AI persona acknowledging it successfully processed a message —
/// whether or not it chose to reply (agents may read without replying).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadReceipt {
    pub group_id: String,
    pub message_id: String,
    pub persona_id: String,
}

/// How far one AI member has got in the current turn — the buckets the pinned
/// progress bar tints avatars by. Mirrors a read receipt's meaning but is
/// turn-scoped rather than message-scoped, so it works for an event trigger that
/// has no message to hang a receipt on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum TurnMemberState {
    /// Not yet processed this turn: still working, or never reached (a turn
    /// suspended by a failed inference leaves the agents after it pending).
    Pending,
    /// Processed and chose not to speak (read silently).
    Read,
    /// Processed and spoke.
    Replied,
}

/// One AI member's progress within a turn.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnMember {
    pub persona_id: String,
    pub state: TurnMemberState,
    /// The id of this member's first reply line this turn — the avatar's jump
    /// target. Present only once `state` is `replied`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_id: Option<String>,
}

/// What kicked off a turn: a conversation line someone sent, or an environment
/// event that carries no message of its own. Tagged by `kind` to match the
/// TypeScript union.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TurnTrigger {
    /// A user (or persona) message. `messageId` locates it in the log; `text` and
    /// `personaId` are carried so the bar can render the trigger even when that
    /// line is outside the client's loaded history window.
    #[serde(rename_all = "camelCase")]
    Message { message_id: String, persona_id: String, text: String },
    /// An environment event — rain, time passing, an emergency. `label` is the
    /// description shown; there is no message to jump to.
    #[serde(rename_all = "camelCase")]
    Event { label: String },
}

/// A processing round: what triggered it and how far each AI member has got.
/// Owned by the backend and streamed independently of message history (a named
/// `turn` SSE frame, seeded on connect), so the pinned progress bar reflects the
/// *current* processing state whether or not the trigger line is in the loaded
/// window — and shows progress for event triggers that have no user message at
/// all. `active` tracks whether the coordinator is still running the round.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: String,
    pub group_id: String,
    pub trigger: TurnTrigger,
    /// Epoch millis when the turn started.
    pub started_at: i64,
    /// Whether the coordinator is still running this turn.
    pub active: bool,
    /// The AI members participating, in the group's member order, each with its
    /// progress this round.
    pub members: Vec<TurnMember>,
}

/// Token usage for one LLM inference. Zero across the board when the provider
/// reports nothing; entirely absent (on the trace) for the rule-based mock,
/// which makes no LLM call.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Input ("prompt") tokens billed for this call.
    pub prompt_tokens: u64,
    /// Output ("completion") tokens billed for this call.
    pub completion_tokens: u64,
    /// Total tokens the provider reported (or input+output when it only gives
    /// the two).
    pub total_tokens: u64,
    /// Of the prompt tokens, how many were served from the provider's cache —
    /// billed cheaper. The basis for the cache-hit ratio and cache savings.
    pub cached_prompt_tokens: u64,
}

/// A debug record of one agent inference: exactly the system + context the
/// character's model received, what it decided, and the tokens it cost. Streamed
/// live as a `debug` SSE frame and available in bulk for panel hydration.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentTrace {
    pub ts: i64,
    pub group_id: String,
    pub persona_id: String,
    pub persona_name: String,
    /// The full system prompt the agent got (persona + variables + roster).
    pub system: String,
    /// The conversation/context text the agent read (the "public" messages).
    pub conversation: String,
    /// The chosen action: `speak` | `speakWithMood` | `mood` | `read`.
    pub action: String,
    /// The spoken line, when it spoke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The mood, when it showed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mood: Option<String>,
    /// Tokens this inference cost; absent for the mock brain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// The model that produced this inference (e.g. `gpt-4o-mini`); absent for
    /// the mock brain, which runs no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// An estimated cost breakdown for the accumulated usage. Always an estimate:
/// rates are operator-supplied and providers/models differ, so the UI labels it
/// "for reference only".
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    /// Currency label the rates are in (e.g. `USD`).
    pub currency: String,
    /// Cost of fresh (non-cached) input tokens.
    pub input: f64,
    /// Cost of cached input tokens.
    pub cached_input: f64,
    /// Cost of output tokens.
    pub output: f64,
    /// Sum of the above.
    pub total: f64,
}

impl Cost {
    /// Adds another cost breakdown into this one, component-wise. Used to
    /// accumulate the running total one trace at a time, at the rate that was
    /// in effect *when that trace was recorded* — so a later change to the
    /// configured rates never reprices history.
    pub fn add(self, other: Cost) -> Cost {
        Cost {
            currency: self.currency,
            input: self.input + other.input,
            cached_input: self.cached_input + other.cached_input,
            output: self.output + other.output,
            total: self.total + other.total,
        }
    }
}

/// One model's slice of the cumulative usage — e.g. after switching providers
/// or models mid-run, each keeps its own running total rather than blending
/// into a single undifferentiated number. Part of [`DebugUsage::models`].
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    /// The model name (e.g. `gpt-4o-mini`), or `"unknown"` for traces from a
    /// brain that runs no real model (the rule-based mock).
    pub model: String,
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_prompt_tokens: u64,
    /// Present once at least one costed trace has been recorded for this
    /// model; absent when pricing was never configured while it was in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Cost>,
}

/// Cumulative LLM usage across the whole server (since first run, when
/// persisted; since startup otherwise) — the global "total usage" view.
/// `GET /debug/usage`.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebugUsage {
    /// Number of agent inferences (LLM requests in LLM mode).
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_prompt_tokens: u64,
    /// Cached prompt tokens ÷ prompt tokens, in `0.0..=1.0`; `0` before any
    /// usage is seen. How much the cache is saving.
    pub cache_hit_ratio: f64,
    /// The running total, accrued one trace at a time at whatever rate was
    /// configured when each trace was recorded. Present once at least one
    /// costed trace has been recorded; absent when pricing has never been
    /// configured. Always an estimate — see [`Cost`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Cost>,
    /// The same totals broken down by model, largest (by total tokens) first.
    /// Empty until the first inference is recorded.
    #[serde(default)]
    pub models: Vec<ModelUsage>,
}

/// One persona's own slice of a group's usage — a further breakdown of that
/// group's [`DebugUsage`] by which character is driving the spend. `GET
/// /groups/{id}/debug/usage/by-persona`.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonaUsage {
    pub persona_id: String,
    pub usage: DebugUsage,
}

/// The `GET`/`PATCH /llm/settings` response: the live LLM provider
/// configuration, with the API key stripped to a presence flag. Built by hand
/// from [`crate::llm_config::LlmSettings`] (never derived by serializing it
/// directly) — that's the one place a leaked key would slip out.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LlmSettingsView {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Whether a key is currently stored. The key itself is never sent to a
    /// client once saved.
    pub has_api_key: bool,
    pub max_tokens: u64,
    pub max_rpm: u64,
    pub max_retries: u32,
    pub retry_base_ms: u64,
    pub compress_after: usize,
    pub compress_keep: usize,
    pub compress_max_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<crate::llm_config::Pricing>,
}

impl From<&crate::llm_config::LlmSettings> for LlmSettingsView {
    fn from(s: &crate::llm_config::LlmSettings) -> Self {
        Self {
            enabled: s.enabled,
            base_url: s.base_url.clone(),
            model: s.model.clone(),
            has_api_key: s.api_key.as_deref().is_some_and(|k| !k.is_empty()),
            max_tokens: s.max_tokens,
            max_rpm: s.max_rpm,
            max_retries: s.max_retries,
            retry_base_ms: s.retry_base_ms,
            compress_after: s.compress_after,
            compress_keep: s.compress_keep,
            compress_max_tokens: s.compress_max_tokens,
            pricing: s.pricing.clone(),
        }
    }
}

/// A partial update to the LLM provider configuration — every field is
/// optional; an absent field leaves the current value alone. `baseUrl` /
/// `model` / `apiKey` treat an empty string as "clear this field", matching the
/// endpoint's other optional-string convention (there's never a reason to
/// *store* an empty string here). `pricing` with both rates at zero clears the
/// configured pricing (shows token counts only) rather than pinning a $0 rate.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LlmSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The new key, or `""` to clear it. Omit entirely to leave the stored key
    /// untouched — the frontend must never send this unless the operator
    /// actually typed a new value, or every unrelated settings save would wipe
    /// the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rpm: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_base_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_keep: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<crate::llm_config::Pricing>,
}

impl LlmSettingsPatch {
    /// Merges this patch onto a base configuration, field by field.
    pub fn apply(self, base: &mut crate::llm_config::LlmSettings) {
        if let Some(v) = self.enabled {
            base.enabled = v;
        }
        if let Some(v) = self.base_url {
            base.base_url = non_empty(v);
        }
        if let Some(v) = self.model {
            base.model = non_empty(v);
        }
        if let Some(v) = self.api_key {
            base.api_key = non_empty(v);
        }
        if let Some(v) = self.max_tokens {
            base.max_tokens = v;
        }
        if let Some(v) = self.max_rpm {
            base.max_rpm = v;
        }
        if let Some(v) = self.max_retries {
            base.max_retries = v;
        }
        if let Some(v) = self.retry_base_ms {
            base.retry_base_ms = v;
        }
        if let Some(v) = self.compress_after {
            base.compress_after = v;
        }
        if let Some(v) = self.compress_keep {
            base.compress_keep = v;
        }
        if let Some(v) = self.compress_max_tokens {
            base.compress_max_tokens = v;
        }
        if let Some(v) = self.pricing {
            base.pricing = (v.input_per_m != 0.0 || v.output_per_m != 0.0).then_some(v);
        }
    }
}

/// Trims and treats blank as "unset" — the `""`-clears-a-field convention
/// [`LlmSettingsPatch::apply`] uses for its optional string fields.
fn non_empty(s: String) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The `POST /llm/models` request: which endpoint to list models from. `apiKey`
/// is optional — omitted, the backend uses the currently-stored key, but only
/// when `baseUrl` matches the stored endpoint (see `routes::llm`), so this
/// can't be used to make the server send its stored credential to an arbitrary
/// third-party URL.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelsQuery {
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// One model a provider's listing endpoint reported. `name` is a display label
/// when the provider offers one (Gemini does; OpenAI-compatible listings
/// usually don't), never required for choosing the model — `id` is what's
/// actually sent as `LlmSettings::model`.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The `POST /llm/models` response.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelsView {
    pub models: Vec<LlmModelInfo>,
}

#[cfg(test)]
mod llm_settings_patch_tests {
    use super::*;
    use crate::llm_config::LlmSettings;

    fn patch() -> LlmSettingsPatch {
        LlmSettingsPatch {
            enabled: None,
            base_url: None,
            model: None,
            api_key: None,
            max_tokens: None,
            max_rpm: None,
            max_retries: None,
            retry_base_ms: None,
            compress_after: None,
            compress_keep: None,
            compress_max_tokens: None,
            pricing: None,
        }
    }

    #[test]
    fn an_absent_field_leaves_the_stored_value_untouched() {
        let mut base = LlmSettings {
            api_key: Some("sk-existing".to_string()),
            ..LlmSettings::default()
        };
        patch().apply(&mut base);
        assert_eq!(base.api_key.as_deref(), Some("sk-existing"));
    }

    #[test]
    fn an_empty_string_clears_the_field() {
        let mut base = LlmSettings {
            api_key: Some("sk-existing".to_string()),
            ..LlmSettings::default()
        };
        LlmSettingsPatch {
            api_key: Some(String::new()),
            ..patch()
        }
        .apply(&mut base);
        assert_eq!(base.api_key, None);
    }

    #[test]
    fn a_non_empty_string_replaces_the_field() {
        let mut base = LlmSettings::default();
        LlmSettingsPatch {
            model: Some("gpt-4o-mini".to_string()),
            ..patch()
        }
        .apply(&mut base);
        assert_eq!(base.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn zero_rates_clear_pricing_instead_of_pinning_a_zero_cost() {
        let mut base = LlmSettings {
            pricing: Some(crate::llm_config::Pricing {
                input_per_m: 1.0,
                cached_input_per_m: 1.0,
                output_per_m: 1.0,
                currency: "USD".to_string(),
            }),
            ..LlmSettings::default()
        };
        LlmSettingsPatch {
            pricing: Some(crate::llm_config::Pricing {
                input_per_m: 0.0,
                cached_input_per_m: 0.0,
                output_per_m: 0.0,
                currency: "USD".to_string(),
            }),
            ..patch()
        }
        .apply(&mut base);
        assert!(base.pricing.is_none());
    }
}
