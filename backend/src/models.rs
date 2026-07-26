//! Wire types shared with the frontend.
//!
//! These mirror `frontend/src/types.ts` exactly on the wire: the `Message`
//! discriminated union is internally tagged by `kind`, and every field is
//! camelCased so the JSON round-trips through the client without translation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
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
}

/// An estimated cost breakdown for the accumulated usage. Always an estimate:
/// rates are operator-supplied and providers/models differ, so the UI labels it
/// "for reference only".
#[derive(Clone, Debug, Serialize, ToSchema)]
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

/// Cumulative LLM usage across the whole server since startup — the global
/// "total usage" view. `GET /debug/usage`.
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
    /// Present only when pricing is configured. An estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<Cost>,
}
