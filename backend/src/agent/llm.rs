//! The LLM-backed [`AgentBrain`] — rig-core structured output over an
//! OpenAI-compatible chat endpoint.
//!
//! This is the real implementation of the single inference seam. The
//! orchestrator still owns all context work; this brain just turns an
//! [`AgentPrompt`] into a structured `respond` decision by asking a model.
//! Everything about the endpoint is configured (base URL, model, key) — nothing
//! is hard-coded to a provider, so it drives OpenAI, OpenRouter, or a local
//! Ollama / llama.cpp server equally.
//!
//! The decision is one structured-output completion: no callable tools, so
//! rig resolves `OutputMode::Auto` to native structured output — a single
//! request, no function-call round-trip. Member lookups the agent might have
//! needed a tool for are handed to it up front in the prompt's `<directory>`
//! section instead. A live tool loop needs a follow-up turn, which providers
//! like Gemini reject (their function calls carry a `thought_signature` that rig
//! 0.40 can't round-trip); staying single-shot keeps every decision succeed-or-
//! genuinely-fail, so a silent "read" is always the model's own choice rather
//! than a swallowed tool-call error.

use std::time::Duration;

use async_trait::async_trait;
use rig_core::client::completion::CompletionClient;
use rig_core::completion::{Prompt, Usage};
use rig_core::providers::openai;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::brain::{AgentBrain, AgentPrompt, BrainError, Decision, Outcome, Respond};
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

/// How the agent is told to use the `respond` schema and the context it has.
/// Appended *after* the persona's own system prompt, so character comes first
/// and mechanics second.
const GUIDANCE: &str = "\
You are one participant in a group text chat. The context above is given in \
XML-tagged sections: <persona> and <context> are you; <group_members> lists who \
is in this room; <directory> lists other members of the workspace you may refer \
to by their exact, globally-unique name (with their short bio) even though they \
are not here. The message below carries the live <conversation>, then any \
<environment> events, and ends with the current <time> (with timezone). Inside \
<conversation>, each line is a <message from=\"NAME\" time=\"TIMESTAMP\">…</message> \
element: `from` is the speaker and `time` is when they sent it (same timezone as \
<time>), so you can judge what is recent and what is stale. Decide your single next \
action using the response schema. Speak only when you have something worth \
adding; otherwise choose `read` to stay silent. Moods are UI-only flavour and \
are never shown to other participants as text. Keep any reply to one short chat \
message.";

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

/// An [`AgentBrain`] that drives decisions with an OpenAI-compatible chat model
/// via rig-core. Endpoint, model, and key all come from config.
pub struct LlmBrain {
    client: openai::CompletionsClient,
    model: String,
    max_tokens: u64,
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
    /// Builds the client against an OpenAI-compatible endpoint. `api_key` may be
    /// empty for local servers that need no auth. `max_rpm` caps outgoing
    /// requests per rolling minute (0 = unlimited). `max_retries` and
    /// `retry_base_ms` tune the automatic backoff on a retryable failure. Fails
    /// only if the client can't be constructed.
    pub fn new(
        base_url: &str,
        model: impl Into<String>,
        api_key: &str,
        max_tokens: u64,
        max_rpm: u64,
        max_retries: u32,
        retry_base_ms: u64,
    ) -> Result<Self, String> {
        let client = openai::CompletionsClient::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|e| format!("failed to build the LLM client: {e}"))?;
        let limiter = (max_rpm > 0).then(|| RateLimiter::per_minute(max_rpm as usize));
        Ok(Self {
            client,
            model: model.into(),
            max_tokens,
            limiter,
            max_retries,
            retry_base: Duration::from_millis(retry_base_ms),
        })
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
        // because the preamble is persona- and membership-specific.
        let preamble = format!("{}\n\n{GUIDANCE}", prompt.system.trim());

        // `RespondArgs` as the structured output schema, with no tools — so rig
        // resolves `OutputMode::Auto` to native structured output: one request,
        // no function-call round-trip. That keeps the decision a single
        // succeed-or-fail completion across providers (including Gemini, which
        // rejects a tool loop's follow-up turn over `thought_signature`).
        let agent = self
            .client
            .agent(self.model.as_str())
            .preamble(&preamble)
            .max_tokens(self.max_tokens)
            .output_schema::<RespondArgs>()
            .build();

        // Retry loop: a retryable failure (rate limit, server overload, transport
        // hiccup) is retried with exponential backoff up to `max_retries`; a
        // permanent one is surfaced immediately. `attempt` counts retries taken so
        // far, so `attempt == 0` is the first try.
        let mut attempt: u32 = 0;
        loop {
            // Throttle before issuing the request so the server stays under the
            // provider's per-minute quota. Waiting here (rather than after) means a
            // hard interrupt that drops this future never spends a request slot.
            if let Some(limiter) = &self.limiter {
                limiter.acquire().await;
            }

            // Extended details carry the token usage, which feeds the debug/cost panel.
            match agent.prompt(prompt.conversation.clone()).extended_details().await {
                Ok(response) => {
                    let respond = serde_json::from_str::<RespondArgs>(response.output.trim())
                        .map(to_respond)
                        .unwrap_or_else(|e| {
                            // The loop finished but the final text wasn't the expected
                            // structured decision. The model *did* answer, so this is
                            // its own (malformed) completion, not a transport failure —
                            // stay silent rather than guess, and don't retry.
                            tracing::warn!(
                                persona = %prompt.persona_name,
                                error = %e,
                                output = %response.output,
                                "could not parse the agent's structured decision; treating as read"
                            );
                            Respond::read()
                        });
                    return Decision {
                        outcome: Outcome::Responded(respond),
                        usage: Some(to_token_usage(response.usage)),
                    };
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
                        continue;
                    }
                    // Out of retries (or a permanent error): surface the sanitized
                    // failure to the chat rather than pretending the agent read.
                    tracing::warn!(
                        persona = %prompt.persona_name,
                        error = %e,
                        ?status,
                        "LLM decide failed; surfacing to chat"
                    );
                    return Decision { outcome: Outcome::Failed(BrainError { status, reason }), usage: None };
                }
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
        let brain = LlmBrain::new("http://x", "m", "", 64, 0, 5, 1000).unwrap();
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
        let brain = LlmBrain::new("http://x", "m", "", 64, 0, 5, 1000).unwrap();
        // A hint wins over the exponential schedule (plus a little jitter).
        let w = brain.backoff(0, Some(Duration::from_secs(3)));
        assert!(w >= Duration::from_secs(3) && w <= Duration::from_millis(3_250), "{w:?}");
        // Still clamped to the cap so a turn can't hang on a huge hint.
        let w = brain.backoff(0, Some(Duration::from_secs(600)));
        assert!(w >= MAX_BACKOFF && w <= MAX_BACKOFF + Duration::from_millis(250), "{w:?}");
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
}
