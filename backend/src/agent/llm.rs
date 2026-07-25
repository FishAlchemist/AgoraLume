//! The LLM-backed [`AgentBrain`] — a rig-core extractor over an
//! OpenAI-compatible chat endpoint.
//!
//! This is the real implementation of the single inference seam. The
//! orchestrator still owns all context work; this brain just turns an
//! [`AgentPrompt`] into a structured `respond` decision by asking a model.
//! Everything about the endpoint is configured (base URL, model, key) — nothing
//! is hard-coded to a provider, so it drives OpenAI, OpenRouter, or a local
//! Ollama / llama.cpp server equally.

use async_trait::async_trait;
use rig_core::client::completion::CompletionClient;
use rig_core::completion::{Prompt, Usage};
use rig_core::providers::openai;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::brain::{AgentBrain, AgentPrompt, Decision, MemberInfo, Respond};
use crate::models::TokenUsage;

/// Model-call budget for one decision: the initial call plus any `lookup_member`
/// round-trips and the final structured answer. Bounded so a chatty model can't
/// loop indefinitely.
const MAX_DECISION_TURNS: usize = 5;

/// The `respond` tool's arguments, as the model returns them. Kept separate from
/// [`Respond`] so the wire schema the model sees stays decoupled from the
/// orchestrator's internal type. rig derives the JSON schema from this.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct RespondArgs {
    /// The single action to take this turn.
    action: ActionKind,
    /// The spoken line. Required for `speak` and `speak_with_mood`.
    #[serde(default)]
    message: Option<String>,
    /// A short mood label, optionally led by an emoji (e.g. "🤔 thinking").
    /// Required for `mood` and `speak_with_mood`.
    #[serde(default)]
    mood: Option<String>,
}

/// The four mutually-exclusive turn actions, named as the model should emit them.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ActionKind {
    /// Say something to the group.
    Speak,
    /// Say something and show a mood.
    SpeakWithMood,
    /// Show a mood only, without speaking.
    Mood,
    /// Stay silent this turn.
    Read,
}

/// Maps the model's decision onto a [`Respond`], degrading gracefully when the
/// field an action needs is missing (e.g. `speak` with no message → stay silent
/// rather than emit an empty line).
fn to_respond(args: RespondArgs) -> Respond {
    let message = args.message.filter(|m| !m.trim().is_empty());
    let mood = args.mood.filter(|m| !m.trim().is_empty());
    match args.action {
        ActionKind::Speak => message.map_or_else(Respond::read, Respond::speak),
        ActionKind::SpeakWithMood => match (message, mood) {
            (Some(m), Some(md)) => Respond::speak_with_mood(m, md),
            (Some(m), None) => Respond::speak(m),
            (None, Some(md)) => Respond::mood(md),
            (None, None) => Respond::read(),
        },
        ActionKind::Mood => mood.map_or_else(Respond::read, Respond::mood),
        ActionKind::Read => Respond::read(),
    }
}

/// How the agent is told to use the `respond` schema and the tools it has.
/// Appended *after* the persona's own system prompt, so character comes first
/// and mechanics second.
const GUIDANCE: &str = "\
You are one participant in a group text chat. The context above is given in \
XML-tagged sections (<persona>, <context>, <group_members>) and the message \
below carries the current <time> (with timezone), the live <conversation>, and \
any <environment> events. You may call the `lookup_member` tool to look up any \
member — including people not in this room — by their exact, globally-unique \
name to read their short bio before you reply. When ready, decide your single \
next action using the response schema. Speak only when you have something worth \
adding; otherwise choose `read` to stay silent. Moods are UI-only flavour and \
are never shown to other participants as text. Keep any reply to one short chat \
message.";

/// Maps rig's completion usage onto our wire [`TokenUsage`]. `total_tokens`
/// falls back to input+output when the provider reports only the two parts.
fn to_token_usage(usage: Usage) -> TokenUsage {
    let total = if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input_tokens + usage.output_tokens
    };
    TokenUsage {
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: total,
        cached_prompt_tokens: usage.cached_input_tokens,
    }
}

/// An [`AgentBrain`] that drives decisions with an OpenAI-compatible chat model
/// via rig-core. Endpoint, model, and key all come from config.
pub struct LlmBrain {
    client: openai::CompletionsClient,
    model: String,
    max_tokens: u64,
}

impl LlmBrain {
    /// Builds the client against an OpenAI-compatible endpoint. `api_key` may be
    /// empty for local servers that need no auth. Fails only if the client can't
    /// be constructed.
    pub fn new(
        base_url: &str,
        model: impl Into<String>,
        api_key: &str,
        max_tokens: u64,
    ) -> Result<Self, String> {
        let client = openai::CompletionsClient::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|e| format!("failed to build the LLM client: {e}"))?;
        Ok(Self { client, model: model.into(), max_tokens })
    }
}

#[async_trait]
impl AgentBrain for LlmBrain {
    async fn decide(&self, prompt: &AgentPrompt) -> Decision {
        // The persona (system) becomes the agent preamble; the clean transcript
        // (plus <time>) is the text to reason over. Built per turn because the
        // preamble is persona-specific and the directory changes with membership.
        let preamble = format!("{}\n\n{GUIDANCE}", prompt.system.trim());

        // `RespondArgs` as the structured output schema + the `lookup_member`
        // tool. The default `OutputMode::Auto` resolves to the tool-call output
        // mode when a schema and tools are both present, so the model can freely
        // call `lookup_member` first and *then* finalize via the output tool —
        // which composes with tools across providers (incl. local Ollama), unlike
        // native structured output.
        let agent = self
            .client
            .agent(self.model.as_str())
            .preamble(&preamble)
            .max_tokens(self.max_tokens)
            .output_schema::<RespondArgs>()
            .tool(LookupMemberTool { members: prompt.directory.clone() })
            .build();

        // Extended details carry the token usage accumulated across the whole
        // tool loop, which feeds the debug/cost panel.
        let result = agent
            .prompt(prompt.conversation.clone())
            .max_turns(MAX_DECISION_TURNS)
            .extended_details()
            .await;

        match result {
            Ok(response) => {
                let respond = serde_json::from_str::<RespondArgs>(response.output.trim())
                    .map(to_respond)
                    .unwrap_or_else(|e| {
                        // The loop finished but the final text wasn't the expected
                        // structured decision; stay silent rather than guess.
                        tracing::warn!(
                            persona = %prompt.persona_name,
                            error = %e,
                            output = %response.output,
                            "could not parse the agent's structured decision; treating as read"
                        );
                        Respond::read()
                    });
                Decision { respond, usage: Some(to_token_usage(response.usage)) }
            }
            Err(e) => {
                // A model or transport failure must not take down the turn: the
                // agent simply stays silent this round.
                tracing::warn!(
                    persona = %prompt.persona_name,
                    error = %e,
                    "LLM decide failed; treating as read (silent)"
                );
                Respond::read().into()
            }
        }
    }
}

/// The `lookup_member` tool: resolves a globally-unique member name to their
/// blurb, on demand, from a snapshot of the workspace directory captured when the
/// prompt was assembled. Lets an agent look up anyone by name without every
/// persona being dumped into the prompt.
struct LookupMemberTool {
    members: Vec<MemberInfo>,
}

/// The `lookup_member` arguments the model supplies.
#[derive(Debug, Deserialize)]
struct LookupArgs {
    /// The exact, globally-unique display name of the member to look up.
    name: String,
}

/// What `lookup_member` returns to the model.
#[derive(Debug, Serialize)]
struct LookupOutput {
    /// Whether a member with that name exists.
    found: bool,
    /// The member's name, echoed back when found.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// The member's short bio, when they have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    blurb: Option<String>,
    /// True when this member is the human user ("you").
    is_user: bool,
}

/// `lookup_member` never fails — an unknown name is a normal `found: false`
/// result — but the [`Tool`] trait requires an error type.
#[derive(Debug)]
struct LookupError;

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "member lookup failed")
    }
}

impl std::error::Error for LookupError {}

impl Tool for LookupMemberTool {
    const NAME: &'static str = "lookup_member";
    type Error = LookupError;
    type Args = LookupArgs;
    type Output = LookupOutput;

    fn description(&self) -> String {
        "Look up a member (any persona in the workspace, including people not in \
         this chat, and the user) by their exact, globally-unique name and return \
         their short bio. Returns found=false when no member has that name."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The exact, globally-unique display name of the member to look up."
                }
            },
            "required": ["name"]
        })
    }

    async fn call(&self, args: LookupArgs) -> Result<LookupOutput, LookupError> {
        let query = args.name.trim();
        let found = self
            .members
            .iter()
            .find(|m| m.name.trim().eq_ignore_ascii_case(query));
        Ok(match found {
            Some(m) => LookupOutput {
                found: true,
                name: Some(m.name.clone()),
                blurb: m.blurb.clone(),
                is_user: m.is_user,
            },
            None => LookupOutput { found: false, name: None, blurb: None, is_user: false },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::brain::Action;

    fn args(action: ActionKind, message: Option<&str>, mood: Option<&str>) -> RespondArgs {
        RespondArgs {
            action,
            message: message.map(str::to_string),
            mood: mood.map(str::to_string),
        }
    }

    #[test]
    fn maps_each_action() {
        let r = to_respond(args(ActionKind::Speak, Some("hi"), None));
        assert_eq!(r.action, Action::Speak);
        assert_eq!(r.message.as_deref(), Some("hi"));

        let r = to_respond(args(ActionKind::SpeakWithMood, Some("hi"), Some("🙂 glad")));
        assert_eq!(r.action, Action::SpeakWithMood);
        assert_eq!(r.mood.as_deref(), Some("🙂 glad"));

        let r = to_respond(args(ActionKind::Mood, None, Some("🤔 thinking")));
        assert_eq!(r.action, Action::Mood);
        assert_eq!(r.mood.as_deref(), Some("🤔 thinking"));

        assert_eq!(to_respond(args(ActionKind::Read, None, None)).action, Action::Read);
    }

    #[test]
    fn degrades_when_required_field_missing() {
        // `speak` with no message → silent, not an empty line.
        assert_eq!(to_respond(args(ActionKind::Speak, None, None)).action, Action::Read);
        // whitespace-only counts as missing.
        assert_eq!(to_respond(args(ActionKind::Speak, Some("   "), None)).action, Action::Read);
        // `speak_with_mood` missing the mood still speaks; missing the line drops
        // to mood-only.
        assert_eq!(
            to_respond(args(ActionKind::SpeakWithMood, Some("hi"), None)).action,
            Action::Speak
        );
        assert_eq!(
            to_respond(args(ActionKind::SpeakWithMood, None, Some("🙂 glad"))).action,
            Action::Mood
        );
        // `mood` with no mood → silent.
        assert_eq!(to_respond(args(ActionKind::Mood, None, None)).action, Action::Read);
    }
}
