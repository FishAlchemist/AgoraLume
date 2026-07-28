//! The LLM-backed [`AgentBrain`] — rig-core structured output over a configured
//! chat endpoint.
//!
//! This is the real implementation of the single inference seam. The
//! orchestrator still owns all context work; this brain just turns an
//! [`AgentPrompt`] into a structured `respond` decision by asking a model.
//!
//! The backend is not coupled to any one provider. Everything about the endpoint
//! is configured (base URL, model, key), so by default an OpenAI-compatible
//! [`Provider::OpenAi`] client drives OpenAI, OpenRouter, or a local Ollama /
//! llama.cpp server equally. The one exception is a base URL that points at
//! Gemini's OpenAI-compat shim (`generativelanguage.googleapis.com/…/openai/`):
//! it is auto-detected and redirected to rig's *native* Gemini provider
//! ([`Provider::Gemini`]), logged when that happens. The compat wire format has
//! no field for Gemini's `thoughtSignature`, which the native path round-trips —
//! so anything that relies on it (a tool / thought-carrying turn) works only on
//! the native client, not the compat one.
//!
//! Persona memory is two pull tools rig exposes to the model. Writing is always
//! available: every decision registers a `remember` tool so the agent can save an
//! in-character fact when one comes up. Reading is conditional: when the
//! orchestrator resolves recallable memories for the persona
//! ([`AgentPrompt::recallable_memories`]) a `recall_memory` tool holding them is
//! registered too, so the model can look them up *on demand*. Member lookups, by
//! contrast, are handed to the agent up front in the prompt's `<directory>`
//! section rather than a tool.
//!
//! Because a decision always carries the `remember` tool, `OutputMode::Auto`
//! resolves to `Tool` mode (the response schema becomes a synthetic `final_result`
//! tool the model calls to finalize) on providers whose native constraint would
//! otherwise suppress tool calls — `RespondArgs`/`validate_decision` are
//! unchanged, rig handles the mode switch. Crucially this costs nothing when the
//! tools go unused: rig finalizes the moment the model calls `final_result`, so a
//! turn that neither remembers nor recalls is still a single request. Only an
//! actual tool call spends an extra round-trip, which is why the decision budget
//! is raised past rig's default of one call (see `MEMORY_MAX_TURNS`). A silent
//! "read" therefore stays the model's own choice, not a swallowed tool-call error.
//! The summary path registers no tools, so it keeps native single-request output.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rig_core::agent::PromptResponse;
use rig_core::client::completion::CompletionClient;
use rig_core::completion::{Prompt, PromptError, Usage};
use rig_core::providers::{gemini, openai};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::brain::{
    AgentBrain, AgentPrompt, BrainError, Decision, Outcome, Respond, SuggestionRequest, Suggestions,
    Summary, SummaryRequest,
};
use crate::agent::ratelimit::RateLimiter;
use crate::models::TokenUsage;

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

/// Why a transport-successful completion still can't be used: the model
/// answered, but the answer isn't a usable decision. Distinct from a
/// [`PromptError`] (a transport/provider failure) — these are the model's own bad
/// output, retried with a corrective hint rather than backoff.
#[derive(Debug)]
enum BadOutput {
    /// Not valid `RespondArgs` JSON — malformed, or (as weak models often do)
    /// truncated mid-string when the reply overran the token cap.
    Malformed,
    /// Parsed, but a field is runaway repetition — a weak model looping the same
    /// phrase or emoji cluster. That's chat spam and wasted tokens, not a reply.
    Repetitive,
}

impl BadOutput {
    /// The corrective hint appended to the next attempt, naming what went wrong so
    /// the retry steers away from repeating it.
    fn hint(&self) -> &'static str {
        match self {
            BadOutput::Malformed => {
                "Your previous reply was not a single valid JSON object for the response \
                 schema (it may have been cut off). Reply again with one short, complete \
                 JSON decision."
            }
            BadOutput::Repetitive => {
                "Your previous reply repeated the same characters or phrase many times. Reply \
                 again with one short, natural chat message — no repetition or filler."
            }
        }
    }

    /// The short, sanitized reason surfaced to the chat once retries are spent —
    /// never the raw model output (which could be a wall of spam).
    fn reason(&self) -> &'static str {
        match self {
            BadOutput::Malformed => "model returned an unparseable response",
            BadOutput::Repetitive => "model returned a repetitive response",
        }
    }
}

/// Turns a completion's raw text into a decision, rejecting output that can't be
/// used. A *valid but under-specified* decision (e.g. `speak` with no message)
/// still degrades to a legitimate silent read via [`to_respond`] — only output
/// that is malformed/truncated JSON or runaway repetition is an error worth a
/// retry.
fn validate_decision(output: &str) -> Result<Respond, BadOutput> {
    let args =
        serde_json::from_str::<RespondArgs>(output.trim()).map_err(|_| BadOutput::Malformed)?;
    if args.message.as_deref().is_some_and(looks_repetitive)
        || args.mood.as_deref().is_some_and(looks_repetitive)
    {
        return Err(BadOutput::Repetitive);
    }
    Ok(to_respond(args))
}

/// Whether `s` is dominated by runaway repetition — a short unit repeated
/// consecutively enough to be spam rather than speech. Tuned to fire only on
/// clear loops: a unit of up to `MAX_PERIOD` chars repeated at least
/// `MIN_REPEATS` times and spanning at least `MIN_COVER` chars, so ordinary
/// emphasis ("!!!", "哈哈哈") stays well under the bar.
fn looks_repetitive(s: &str) -> bool {
    const MAX_PERIOD: usize = 20;
    const MIN_REPEATS: usize = 8;
    const MIN_COVER: usize = 60;
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    if n < MIN_COVER {
        return false;
    }
    for period in 1..=MAX_PERIOD.min(n / 2) {
        let mut i = 0;
        while i + period <= n {
            // How many times the window [i, i+period) repeats back-to-back.
            let mut j = i + period;
            let mut repeats = 1;
            while j + period <= n && chars[i..i + period] == chars[j..j + period] {
                repeats += 1;
                j += period;
            }
            if repeats >= MIN_REPEATS && repeats * period >= MIN_COVER {
                return true;
            }
            // Jump past a measured run; otherwise slide one char to the next
            // alignment.
            i = if repeats > 1 { j } else { i + 1 };
        }
    }
    false
}

/// How the agent is told to use the `respond` schema and the context it has.
/// Appended *after* the persona's own system prompt, so character comes first
/// and mechanics second.
const GUIDANCE: &str = "\
You are one participant in a group text chat. The context above is given in \
XML-tagged sections: <persona> and <context> are you; <group_members> lists who \
is in this room; <directory> lists other members of the workspace you may refer \
to by their exact, globally-unique name (with their short bio) even though they \
are not here. The message below may open with a <summary> of earlier \
conversation that has scrolled out of view — treat it as established background — \
then carries the live <conversation>, then any <environment> events, and ends \
with the current <time> (with timezone). Inside \
<conversation>, each line is a <message from=\"NAME\" time=\"TIMESTAMP\">…</message> \
element: `from` is the speaker and `time` is when they sent it (same timezone as \
<time>), so you can judge what is recent and what is stale. Decide your single next \
action using the response schema. Speak only when you have something worth \
adding; otherwise choose `read` to stay silent. Moods are UI-only flavour and \
are never shown to other participants as text. Keep any reply to one short chat \
message.";

/// Appended to every decision preamble, since the `remember` tool is always
/// offered. Short by design: the tool's own description carries the detail; this
/// just tells the model when saving is worthwhile and to keep it invisible.
const REMEMBER_GUIDANCE: &str = "\
You have a remember tool that saves a durable fact about this conversation for \
your future self as this character. Use it sparingly, only for something worth \
recalling in a later conversation (a stated preference, a commitment, a lasting \
detail) — not passing chit-chat. Never mention the tool itself to the group.";

/// Appended to the preamble only when a `recall_memory` tool is also registered,
/// so a persona with no memories is never told about a tool it doesn't have. Short
/// by design: the tool's own description carries the detail; this just points at it.
const RECALL_GUIDANCE: &str = "\
You also have a recall_memory tool holding things you have remembered as this \
character. Use it only when recalling a saved detail would actually change your \
reply — most turns need no lookup. Never mention the tool itself to the group.";

/// The model-call budget for a decision. rig's default is a single call, but a
/// decision always carries the `remember` tool (and often `recall_memory` too), so
/// a turn that uses one needs the continuation after the tool runs, and the turn
/// that uses both needs a second — plus the `final_result` call that carries the
/// decision. This ceiling covers remember + recall + finalize while capping how
/// many requests one turn can spend if the model over-uses a tool. A turn that
/// calls no tool still finalizes in one request, well under the ceiling.
const MEMORY_MAX_TURNS: usize = 3;

/// The persona's recallable memory, exposed to the model as a pull tool. It holds
/// the contents the orchestrator already resolved (current identity only), so
/// calling it touches no app state — the brain stays a pure prompt-in/decision-out
/// function. Registered only when there is something to recall.
struct RecallMemory {
    memories: Vec<String>,
}

/// The recall tool's arguments: an optional free-text filter. Omitted (or blank)
/// returns everything the persona currently remembers.
#[derive(Debug, Deserialize)]
struct RecallArgs {
    #[serde(default)]
    query: Option<String>,
}

/// Recall never actually fails — its data is already in memory — but [`Tool`]
/// requires an error type. This unit satisfies the bound and is never returned.
#[derive(Debug)]
struct RecallError;

impl std::fmt::Display for RecallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("memory recall failed")
    }
}

impl std::error::Error for RecallError {}

impl Tool for RecallMemory {
    const NAME: &'static str = "recall_memory";
    type Error = RecallError;
    type Args = RecallArgs;
    type Output = Vec<String>;

    fn description(&self) -> String {
        "Look up what you have been asked to remember — durable facts, preferences, \
         and details saved for you as this character. Optionally pass `query` with \
         keywords to narrow the results; omit it to see everything you remember. \
         Call this only when recalling would change your reply."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Optional keywords to filter memories; omit to return all."
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let found = match args.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
            Some(query) => {
                let needle = query.to_lowercase();
                self.memories.iter().filter(|m| m.to_lowercase().contains(&needle)).cloned().collect()
            }
            None => self.memories.clone(),
        };
        Ok(found)
    }
}

/// The `remember` tool: the model's write path into persona memory. It collects
/// what the model chose to remember into a shared `sink`, which the caller drains
/// after the prompt loop and hands back on the [`Decision`] for the orchestrator
/// to persist — the tool never touches app state, so the brain stays a pure
/// function. Blank or duplicate content is dropped here so a confused model can't
/// spam the store. Registered on every decision (writing is always available).
struct RememberMemory {
    sink: Arc<Mutex<Vec<String>>>,
}

/// The remember tool's arguments: the single fact to save, in the model's words.
#[derive(Debug, Deserialize)]
struct RememberArgs {
    content: String,
}

/// Remembering never actually fails — it only appends to an in-memory sink — but
/// [`Tool`] requires an error type. This unit satisfies the bound and is never
/// returned.
#[derive(Debug)]
struct RememberError;

impl std::fmt::Display for RememberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("memory write failed")
    }
}

impl std::error::Error for RememberError {}

impl Tool for RememberMemory {
    const NAME: &'static str = "remember";
    type Error = RememberError;
    type Args = RememberArgs;
    type Output = String;

    fn description(&self) -> String {
        "Save one durable fact about this conversation for your future self as this \
         character — a stated preference, a commitment, a lasting detail worth \
         recalling in a later conversation. Pass the fact as `content`. Use it \
         sparingly; do not save passing chit-chat."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The single fact to remember, in your own words."
                }
            },
            "required": ["content"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let content = args.content.trim();
        if !content.is_empty() {
            let mut sink = self.sink.lock().unwrap();
            // Drop a duplicate so a model that calls the tool twice in one turn
            // doesn't store the same fact twice.
            if !sink.iter().any(|existing| existing == content) {
                sink.push(content.to_string());
            }
        }
        Ok("Saved.".to_string())
    }
}

/// How the model is told to compress older history. Deliberately narrow: produce
/// *notes*, not a reply, and keep the facts that outlive small talk — so the
/// running summary stays a faithful, compact stand-in for what it replaces.
const SUMMARY_GUIDANCE: &str = "\
You maintain a running summary of an ongoing group chat so its older history can \
be dropped from context without losing what matters. You are given the summary \
so far (if any) inside <summary_so_far>, then the next batch of older messages \
inside <older_messages>. Produce ONE updated summary that folds the new batch \
into the prior one. Preserve durable facts: decisions made, commitments, \
participants and their stated views, open questions, and anything a later reply \
would need to stay consistent. Drop greetings, filler, and resolved small talk. \
Write compact plain notes (short bullet points or a few short paragraphs), in \
the conversation's own language — not a message addressed to anyone. Output only \
the summary.";

/// The suggestion generator's structured output: the opener lines the model
/// proposes. A plain list schema — no tools — so it stays a single request.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct SuggestArgs {
    /// A few short, first-person messages the user could send next.
    suggestions: Vec<String>,
}

/// How the model is told to produce conversation-starter suggestions. It writes
/// openers *as the user*, short and time-appropriate, in the user's language — so
/// each one can be sent verbatim. The data (roster, summary, recent lines, time)
/// arrives in the tagged input; this preamble is the standing instruction.
const SUGGEST_GUIDANCE: &str = "\
You help a user who isn't sure what to say next in a group text chat. You are \
given who is in the room in <group_members> (the human is marked \"(the user)\"), \
an optional <summary> of earlier talk, the recent <conversation>, the user's \
language in <language>, and the current <time> with a coarse part of day. \
Propose a few short messages THE USER could send next to start or revive the \
conversation. Rules: write in the first person AS the user (not as any AI \
member); keep each to one short, natural sentence that can be sent verbatim; fit \
the part of day (never ask about the evening in the morning, or vice versa); and \
prefer variety — a question, a fresh topic, a follow-up on what was just said. \
Write them in the user's language when <language> is given, otherwise match the \
conversation's language. Output only the response schema.";

/// Output-token ceiling for one suggestion pass — a short list, so far under a
/// reply's cap. Kept local (not a config knob) since the payload is tiny and
/// fixed in shape.
const SUGGEST_MAX_TOKENS: u64 = 300;

/// How many suggestions to keep at most, however many the model returns — enough
/// for variety without crowding the composer.
const SUGGEST_MAX_ITEMS: usize = 6;

/// An upper bound on any single retry backoff, so a misconfigured base or a
/// large retry count can't park an agent (and its turn) for minutes.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Whether a failure with this HTTP status is worth an automatic retry. Server
/// overload and rate-limit responses (and transport failures, which carry no
/// status) are transient; other client errors (400/401/403/404, …) are
/// permanent — retrying just wastes the quota, so they go straight to the chat
/// for the user to act on.
fn is_retryable(status: Option<u16>) -> bool {
    match status {
        Some(code) => matches!(code, 408 | 429 | 500 | 502 | 503 | 504),
        // No status means a transport/timeout error rather than a provider
        // verdict — transient, so a bounded retry is worth it.
        None => true,
    }
}

/// Distills a rig prompt failure into the sanitized parts that may leave the
/// brain: the HTTP status code and its canonical name only, never the provider's
/// raw body (which can carry quota specifics or the key). Falls back to a generic
/// reason when the failure carried no HTTP status (a transport error).
fn classify_failure(error: &rig_core::completion::PromptError) -> (Option<u16>, String) {
    let status = error.provider_response_status();
    let reason = status
        .and_then(|s| s.canonical_reason())
        .map(str::to_string)
        .unwrap_or_else(|| "Request failed".to_string());
    (status.map(|s| s.as_u16()), reason)
}

/// A server-suggested delay before retrying, when the provider offers one. rig
/// 0.40 doesn't surface response *headers* (so a standard `Retry-After` header is
/// out of reach), but Gemini — the motivating free tier — puts the hint in the
/// JSON body as a `RetryInfo` with `retryDelay: "48s"`, and some APIs use a
/// top-level `retry_after`/`retryAfter` in seconds. We honor whichever we find.
fn retry_hint(error: &rig_core::completion::PromptError) -> Option<Duration> {
    let json = error.provider_response_json().ok().flatten()?;
    // Gemini: `error.details[]` carries a RetryInfo `{ retryDelay: "48s" }`.
    if let Some(details) = json.pointer("/error/details").and_then(|d| d.as_array()) {
        for item in details {
            if let Some(delay) = item.get("retryDelay").and_then(|v| v.as_str())
                && let Some(d) = parse_duration_secs(delay)
            {
                return Some(d);
            }
        }
    }
    // Generic: a top-level retry-after in seconds.
    for key in ["retry_after", "retryAfter"] {
        if let Some(secs) = json.get(key).and_then(serde_json::Value::as_f64) {
            return Some(Duration::from_secs_f64(secs.max(0.0)));
        }
    }
    None
}

/// Parses a Go-style seconds duration such as `"48s"` or `"1.5s"` (the shape
/// Gemini's `retryDelay` uses). A bare number without the `s` suffix is accepted
/// too. Negative or unparseable values yield `None`.
fn parse_duration_secs(s: &str) -> Option<Duration> {
    let trimmed = s.trim();
    let secs = trimmed.strip_suffix('s').unwrap_or(trimmed);
    secs.parse::<f64>().ok().filter(|v| *v >= 0.0).map(Duration::from_secs_f64)
}

/// A cheap `[0.0, 1.0)` pseudo-random from the wall clock, hashed with SplitMix64
/// so successive calls — and separate server processes — diverge. Enough to
/// jitter a retry backoff (de-synchronizing many clients that failed at once)
/// without pulling in an RNG crate, matching the loop's own hand-rolled RNG.
fn jitter_unit() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Top 53 bits mapped into the unit interval.
    (z >> 11) as f64 / (1u64 << 53) as f64
}

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

/// Sums two usage records so a decision reached after retries reports the tokens
/// every attempt cost. Saturating, so a misreporting provider can't wrap.
fn add_usage(a: TokenUsage, b: TokenUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: a.prompt_tokens.saturating_add(b.prompt_tokens),
        completion_tokens: a.completion_tokens.saturating_add(b.completion_tokens),
        total_tokens: a.total_tokens.saturating_add(b.total_tokens),
        cached_prompt_tokens: a.cached_prompt_tokens.saturating_add(b.cached_prompt_tokens),
    }
}

/// A short, single-line preview of model output for logs, so rejecting a wall of
/// repetition doesn't itself flood the log with that same spam.
fn preview(s: &str) -> String {
    const MAX: usize = 160;
    let trimmed = s.trim();
    let mut out: String = trimmed.chars().take(MAX).collect();
    if trimmed.chars().nth(MAX).is_some() {
        out.push('…');
    }
    out
}

/// Everything needed to build an [`LlmBrain`], grouped so the constructor stays
/// one clear config bag instead of a long positional argument list.
pub struct LlmConfig<'a> {
    /// OpenAI-compatible API root, e.g. `https://api.openai.com/v1`.
    pub base_url: &'a str,
    /// Model name to request.
    pub model: &'a str,
    /// Bearer key; empty for local endpoints that need none.
    pub api_key: &'a str,
    /// Output-token ceiling for one agent reply.
    pub max_tokens: u64,
    /// Output-token ceiling for one compression call — larger, since a running
    /// summary is a cumulative digest rather than a single short reply.
    pub summary_max_tokens: u64,
    /// Requests per rolling minute (0 = unlimited).
    pub max_rpm: u64,
    /// Automatic retries on a retryable failure before it surfaces to the chat.
    pub max_retries: u32,
    /// Base backoff (ms) before the first retry; each further retry doubles it.
    pub retry_base_ms: u64,
}

/// The concrete rig client behind the brain. Keeping the two providers behind one
/// enum is what lets the rest of the brain — retry loop, usage accounting, prompt
/// framing — stay provider-agnostic: only [`LlmBrain::prompt_once`] ever has to
/// name which one is in play.
enum Provider {
    /// An OpenAI-compatible chat endpoint (OpenAI, OpenRouter, Ollama, …).
    OpenAi(openai::CompletionsClient),
    /// rig's native Gemini provider, selected when the configured base URL targets
    /// Gemini's OpenAI-compat shim (see [`is_gemini_openai_compat`]).
    Gemini(gemini::Client),
}

impl Provider {
    /// Selects and builds the provider for a configured endpoint. A base URL that
    /// targets Gemini's OpenAI-compat shim is redirected to the native Gemini
    /// provider (and logged, so the switch is visible); every other endpoint is
    /// treated as OpenAI-compatible. `api_key` may be empty for a local server
    /// that needs none.
    fn resolve(base_url: &str, api_key: &str) -> Result<Self, String> {
        if is_gemini_openai_compat(base_url) {
            tracing::info!(
                base_url,
                "base URL targets Gemini's OpenAI-compat endpoint; switching to \
                 rig's native Gemini provider so thought signatures round-trip"
            );
            // The native provider uses its own default base URL and API path — the
            // compat URL was only the signal, not an endpoint to forward to.
            let client = gemini::Client::builder()
                .api_key(api_key)
                .build()
                .map_err(|e| format!("failed to build the Gemini client: {e}"))?;
            Ok(Provider::Gemini(client))
        } else {
            let client = openai::CompletionsClient::builder()
                .api_key(api_key)
                .base_url(base_url)
                .build()
                .map_err(|e| format!("failed to build the LLM client: {e}"))?;
            Ok(Provider::OpenAi(client))
        }
    }
}

/// Builds and runs one completion against any rig client, handing rig's raw
/// response back — plus whatever the model chose to remember — for the caller's
/// retry, usage, and persistence handling. Generic over the provider so the
/// OpenAI and Gemini arms share a single body; the two concrete clients differ
/// only in construction, never here. `structured` toggles the `respond` output
/// schema (a decision) versus a plain-text completion (a summary). A decision
/// always registers the `remember` tool and, when `recall` is non-empty, the
/// `recall_memory` tool too. The returned `Vec` is what the model remembered this
/// call (empty for a summary, or a decision that didn't remember). Extended
/// details carry token usage, which feeds the debug/cost panel.
async fn prompt_completion<C>(
    client: &C,
    model: &str,
    preamble: &str,
    max_tokens: u64,
    structured: bool,
    recall: &[String],
    text: String,
) -> Result<(PromptResponse, Vec<String>), PromptError>
where
    C: CompletionClient,
    C::CompletionModel: 'static,
{
    let mut builder = client.agent(model).preamble(preamble).max_tokens(max_tokens);
    if structured {
        builder = builder.output_schema::<RespondArgs>();
    }
    // The summary path is not a decision, so it registers no tools: `OutputMode::
    // Auto` stays native single-request output, no round-trip and nothing to
    // remember.
    if !structured {
        let response = builder.build().prompt(text).extended_details().await?;
        return Ok((response, Vec::new()));
    }
    // A decision always offers `remember` (writing is always available) and adds
    // `recall_memory` when the persona has something to recall. `remember` writes
    // into `sink`, drained after the loop and returned for the orchestrator to
    // persist. The budget is raised for a possible tool round-trip; a turn that
    // calls neither tool still finalizes in one request (rig ends the moment the
    // model calls `final_result`, so the ceiling only bounds actual tool use).
    let sink = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut builder = builder.tool(RememberMemory { sink: Arc::clone(&sink) });
    if !recall.is_empty() {
        builder = builder.tool(RecallMemory { memories: recall.to_vec() });
    }
    let response =
        builder.build().prompt(text).max_turns(MEMORY_MAX_TURNS).extended_details().await?;
    let remembered = std::mem::take(&mut *sink.lock().unwrap());
    Ok((response, remembered))
}

/// Runs one structured-output completion for a suggestion pass: the [`SuggestArgs`]
/// schema and *no tools*, so — like the summary path but with a schema —
/// `OutputMode::Auto` stays a single native request. Generic over the provider so
/// the OpenAI and Gemini arms share one body. Returns rig's raw response for the
/// caller to parse and account for.
async fn prompt_suggestions<C>(
    client: &C,
    model: &str,
    preamble: &str,
    max_tokens: u64,
    text: String,
) -> Result<PromptResponse, PromptError>
where
    C: CompletionClient,
    C::CompletionModel: 'static,
{
    client
        .agent(model)
        .preamble(preamble)
        .max_tokens(max_tokens)
        .output_schema::<SuggestArgs>()
        .build()
        .prompt(text)
        .extended_details()
        .await
}

/// Whether a base URL points at Gemini's OpenAI-compatibility endpoint
/// (`https://generativelanguage.googleapis.com/…/openai/`). Such a URL is the
/// signal to drive rig's *native* Gemini provider instead: the OpenAI wire format
/// has no field for Gemini's `thoughtSignature`, which the native path round-trips.
/// Tolerant of a trailing slash and letter case so a hand-typed value still matches.
fn is_gemini_openai_compat(base_url: &str) -> bool {
    let normalized = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    normalized.contains("generativelanguage.googleapis.com") && normalized.ends_with("/openai")
}

/// An [`AgentBrain`] that drives decisions with a rig-core chat model. Endpoint,
/// model, and key all come from config; the provider (OpenAI-compatible or native
/// Gemini) is chosen from the base URL at construction.
pub struct LlmBrain {
    client: Provider,
    model: String,
    max_tokens: u64,
    /// Output-token ceiling for a summarization call — larger than `max_tokens`
    /// because a running summary is a cumulative digest of a whole multi-persona
    /// history, not a single short reply.
    summary_max_tokens: u64,
    /// Optional server-wide request throttle, so a free-tier per-minute quota
    /// (e.g. Gemini's 15 rpm) isn't exceeded. `None` means unlimited.
    limiter: Option<RateLimiter>,
    /// How many automatic retries a retryable failure gets (on top of the first
    /// attempt) before the error is surfaced to the chat. `0` disables retries.
    max_retries: u32,
    /// Base backoff before the first retry; each further retry doubles it, capped
    /// at [`MAX_BACKOFF`].
    retry_base: Duration,
}

impl LlmBrain {
    /// Builds the brain from an [`LlmConfig`], choosing the provider from the base
    /// URL (see [`Provider::resolve`]). `api_key` may be empty for local servers
    /// that need no auth. Fails only if the client can't be constructed.
    pub fn new(config: LlmConfig<'_>) -> Result<Self, String> {
        let LlmConfig {
            base_url,
            model,
            api_key,
            max_tokens,
            summary_max_tokens,
            max_rpm,
            max_retries,
            retry_base_ms,
        } = config;
        let client = Provider::resolve(base_url, api_key)?;
        let limiter = (max_rpm > 0).then(|| RateLimiter::per_minute(max_rpm as usize));
        Ok(Self {
            client,
            model: model.to_string(),
            max_tokens,
            summary_max_tokens,
            limiter,
            max_retries,
            retry_base: Duration::from_millis(retry_base_ms),
        })
    }

    /// Dispatches one completion to whichever provider backs this brain. Both the
    /// decision retry loop and the summary path go through here, so the choice of
    /// provider is named in exactly one place rather than duplicated per call site.
    /// `recall` is the persona's recallable memory (empty for a summary, or a
    /// decision with nothing to recall), registered as the `recall_memory` tool.
    /// Returns rig's response alongside whatever the model chose to remember (see
    /// [`prompt_completion`]).
    async fn prompt_once(
        &self,
        preamble: &str,
        max_tokens: u64,
        structured: bool,
        recall: &[String],
        text: String,
    ) -> Result<(PromptResponse, Vec<String>), PromptError> {
        match &self.client {
            Provider::OpenAi(c) => {
                prompt_completion(c, &self.model, preamble, max_tokens, structured, recall, text)
                    .await
            }
            Provider::Gemini(c) => {
                prompt_completion(c, &self.model, preamble, max_tokens, structured, recall, text)
                    .await
            }
        }
    }

    /// Dispatches one suggestion completion to whichever provider backs this
    /// brain, mirroring [`Self::prompt_once`] but for the tool-free
    /// [`prompt_suggestions`] path so the provider choice stays named in one place.
    async fn suggest_once(
        &self,
        preamble: &str,
        max_tokens: u64,
        text: String,
    ) -> Result<PromptResponse, PromptError> {
        match &self.client {
            Provider::OpenAi(c) => prompt_suggestions(c, &self.model, preamble, max_tokens, text).await,
            Provider::Gemini(c) => prompt_suggestions(c, &self.model, preamble, max_tokens, text).await,
        }
    }

    /// The wait before the `attempt`-th retry (0-based). A server-supplied
    /// `hint` (e.g. Gemini's `retryDelay`) is honored first, clamped to
    /// [`MAX_BACKOFF`] so a turn can't hang for minutes, with a little jitter to
    /// desynchronize clients. Otherwise it's exponential backoff — `retry_base ·
    /// 2^attempt`, capped — with *equal* jitter: at least half the interval (so a
    /// 5xx isn't hammered too fast), the rest random to avoid a thundering herd.
    fn backoff(&self, attempt: u32, hint: Option<Duration>) -> Duration {
        match hint {
            Some(hint) => {
                let base = hint.min(MAX_BACKOFF);
                base + Duration::from_millis((jitter_unit() * 250.0) as u64)
            }
            None => {
                let cap = self
                    .retry_base
                    .saturating_mul(2u32.saturating_pow(attempt.min(16)))
                    .min(MAX_BACKOFF);
                let half = cap / 2;
                half + half.mul_f64(jitter_unit())
            }
        }
    }
}

#[async_trait]
impl AgentBrain for LlmBrain {
    async fn decide(&self, prompt: &AgentPrompt) -> Decision {
        // The persona (system), with the group members and directory already
        // folded in by the orchestrator, becomes the agent preamble; the clean
        // transcript (plus <time>) is the text to reason over. Built per turn
        // because the preamble is persona- and membership-specific. The remember
        // note is always present (writing is always available); the recall note is
        // appended only when the persona has memories to recall, so the model is
        // never told about a tool it lacks (see `prompt_completion`).
        let mut preamble = format!("{}\n\n{GUIDANCE}\n\n{REMEMBER_GUIDANCE}", prompt.system.trim());
        if !prompt.recallable_memories.is_empty() {
            preamble.push_str("\n\n");
            preamble.push_str(RECALL_GUIDANCE);
        }

        // Retry loop. Two kinds of failure feed it: a transport/provider error
        // (rate limit, overload, network) is retried with exponential backoff; a
        // transport-successful but *unusable* completion — malformed/truncated
        // JSON, or runaway repetition — is retried immediately with a corrective
        // hint appended so the model steers away from repeating the mistake.
        // Either way, once `max_retries` is spent the failure is surfaced to the
        // chat rather than swallowed as a silent read. `attempt == 0` is the first
        // try.
        let mut attempt: u32 = 0;
        // A one-line hint appended to the *next* attempt after unusable output.
        let mut correction: Option<&'static str> = None;
        // Tokens accumulate across attempts: every attempt that reaches the model
        // is billed, so a decision reached after retries reports its whole cost.
        let mut usage_acc = TokenUsage::default();
        loop {
            // Throttle before issuing the request so the server stays under the
            // provider's per-minute quota. Waiting here (rather than after) means a
            // hard interrupt that drops this future never spends a request slot.
            if let Some(limiter) = &self.limiter {
                limiter.acquire().await;
            }

            // A structured-output completion with `RespondArgs` as the schema. The
            // decision always carries the `remember` tool (and `recall_memory` when
            // the persona has memories), so `OutputMode::Auto` resolves to Tool mode
            // and `remember` may return facts to persist; a turn that calls no tool
            // still finalizes in one request (see `prompt_completion`). Any
            // correction from a prior unusable attempt rides along as its own tagged
            // section so the model sees why its last reply was rejected.
            let text = match correction {
                Some(hint) => {
                    format!("{}\n\n<retry_note>{hint}</retry_note>", prompt.conversation)
                }
                None => prompt.conversation.clone(),
            };
            match self
                .prompt_once(&preamble, self.max_tokens, true, &prompt.recallable_memories, text)
                .await
            {
                Ok((response, remembered)) => {
                    usage_acc = add_usage(usage_acc, to_token_usage(response.usage));
                    match validate_decision(&response.output) {
                        Ok(respond) => {
                            // Only the winning attempt's remembers propagate: a
                            // discarded (retried) attempt's are dropped with it, so
                            // a fact isn't stored twice across retries.
                            return Decision {
                                outcome: Outcome::Responded(respond),
                                usage: Some(usage_acc),
                                remembered,
                            };
                        }
                        // The model answered, but the answer can't be used. This is
                        // the model's own output, not a transport failure, so retry
                        // it *with a hint* (no backoff — it isn't server pressure)
                        // and, when retries run out, surface a failure so the turn
                        // stays unread instead of masquerading as a chosen silence.
                        Err(bad) if attempt < self.max_retries => {
                            tracing::warn!(
                                persona = %prompt.persona_name,
                                reason = bad.reason(),
                                output = %preview(&response.output),
                                attempt = attempt + 1,
                                "unusable agent output; retrying with a corrective hint"
                            );
                            correction = Some(bad.hint());
                            attempt += 1;
                            continue;
                        }
                        Err(bad) => {
                            tracing::warn!(
                                persona = %prompt.persona_name,
                                reason = bad.reason(),
                                output = %preview(&response.output),
                                "unusable agent output after retries; surfacing to chat"
                            );
                            return Decision {
                                outcome: Outcome::Failed(BrainError {
                                    status: None,
                                    reason: bad.reason().to_string(),
                                }),
                                usage: Some(usage_acc),
                                remembered: Vec::new(),
                            };
                        }
                    }
                }
                Err(e) => {
                    let (status, reason) = classify_failure(&e);
                    // Retry transient failures with backoff; the sleep is inside
                    // this awaited future, so a hard interrupt still cancels cleanly.
                    if is_retryable(status) && attempt < self.max_retries {
                        // Prefer the provider's own retry hint (e.g. a 429's
                        // `retryDelay`) over our computed backoff.
                        let wait = self.backoff(attempt, retry_hint(&e));
                        tracing::warn!(
                            persona = %prompt.persona_name,
                            error = %e,
                            ?status,
                            attempt = attempt + 1,
                            backoff_ms = wait.as_millis() as u64,
                            "LLM decide failed; retrying after backoff"
                        );
                        tokio::time::sleep(wait).await;
                        attempt += 1;
                        // This attempt never reached the model, so there's no bad
                        // completion to correct — drop any pending content hint.
                        correction = None;
                        continue;
                    }
                    // Out of retries (or a permanent error): surface the sanitized
                    // failure to the chat rather than pretending the agent read.
                    // Report any tokens earlier attempts already spent.
                    tracing::warn!(
                        persona = %prompt.persona_name,
                        error = %e,
                        ?status,
                        "LLM decide failed; surfacing to chat"
                    );
                    let usage = (usage_acc.total_tokens > 0).then_some(usage_acc);
                    return Decision {
                        outcome: Outcome::Failed(BrainError { status, reason }),
                        usage,
                        remembered: Vec::new(),
                    };
                }
            }
        }
    }

    async fn summarize(&self, request: &SummaryRequest) -> Result<Summary, BrainError> {
        use std::fmt::Write as _;

        // The prior summary (framed) plus the batch of older lines to absorb.
        // Same XML-tagged framing the guidance names, so the model can tell the
        // established summary apart from the raw messages being folded in.
        let mut input = String::new();
        if let Some(prior) = request.prior.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            let _ = write!(input, "<summary_so_far>\n{prior}\n</summary_so_far>\n\n");
        }
        input.push_str("<older_messages>\n");
        for line in &request.lines {
            let _ = writeln!(input, "{line}");
        }
        input.push_str("</older_messages>");

        // One attempt, throttled like a decision. Compression is best-effort: if
        // it fails the orchestrator just keeps the full transcript this time and
        // retries at the next boundary, so a retry loop here would only spend more
        // quota for no correctness gain.
        if let Some(limiter) = &self.limiter {
            limiter.acquire().await;
        }
        // A plain-text completion (no structured output/tools): the summary is
        // free-form notes, not a `respond` decision.
        match self.prompt_once(SUMMARY_GUIDANCE, self.summary_max_tokens, false, &[], input).await {
            // A summary registers no tools, so nothing is ever remembered here.
            Ok((response, _)) => Ok(Summary {
                text: response.output.trim().to_string(),
                usage: Some(to_token_usage(response.usage)),
            }),
            Err(e) => {
                let (status, reason) = classify_failure(&e);
                tracing::warn!(error = %e, ?status, "context compression failed");
                Err(BrainError { status, reason })
            }
        }
    }

    async fn suggest(&self, request: &SuggestionRequest) -> Result<Suggestions, BrainError> {
        use std::fmt::Write as _;

        // The context, in the XML-tagged framing the guidance names, so the model
        // can tell the roster, the summary, and the live tail apart. Only the
        // sections that carry something are emitted.
        let mut input = String::new();
        if !request.members.is_empty() {
            input.push_str("<group_members>\n");
            for member in &request.members {
                let marker = if member.is_user { " (the user)" } else { "" };
                match member.blurb.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
                    Some(blurb) => {
                        let _ = writeln!(input, "- {}{marker}: {blurb}", member.name);
                    }
                    None => {
                        let _ = writeln!(input, "- {}{marker}", member.name);
                    }
                }
            }
            input.push_str("</group_members>\n\n");
        }
        if let Some(summary) = request.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let _ = write!(input, "<summary>\n{summary}\n</summary>\n\n");
        }
        if !request.recent.is_empty() {
            input.push_str("<conversation>\n");
            for line in &request.recent {
                let _ = writeln!(input, "{line}");
            }
            input.push_str("</conversation>\n\n");
        }
        if let Some(language) = request.language.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            let _ = write!(input, "<language>{language}</language>\n\n");
        }
        let _ = write!(input, "<time>\n{} ({})\n</time>\n\n", request.now, request.time_of_day);
        let _ = write!(input, "Suggest at least {} messages.", request.min_count.max(1));

        // Throttled like a decision so the suggestion pass counts against the same
        // per-minute quota. Best-effort: a failure surfaces to the caller, which
        // simply keeps whatever was already cached rather than retrying and
        // spending more quota on a non-critical payload.
        if let Some(limiter) = &self.limiter {
            limiter.acquire().await;
        }
        match self.suggest_once(SUGGEST_GUIDANCE, SUGGEST_MAX_TOKENS, input.clone()).await {
            Ok(response) => {
                let usage = Some(to_token_usage(response.usage));
                // The model returns the schema as a JSON string; parse it, then
                // drop blank or runaway-repetitive lines and cap the count.
                let parsed = serde_json::from_str::<SuggestArgs>(response.output.trim())
                    .map_err(|_| BrainError {
                        status: None,
                        reason: "model returned an unparseable suggestion list".to_string(),
                    })?;
                let mut prompts: Vec<String> = Vec::new();
                for line in parsed.suggestions {
                    let line = line.trim();
                    if line.is_empty() || looks_repetitive(line) {
                        continue;
                    }
                    // De-duplicate so a model that repeats itself doesn't fill the
                    // list with the same opener.
                    if !prompts.iter().any(|existing| existing == line) {
                        prompts.push(line.to_string());
                    }
                    if prompts.len() >= SUGGEST_MAX_ITEMS {
                        break;
                    }
                }
                // Report the exact prompt back so the orchestrator can record it
                // in the trace — the debug panel then shows what informed the
                // openers, not just the openers themselves.
                Ok(Suggestions { prompts, usage, system: SUGGEST_GUIDANCE.to_string(), context: input })
            }
            Err(e) => {
                let (status, reason) = classify_failure(&e);
                tracing::warn!(error = %e, ?status, "suggestion generation failed");
                Err(BrainError { status, reason })
            }
        }
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
    fn retryable_covers_transient_failures_only() {
        // Rate limit and server-overload responses are transient.
        for code in [408, 429, 500, 502, 503, 504] {
            assert!(is_retryable(Some(code)), "{code} should be retryable");
        }
        // Client errors are permanent — retrying just burns quota.
        for code in [400, 401, 403, 404, 422] {
            assert!(!is_retryable(Some(code)), "{code} should not be retryable");
        }
        // No status = a transport/timeout error, which is worth a bounded retry.
        assert!(is_retryable(None));
    }

    #[test]
    fn backoff_grows_with_equal_jitter_then_caps() {
        let brain = LlmBrain::new(LlmConfig {
            base_url: "http://x",
            model: "m",
            api_key: "",
            max_tokens: 64,
            summary_max_tokens: 1024,
            max_rpm: 0,
            max_retries: 5,
            retry_base_ms: 1000,
        })
        .unwrap();
        // Equal jitter: the wait lands in [cap/2, cap) for cap = 1s, 2s, 4s …
        // Sample repeatedly so the random half is actually exercised.
        for _ in 0..50 {
            let w = brain.backoff(0, None);
            assert!(w >= Duration::from_millis(500) && w < Duration::from_millis(1000), "{w:?}");
            let w = brain.backoff(1, None);
            assert!(w >= Duration::from_millis(1000) && w < Duration::from_millis(2000), "{w:?}");
            // Never exceeds the hard cap, however large the attempt.
            let w = brain.backoff(20, None);
            assert!(w >= MAX_BACKOFF / 2 && w < MAX_BACKOFF, "{w:?}");
        }
    }

    #[test]
    fn backoff_honors_the_server_hint() {
        let brain = LlmBrain::new(LlmConfig {
            base_url: "http://x",
            model: "m",
            api_key: "",
            max_tokens: 64,
            summary_max_tokens: 1024,
            max_rpm: 0,
            max_retries: 5,
            retry_base_ms: 1000,
        })
        .unwrap();
        // A hint wins over the exponential schedule (plus a little jitter).
        let w = brain.backoff(0, Some(Duration::from_secs(3)));
        assert!(w >= Duration::from_secs(3) && w <= Duration::from_millis(3_250), "{w:?}");
        // Still clamped to the cap so a turn can't hang on a huge hint.
        let w = brain.backoff(0, Some(Duration::from_secs(600)));
        assert!(w >= MAX_BACKOFF && w <= MAX_BACKOFF + Duration::from_millis(250), "{w:?}");
    }

    #[test]
    fn detects_gemini_openai_compat_endpoint() {
        // The exact shim URL, and tolerant of a trailing slash / letter case.
        assert!(is_gemini_openai_compat("https://generativelanguage.googleapis.com/v1beta/openai/"));
        assert!(is_gemini_openai_compat("https://generativelanguage.googleapis.com/v1beta/openai"));
        assert!(is_gemini_openai_compat(
            "  HTTPS://generativelanguage.googleapis.com/v1beta/OpenAI/  "
        ));
        // A plain OpenAI-compatible endpoint (incl. Gemini's *native* base) is not
        // the compat shim, so it stays on the OpenAI-compatible client.
        assert!(!is_gemini_openai_compat("https://api.openai.com/v1"));
        assert!(!is_gemini_openai_compat("http://localhost:11434/v1"));
        assert!(!is_gemini_openai_compat("https://generativelanguage.googleapis.com"));
    }

    #[test]
    fn parses_gemini_style_retry_delay() {
        assert_eq!(parse_duration_secs("48s"), Some(Duration::from_secs(48)));
        assert_eq!(parse_duration_secs(" 1.5s "), Some(Duration::from_secs_f64(1.5)));
        assert_eq!(parse_duration_secs("2"), Some(Duration::from_secs(2)));
        assert_eq!(parse_duration_secs("soon"), None);
        assert_eq!(parse_duration_secs("-1s"), None);
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

    #[test]
    fn flags_runaway_repetition_only() {
        // A looped emoji cluster and a long single-char run are spam.
        assert!(looks_repetitive(&"🏐💥🚀🔥💪✨🚀🎉".repeat(30)));
        assert!(looks_repetitive(&"!".repeat(80)));
        assert!(looks_repetitive(&"哈".repeat(80)));
        // Ordinary emphasis and normal chat stay under the bar.
        assert!(!looks_repetitive("哈哈哈"));
        assert!(!looks_repetitive("好的！！！我知道了"));
        assert!(!looks_repetitive(
            "這是一句正常、稍微長一點點的聊天訊息，內容沒有任何連續重複的片段。"
        ));
    }

    #[test]
    fn validate_rejects_unusable_output_but_keeps_valid() {
        // Malformed / truncated JSON → a retryable content error.
        assert!(matches!(
            validate_decision(r#"{"action": "speak", "message": "hi"#),
            Err(BadOutput::Malformed)
        ));
        // Valid JSON whose spoken line is spam → a retryable content error.
        let spam = format!(r#"{{"action":"speak","message":"{}"}}"#, "🔥🚀".repeat(50));
        assert!(matches!(validate_decision(&spam), Err(BadOutput::Repetitive)));
        // A clean decision parses through.
        let ok = validate_decision(r#"{"action":"speak","message":"hello"}"#).unwrap();
        assert_eq!(ok.action, Action::Speak);
        // A valid-but-underspecified decision still degrades to a legit read,
        // *not* an error — the model genuinely chose to say nothing usable.
        let read = validate_decision(r#"{"action":"speak"}"#).unwrap();
        assert_eq!(read.action, Action::Read);
    }

    #[tokio::test]
    async fn recall_tool_filters_by_query_or_returns_all() {
        let tool = RecallMemory {
            memories: vec![
                "Prefers tea over coffee".into(),
                "Has a cat named Mochi".into(),
                "Dislikes loud rooms".into(),
            ],
        };

        // No query (and a blank one) returns everything the persona remembers.
        assert_eq!(tool.call(RecallArgs { query: None }).await.unwrap().len(), 3);
        assert_eq!(tool.call(RecallArgs { query: Some("  ".into()) }).await.unwrap().len(), 3);

        // A query narrows to matching memories, case-insensitively.
        let cat = tool.call(RecallArgs { query: Some("CAT".into()) }).await.unwrap();
        assert_eq!(cat, ["Has a cat named Mochi"]);

        // No match yields an empty list, not an error.
        assert!(tool.call(RecallArgs { query: Some("weather".into()) }).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn remember_tool_collects_trimmed_deduped_content() {
        let sink = Arc::new(Mutex::new(Vec::<String>::new()));
        let tool = RememberMemory { sink: Arc::clone(&sink) };

        // A fact is saved (trimmed); a blank one is ignored; a duplicate is dropped.
        tool.call(RememberArgs { content: "  prefers tea over coffee  ".into() }).await.unwrap();
        tool.call(RememberArgs { content: "   ".into() }).await.unwrap();
        tool.call(RememberArgs { content: "prefers tea over coffee".into() }).await.unwrap();
        tool.call(RememberArgs { content: "has a cat named Mochi".into() }).await.unwrap();

        let saved = sink.lock().unwrap().clone();
        assert_eq!(saved, ["prefers tea over coffee", "has a cat named Mochi"]);
    }

    #[test]
    fn add_usage_sums_each_field() {
        let a = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            cached_prompt_tokens: 2,
        };
        let b = TokenUsage {
            prompt_tokens: 3,
            completion_tokens: 7,
            total_tokens: 10,
            cached_prompt_tokens: 1,
        };
        let sum = add_usage(a, b);
        assert_eq!(sum.prompt_tokens, 13);
        assert_eq!(sum.completion_tokens, 12);
        assert_eq!(sum.total_tokens, 25);
        assert_eq!(sum.cached_prompt_tokens, 3);
    }
}
