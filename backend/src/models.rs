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

    /// The stable id used to look a message up in a group's log.
    pub fn id(&self) -> &str {
        match self {
            Message::Conversation { id, .. } | Message::Mood { id, .. } => id,
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
