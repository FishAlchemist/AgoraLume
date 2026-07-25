//! The `AgentBrain` seam — the single piece a real LLM replaces.
//!
//! The orchestrator does the context work: it resolves the persona, filters the
//! conversation, retrieves memories, and folds in events, then assembles them
//! into an [`AgentPrompt`]. A brain is a pure "prompt in → decision out"
//! function — exactly the LLM boundary. A real implementation sends the prompt's
//! `system` + `conversation` to a model and parses its `respond` tool call; the
//! bundled [`crate::agent::mock::RuleBrain`] reads the convenience fields and
//! applies rules. The orchestrator never knows which it is.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::models::TokenUsage;

/// One of the four mutually-exclusive things an agent may do on its turn — the
/// entire surface of the `respond` tool. The orchestrator routes each variant to
/// the two streams (Context / UI View); see `agent::turn`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A spoken line. Enters the Context Stream and the UI.
    Speak,
    /// A line plus a mood. The line enters Context + UI; the mood is
    /// UI-only (moods never enter the Context other agents read).
    SpeakWithMood,
    /// A mood only. UI-only; never the Context Stream.
    Mood,
    /// Processed without responding. Nothing is broadcast.
    Read,
}

/// The output of a single agent turn: exactly what the `respond` tool carries.
#[derive(Clone, Debug)]
pub struct Respond {
    pub action: Action,
    /// Required for [`Action::Speak`] / [`Action::SpeakWithMood`].
    pub message: Option<String>,
    /// Required for [`Action::Mood`] / [`Action::SpeakWithMood`].
    pub mood: Option<String>,
}

impl Respond {
    /// Read — processed silently; nothing is broadcast.
    pub fn read() -> Self {
        Self { action: Action::Read, message: None, mood: None }
    }

    /// Speak — a spoken line.
    pub fn speak(message: impl Into<String>) -> Self {
        Self { action: Action::Speak, message: Some(message.into()), mood: None }
    }

    /// Speak with mood — a spoken line plus a UI-only mood.
    pub fn speak_with_mood(message: impl Into<String>, mood: impl Into<String>) -> Self {
        Self {
            action: Action::SpeakWithMood,
            message: Some(message.into()),
            mood: Some(mood.into()),
        }
    }

    /// Mood — a UI-only mood, no spoken line.
    pub fn mood(mood: impl Into<String>) -> Self {
        Self { action: Action::Mood, message: None, mood: Some(mood.into()) }
    }
}

/// A resolved view of the persona an agent speaks as: its identity plus the
/// inherited template variables the model needs. Assembled by the orchestrator
/// from the workspace (the SSOT), then rendered into an [`AgentPrompt`].
#[derive(Clone, Debug)]
pub struct AgentPersona {
    pub name: String,
    pub system_prompt: String,
    pub variables: HashMap<String, String>,
}

/// One entry in the global member directory an agent can consult by name. Unlike
/// the group roster (who is in *this* room, injected into every prompt), the
/// directory covers *every* persona in the workspace and is queried on demand
/// through the `lookup_member` tool — so the prompt stays small while the agent
/// can still look anyone up by their globally-unique name.
#[derive(Clone, Debug)]
pub struct MemberInfo {
    pub name: String,
    pub blurb: Option<String>,
    /// True for the single human identity ("you").
    pub is_user: bool,
}

/// The fully-assembled prompt handed to a brain — the self-managed context, in
/// the form a model consumes. `system` carries the persona (with variables);
/// `conversation` carries the clean transcript plus injected environment events.
/// `persona_name` and `last_line` are conveniences so a non-LLM brain need not
/// re-parse the text.
#[derive(Clone, Debug)]
pub struct AgentPrompt {
    pub system: String,
    pub conversation: String,
    pub persona_name: String,
    pub last_line: Option<String>,
    /// Every persona in the workspace, so a brain can offer a `lookup_member`
    /// tool that resolves a name to its blurb on demand. Empty for brains/tests
    /// that don't use it; the mock ignores it.
    pub directory: Vec<MemberInfo>,
}

/// A brain's output: the decision plus optional telemetry. Keeping usage
/// alongside the decision lets the orchestrator record token cost without the
/// brain reaching into app state — it stays a pure prompt-in/decision-out
/// function. The mock and providers that report nothing leave `usage` `None`.
#[derive(Clone, Debug)]
pub struct Decision {
    pub respond: Respond,
    pub usage: Option<TokenUsage>,
}

impl From<Respond> for Decision {
    /// A decision with no usage telemetry — for the mock and test brains.
    fn from(respond: Respond) -> Self {
        Self { respond, usage: None }
    }
}

/// The single inference seam. A real implementation sends the prompt to an LLM
/// and parses its `respond` tool call; the mock applies deterministic rules.
/// Implementations must be cancel-safe: dropping the returned future aborts the
/// agent cleanly (a hard interrupt) — the orchestrator discards it and nothing
/// is left half-written.
#[async_trait]
pub trait AgentBrain: Send + Sync {
    async fn decide(&self, prompt: &AgentPrompt) -> Decision;
}
