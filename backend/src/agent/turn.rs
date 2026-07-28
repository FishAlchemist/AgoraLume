//! The multi-agent turn orchestrator — the Global Loop, the per-agent Local
//! Loop, and interrupt handling, all in production code around the swappable
//! [`AgentBrain`].
//!
//! A user message (or environment event) reaches a group's coordinator, which
//! runs a *turn*: up to `max_rounds` sweeps of the group's AI members in random
//! order, each member deciding once — via a single `respond` tool call — on the
//! freshly-updated context. Speaking, showing a mood, and staying silent route
//! to different streams (§ `agent_loop_arch.md`). Soft events fold in at member
//! boundaries; a hard event preempts the turn, discarding the in-flight agent.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike as _;
use tokio::sync::mpsc;

use crate::agent::brain::{
    Action, AgentBrain, AgentPersona, AgentPrompt, BrainError, Decision, MemberInfo, Outcome,
    Respond, SuggestionRequest, SummaryRequest,
};
use crate::agent::event::{Event, Salience, appraise};
use crate::agent::mock::RuleBrain;
use crate::models::{AgentTrace, GroupSuggestions, Message, now_ms};
use crate::state::{AppState, GroupSummary};
use crate::workspace::RosterMember;

/// Tunables for the loop. The bounded compute per triggering message — the "not
/// stuck, not runaway, not wasteful" guarantee — lives in `max_rounds`.
#[derive(Clone)]
pub struct LoopConfig {
    /// Hard cap on sweeps of the member list per triggering message. A round
    /// only leads to another if it produced speech; a silent round ends the
    /// turn early. Default 1.
    pub max_rounds: usize,
    /// Cosmetic delay between a mood and its message, for natural pacing; set to
    /// 0 in tests.
    pub pace_ms: u64,
    /// Shuffle seed. `None` seeds from the clock (production); `Some` makes the
    /// member order deterministic (tests).
    pub seed: Option<u64>,
    /// Context-compression high-water mark: once a group's un-summarized
    /// conversation tail passes this many lines, the oldest are folded into its
    /// running summary at the next turn boundary. `0` disables compression. Only
    /// acts on an LLM runtime (`mock` false) — the mock has no model to summarize
    /// with. See [`crate::config::Config::compress_after`].
    pub compress_after: usize,
    /// How many of the most recent conversation lines stay verbatim when
    /// compressing — the "don't forget what just happened" window. Kept below
    /// `compress_after`. See [`crate::config::Config::compress_keep`].
    pub compress_keep: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self { max_rounds: 1, pace_ms: 450, seed: None, compress_after: 50, compress_keep: 20 }
    }
}

/// The agent runtime bundled into [`AppState`]: the swappable brain and the loop
/// tunables. Swap `brain` for an LLM-backed implementation and the same
/// orchestrator drives a real model — nothing else changes.
pub struct AgentRuntime {
    pub brain: Arc<dyn AgentBrain>,
    pub config: LoopConfig,
    /// True when this is the mock data layer (rule brain) rather than a real
    /// LLM-backed runtime. Surfaced through `/meta`.
    pub mock: bool,
}

impl AgentRuntime {
    /// The default runtime: rule-based replies — no LLM, no persistence. The
    /// "mock" data layer behind the production API.
    pub fn mock() -> Self {
        Self { brain: Arc::new(RuleBrain::new()), config: LoopConfig::default(), mock: true }
    }

    /// A runtime driven by a real LLM brain. `mock` is false, so `/meta` reports
    /// the server is talking to a model.
    pub fn llm(brain: Arc<dyn AgentBrain>) -> Self {
        Self { brain, config: LoopConfig::default(), mock: false }
    }
}

/// How a turn ended.
enum TurnOutcome {
    /// The turn ran to completion (or the group emptied / channel closed).
    Done,
    /// A hard event preempted the turn; the coordinator restarts with it.
    Preempted(Event),
    /// An agent's inference failed; the turn stops at that agent and its trigger
    /// is held so a manual retry can resume the agents that haven't yet read it.
    Suspended(Event),
}

/// A group's coordinator task: it owns the command channel and runs turns one at
/// a time, so there is never more than one turn in flight per group. A hard
/// interrupt returns here and immediately re-runs with the preempting event.
pub async fn coordinator_loop(state: Arc<AppState>, group_id: String, mut rx: mpsc::Receiver<Event>) {
    // The trigger of a turn suspended by a failed inference, held while the loop
    // is idle so a manual retry can resume it. Any fresh trigger clears it, which
    // is exactly the "once you send a message you can't retry" rule.
    let mut pending: Option<Event> = None;
    while let Some(event) = rx.recv().await {
        // Resolve the incoming event into the turn to run: a retry resumes the
        // pending trigger (or is dropped if there is none); anything else is a
        // fresh trigger that voids any pending retry.
        let (mut trigger, mut resuming) = match event {
            Event::Retry => match pending.take() {
                Some(t) => (t, true),
                None => continue,
            },
            other => {
                pending = None;
                (other, false)
            }
        };
        // The loop is busy for the whole turn (including any hard-interrupt
        // restarts); it goes idle only once fully done. The composer gates on
        // this so a new user message can't interleave with a running turn.
        state.set_active(&group_id, true);
        let completed = loop {
            match run_turn(&state, &group_id, trigger, resuming, &mut rx).await {
                TurnOutcome::Done => break true,
                TurnOutcome::Preempted(next) => {
                    trigger = next;
                    resuming = false;
                }
                TurnOutcome::Suspended(t) => {
                    pending = Some(t);
                    break false;
                }
            }
        };
        state.set_active(&group_id, false);
        // Compress the older history once the turn has fully settled — not after a
        // suspend, whose failing provider would only fail the summary too. Running
        // here (idle, off the reply path) keeps the summarizing call's latency out
        // of the user's wait, and a message that arrives meanwhile simply queues
        // and starts its turn on the freshly compacted context.
        if completed {
            maybe_compress(&state, &group_id).await;
        }
    }
}

/// Folds a group's oldest conversation lines into its running summary when the
/// un-summarized tail has grown past the high-water mark, keeping only the most
/// recent `compress_keep` lines verbatim. A no-op on the mock runtime, when
/// compression is disabled (`compress_after == 0`), or when the tail is still
/// short. Best-effort: a failed summarization is logged and the full transcript
/// is kept, to be retried at the next boundary.
async fn maybe_compress(state: &Arc<AppState>, group_id: &str) {
    let runtime = &state.runtime;
    let cfg = &runtime.config;
    if runtime.mock || cfg.compress_after == 0 {
        return;
    }

    let summary = state.summary(group_id);
    let transcript = build_transcript(state, group_id);
    let tail = tail_after(&transcript, summary.through_id.as_deref());
    // Only compress once the live tail is over the high-water mark; below it, the
    // whole tail is cheap enough to send verbatim.
    if tail.len() <= cfg.compress_after {
        return;
    }
    // Fold everything except the most recent `compress_keep` lines.
    let fold_count = tail.len().saturating_sub(cfg.compress_keep);
    if fold_count == 0 {
        return;
    }
    let to_fold = &tail[..fold_count];
    let through_id = to_fold.last().map(|line| line.id.clone());
    let lines: Vec<String> =
        to_fold.iter().map(|line| format!("{}: {}", line.name, line.text)).collect();
    let folded = lines.join("\n");

    let request = SummaryRequest {
        prior: (!summary.text.trim().is_empty()).then(|| summary.text.clone()),
        lines,
    };
    match runtime.brain.summarize(&request).await {
        Ok(result) => {
            let text = result.text.trim().to_string();
            // A blank result would silently drop the folded lines — skip rather
            // than advance `through_id` past history the summary doesn't cover.
            if text.is_empty() {
                tracing::warn!(group = %group_id, "compression returned an empty summary; keeping the full transcript");
                return;
            }
            state.set_summary(group_id, GroupSummary { text: text.clone(), through_id });
            // Record the summarizing call so its tokens count toward the usage /
            // cost panel, exactly like a decision. It is not an agent's turn, so it
            // rides under a synthetic "system" persona with a `summarize` action.
            state.record_trace(
                group_id,
                AgentTrace {
                    ts: now_ms(),
                    group_id: group_id.to_string(),
                    persona_id: "system".to_string(),
                    persona_name: "Context summary".to_string(),
                    system: String::new(),
                    conversation: folded,
                    action: "summarize".to_string(),
                    message: Some(text),
                    mood: None,
                    usage: result.usage,
                },
            );
            tracing::debug!(
                group = %group_id,
                folded = fold_count,
                kept = cfg.compress_keep,
                "compressed older conversation into the running summary"
            );
        }
        Err(error) => {
            tracing::warn!(
                group = %group_id,
                status = ?error.status,
                reason = %error.reason,
                "context compression failed; keeping the full transcript this round"
            );
        }
    }
}

/// Runs one turn for a group. `rx` is watched throughout so events arriving
/// mid-turn are appraised: hard ones preempt (returning `Preempted`), soft ones
/// are folded into the context at the next member boundary.
///
/// When `resuming` is true this is a manual retry of a suspended turn: each
/// round's members are filtered to those that have *not* yet read the trigger
/// message, so agents that already responded aren't re-run. A fresh turn
/// (`resuming` false) runs every member, leaving multi-round behavior intact.
async fn run_turn(
    state: &AppState,
    group_id: &str,
    trigger: Event,
    resuming: bool,
    rx: &mut mpsc::Receiver<Event>,
) -> TurnOutcome {
    let runtime = &state.runtime;
    let cfg = &runtime.config;

    // The user's line every agent marks as read this turn (unlocks the composer).
    let read_target = match &trigger {
        Event::User { message_id, .. } => Some(message_id.clone()),
        Event::Environment { .. } | Event::Retry => None,
    };

    // Events already folded into the context all agents can see. An environment
    // trigger seeds it; soft events drained at boundaries append to it.
    let mut injected_events: Vec<String> = Vec::new();
    if let Some(text) = trigger.as_context() {
        injected_events.push(text.to_string());
    }

    let mut rng = Rng::new(cfg.seed.unwrap_or_else(time_seed));

    // The compressed older history, fetched once: it can only change at a turn
    // boundary (via `maybe_compress`, on this same coordinator task), so it is
    // stable for the whole turn even as agents append fresh lines.
    let summary = state.summary(group_id);

    for _round in 0..cfg.max_rounds.max(1) {
        // Phase 2: fresh membership, reshuffled each round.
        let Some((_self_id, member_ids)) = state.workspace().turn_members(group_id) else {
            return TurnOutcome::Done;
        };
        let mut order = member_ids;
        // On a resume, re-run only the agents that have not yet read the pending
        // message — those who already responded (or read silently) are skipped,
        // so a retry picks up exactly where the failure left off.
        if resuming
            && let Some(target) = &read_target
        {
            order.retain(|persona_id| !state.has_read(group_id, target, persona_id));
        }
        if order.is_empty() {
            return TurnOutcome::Done;
        }
        shuffle(&mut order, &mut rng);

        // The roster (who's in the room, incl. the user) is fixed for the round;
        // every agent this sweep is told the same membership. The directory (every
        // persona in the workspace) is injected so an agent can refer to people
        // outside the room by their globally-unique name.
        let roster = state.workspace().group_roster(group_id).unwrap_or_default();
        let directory = build_directory(state);
        // A single wall-clock reading for the round, with the server's timezone
        // offset, injected so agents can reason about "now".
        let now = local_now();

        let mut spoke_this_round = false;

        // Phase 3: serial pipeline — each agent decides on the updated context.
        for persona_id in order {
            let Some(persona) = build_persona(state, &persona_id) else {
                continue;
            };
            // The persona's in-character memory, resolved fresh: only what its
            // current identity wrote. Handed to the brain as pull-tool data, not
            // folded into the prompt text (see `AgentPrompt::recallable_memories`).
            let recallable = recall_lines(state, &persona_id);
            let transcript = build_transcript(state, group_id);
            // Only the lines past the summary go in verbatim; the summary stands
            // in for everything before them.
            let tail = tail_after(&transcript, summary.through_id.as_deref());
            let summary_text =
                (!summary.text.trim().is_empty()).then_some(summary.text.as_str());

            // Assemble the prompt — the orchestrator owns context, so a brain is
            // just prompt-in/decision-out (the LLM boundary). The persona's
            // recallable memory rides alongside the text as pull-tool data.
            let mut prompt = assemble_prompt(
                &persona,
                &roster,
                &directory,
                summary_text,
                tail,
                &injected_events,
                &now,
            );
            prompt.recallable_memories = recallable;
            tracing::trace!(
                target: "agent::prompt",
                persona = %persona_id,
                system = %prompt.system,
                conversation = %prompt.conversation,
                "assembled agent prompt",
            );

            // Soft events that arrive while this agent is thinking, held back to
            // inject at the boundary (they must not cut the current agent).
            let mut pending_soft: Vec<String> = Vec::new();

            // The single inference, watched for interrupts. `&mut fut` is only
            // dropped on a hard interrupt (discard); a soft event loops back and
            // re-polls it, so the in-flight agent keeps running.
            let decision = {
                let mut fut = std::pin::pin!(runtime.brain.decide(&prompt));
                loop {
                    tokio::select! {
                        biased;
                        maybe = rx.recv() => {
                            let Some(ev) = maybe else {
                                return TurnOutcome::Done; // channel closed: shutting down
                            };
                            match appraise(&ev) {
                                Salience::Hard => return TurnOutcome::Preempted(ev),
                                Salience::Soft => {
                                    if let Some(text) = ev.as_context() {
                                        pending_soft.push(text.to_string());
                                    }
                                }
                                Salience::Ignore => {}
                            }
                        }
                        decided = &mut fut => break decided,
                    }
                }
            };

            let Decision { outcome, usage, remembered } = decision;

            // A failed inference is not a silent read: record it, surface a
            // sanitized notice to the chat, and suspend the turn at this agent
            // (leaving it *unread*) so a manual retry resumes from here.
            let respond = match outcome {
                Outcome::Responded(respond) => respond,
                Outcome::Failed(error) => {
                    state.record_trace(
                        group_id,
                        AgentTrace {
                            ts: now_ms(),
                            group_id: group_id.to_string(),
                            persona_id: persona_id.clone(),
                            persona_name: persona.name.clone(),
                            system: prompt.system.clone(),
                            conversation: prompt.conversation.clone(),
                            action: "error".to_string(),
                            message: Some(error.reason.clone()),
                            mood: None,
                            usage,
                        },
                    );
                    emit_error(state, group_id, &persona_id, error);
                    return TurnOutcome::Suspended(trigger);
                }
            };
            let Respond { action, message, mood } = respond;

            // Record exactly what this agent saw and decided (plus token cost),
            // for the debug panel. Cloned because the routing below consumes the
            // message/mood.
            state.record_trace(
                group_id,
                AgentTrace {
                    ts: now_ms(),
                    group_id: group_id.to_string(),
                    persona_id: persona_id.clone(),
                    persona_name: persona.name.clone(),
                    system: prompt.system.clone(),
                    conversation: prompt.conversation.clone(),
                    action: action_label(action).to_string(),
                    message: message.clone(),
                    mood: mood.clone(),
                    usage,
                },
            );

            // Persist anything the agent chose to remember this turn, stamped with
            // its current identity so a future turn can recall it (see
            // `Workspace::add_memory`). The tool already ran inside the brain; this
            // just commits its intent to the SSOT. A no-op when nothing was
            // remembered — which is every turn the model doesn't use the tool.
            if !remembered.is_empty() {
                {
                    let mut workspace = state.workspace();
                    for content in &remembered {
                        workspace.add_memory(&persona_id, content);
                    }
                }
                state.persist_workspace();
            }

            // Phase 4: route the decision to the two streams.
            match action {
                Action::Speak => {
                    if let Some(text) = message {
                        emit_message(state, group_id, &persona_id, text);
                        spoke_this_round = true;
                    }
                }
                Action::SpeakWithMood => {
                    if let Some(m) = mood {
                        emit_mood(state, group_id, &persona_id, m);
                    }
                    pace(cfg).await;
                    if let Some(text) = message {
                        emit_message(state, group_id, &persona_id, text);
                        spoke_this_round = true;
                    }
                }
                Action::Mood => {
                    if let Some(m) = mood {
                        emit_mood(state, group_id, &persona_id, m);
                    }
                }
                Action::Read => {}
            }

            // Every agent processed the message, whatever it chose to do.
            if let Some(message_id) = &read_target {
                state.mark_read(group_id, message_id, &persona_id);
            }

            // Handover: fold any soft events into the context for the next agent.
            injected_events.append(&mut pending_soft);
        }

        // Silence ends the turn; otherwise loop only while under the round cap.
        if !spoke_this_round {
            break;
        }
    }

    TurnOutcome::Done
}

/// The wire label for an action, as the debug trace reports it (matching the
/// TypeScript action names).
fn action_label(action: Action) -> &'static str {
    match action {
        Action::Speak => "speak",
        Action::SpeakWithMood => "speakWithMood",
        Action::Mood => "mood",
        Action::Read => "read",
    }
}

/// Emits a spoken line to the group (Context Stream + UI View).
fn emit_message(state: &AppState, group_id: &str, persona_id: &str, text: String) {
    state.emit(group_id, Message::conversation(group_id, persona_id, text, None));
}

/// Emits a mood to the group (UI View only — moods never enter the Context).
fn emit_mood(state: &AppState, group_id: &str, persona_id: &str, mood: String) {
    state.emit(group_id, Message::mood(group_id, persona_id, mood, None));
}

/// Emits a system error notice to the group after an agent's inference failed —
/// the sanitized status + reason only, never the provider body. Like a mood, it
/// is UI-only and never enters the Context other agents read.
fn emit_error(state: &AppState, group_id: &str, persona_id: &str, error: BrainError) {
    state.emit(group_id, Message::system(group_id, persona_id, error.status, error.reason));
}

/// Cosmetic pacing between a mood and its message.
async fn pace(cfg: &LoopConfig) {
    if cfg.pace_ms > 0 {
        tokio::time::sleep(Duration::from_millis(cfg.pace_ms)).await;
    }
}

/// One line of the Context Stream — the filtered transcript agents read. Only
/// spoken lines appear; moods, read receipts and private notes never do. `ts` is
/// the send time (epoch millis) so the assembled prompt can stamp each message;
/// `id` is the message id, so compression can mark how far the summary reaches.
struct ContextLine {
    id: String,
    name: String,
    text: String,
    ts: i64,
}

/// Builds the Context Stream: the group's log filtered to spoken lines only.
/// Moods and read receipts are excluded, so agents reason over clean context.
fn build_transcript(state: &AppState, group_id: &str) -> Vec<ContextLine> {
    let messages = state.list(group_id);
    let workspace = state.workspace();
    messages
        .into_iter()
        .filter_map(|message| match message {
            Message::Conversation { id, persona_id, text, ts, .. } => {
                let name =
                    workspace.persona(&persona_id).map_or(persona_id, |p| p.name);
                Some(ContextLine { id, name, text, ts })
            }
            // Moods and system error notices are UI-only; agents reason over
            // spoken lines alone.
            Message::Mood { .. } | Message::System { .. } => None,
        })
        .collect()
}

/// The transcript lines *after* the one the summary reaches through — the live
/// tail sent verbatim. With no `through_id` (nothing compressed yet) the whole
/// transcript is the tail. If the boundary message can't be found (e.g. a log
/// edited out from under a stale summary), the whole transcript is returned
/// rather than risk hiding recent lines.
fn tail_after<'a>(transcript: &'a [ContextLine], through_id: Option<&str>) -> &'a [ContextLine] {
    match through_id {
        Some(id) => match transcript.iter().position(|line| line.id == id) {
            Some(pos) => &transcript[pos + 1..],
            None => transcript,
        },
        None => transcript,
    }
}

/// Resolves a persona into the identity + variables a brain needs to decide.
fn build_persona(state: &AppState, id: &str) -> Option<AgentPersona> {
    let workspace = state.workspace();
    let persona = workspace.persona(id)?;
    let variables = workspace.resolve_variables(&persona);
    Some(AgentPersona {
        name: persona.name,
        system_prompt: persona.system_prompt.unwrap_or_default(),
        variables,
    })
}

/// A persona's recallable memory contents (current identity only), newest first —
/// the data an LLM brain exposes as a pull tool. Empty for a persona with no
/// prompt or no memories under its current hash, in which case the brain attaches
/// no tool and the turn stays a single completion.
fn recall_lines(state: &AppState, persona_id: &str) -> Vec<String> {
    state.workspace().recallable_memories(persona_id).into_iter().map(|m| m.content).collect()
}

/// Every persona in the workspace, as the member directory injected into each
/// prompt. Cloned so the brain can hold it past the workspace lock.
fn build_directory(state: &AppState) -> Vec<MemberInfo> {
    state
        .workspace()
        .personas
        .iter()
        .map(|p| MemberInfo {
            name: p.name.clone(),
            blurb: p.blurb.clone(),
            is_user: p.kind == crate::models::PersonaKind::User,
        })
        .collect()
}

/// The current local time as an RFC 3339 string carrying the server's timezone
/// offset (e.g. `2026-07-25T15:30:00+08:00`), so agents can reason about "now".
fn local_now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// How many suggestions the generator aims for — the user's "at least 3".
pub const SUGGEST_MIN: usize = 3;

/// How many of the most recent transcript lines to hand the suggestion generator
/// as context — enough to follow the thread without resending the whole history.
const SUGGEST_CONTEXT_LINES: usize = 12;

/// Generates conversation-starter suggestions for a group and stores them.
/// Spawned off the request path by [`AppState::request_suggestions`] so the HTTP
/// GET never blocks on the model; the result is persisted and pushed on the
/// group's `suggestions` SSE frame. On failure — or an empty result — the
/// previous suggestions are kept and only the in-flight gate is cleared.
/// Assembles the same context a decision sees, plus the current local time and
/// part of day so the openers fit the moment.
pub async fn generate_suggestions(state: Arc<AppState>, group_id: String) {
    let runtime = &state.runtime;

    // The room's members (name + blurb + who's the user), for addressing.
    let members: Vec<MemberInfo> = state
        .workspace()
        .group_roster(&group_id)
        .unwrap_or_default()
        .into_iter()
        .map(|m| MemberInfo { name: m.name, blurb: m.blurb, is_user: m.is_self })
        .collect();
    // The user's own language, so openers are written in something they can send.
    let language = {
        let ws = state.workspace();
        let lang = ws.settings.native_language.trim().to_string();
        (!lang.is_empty()).then_some(lang)
    };

    // The running summary + recent tail, exactly as a turn would build them.
    let summary = state.summary(&group_id);
    let transcript = build_transcript(&state, &group_id);
    let tail = tail_after(&transcript, summary.through_id.as_deref());
    let recent: Vec<String> = tail
        .iter()
        .rev()
        .take(SUGGEST_CONTEXT_LINES)
        .rev()
        .map(|line| format!("{}: {}", line.name, line.text))
        .collect();
    // The staleness key: the last conversation line these suggestions follow.
    let through_id = transcript.last().map(|line| line.id.clone());

    let now = local_now();
    let time_of_day = current_time_of_day();

    let request = SuggestionRequest {
        members,
        summary: (!summary.text.trim().is_empty()).then(|| summary.text.clone()),
        recent,
        now,
        time_of_day: time_of_day.to_string(),
        language,
        min_count: SUGGEST_MIN,
    };

    match runtime.brain.suggest(&request).await {
        Ok(result) => {
            if result.prompts.is_empty() {
                // Nothing usable — keep whatever was cached, just release the gate.
                state.finish_suggestions(&group_id, None);
                return;
            }
            // A suggestion pass that actually called a model is a real request:
            // record it so its tokens land in the usage/cost panel, under the same
            // synthetic "system" persona a compression uses. The mock reports no
            // usage, so it is never counted as an inference.
            if let Some(usage) = result.usage {
                state.record_trace(
                    &group_id,
                    AgentTrace {
                        ts: now_ms(),
                        group_id: group_id.clone(),
                        persona_id: "system".to_string(),
                        persona_name: "Chat suggestions".to_string(),
                        // The exact prompt the pass ran on, so the debug panel shows
                        // what informed the openers beyond the standing instruction.
                        system: result.system,
                        conversation: result.context,
                        action: "suggest".to_string(),
                        message: Some(result.prompts.join("\n")),
                        mood: None,
                        usage: Some(usage),
                    },
                );
            }
            let suggestions = GroupSuggestions {
                prompts: result.prompts,
                generated_at: now_ms(),
                time_of_day: time_of_day.to_string(),
                through_id,
            };
            state.finish_suggestions(&group_id, Some(suggestions));
        }
        Err(error) => {
            tracing::warn!(
                group = %group_id,
                status = ?error.status,
                reason = %error.reason,
                "suggestion generation failed; keeping the previous suggestions"
            );
            state.finish_suggestions(&group_id, None);
        }
    }
}

/// The current part of day, from the server's local clock — the bucket the
/// suggestion generator tunes openers to, and the staleness key that forces a
/// regeneration when the day moves on (e.g. morning → evening).
pub fn current_time_of_day() -> &'static str {
    time_of_day(chrono::Local::now().hour())
}

/// Buckets a 24-hour clock hour into a coarse part of day.
fn time_of_day(hour: u32) -> &'static str {
    match hour {
        5..=10 => "morning",
        11..=16 => "afternoon",
        17..=21 => "evening",
        _ => "night",
    }
}

/// Formats an epoch-millisecond send time in the same local RFC 3339 form as
/// [`local_now`], so a message's `time` attribute compares directly against the
/// `<time>` "now". Falls back to the raw millis if the value is out of range.
fn format_ts(ts: i64) -> String {
    use chrono::TimeZone as _;
    match chrono::Local.timestamp_millis_opt(ts) {
        chrono::LocalResult::Single(dt) => dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        _ => ts.to_string(),
    }
}

/// Minimal XML escaping so a member's name or message text can't break out of
/// its `<message>` element — a user could otherwise inject a stray `</message>`
/// (or `</conversation>`) tag and confuse the framing. Escapes the predefined
/// XML entities; `"` matters for the `from`/`time` attributes.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Writes one "- Name (marker): blurb" bullet, dropping the blurb clause when the
/// member has none. Shared by the `<group_members>` and `<directory>` sections so
/// both render members identically.
fn write_member_line(out: &mut String, name: &str, marker: &str, blurb: Option<&str>) {
    use std::fmt::Write as _;
    match blurb.map(str::trim).filter(|b| !b.is_empty()) {
        Some(blurb) => {
            let _ = writeln!(out, "- {name}{marker}: {blurb}");
        }
        None => {
            let _ = writeln!(out, "- {name}{marker}");
        }
    }
}

/// Renders the persona, clean transcript, and injected events into the prompt a
/// brain consumes. This is the self-managing context: everything a model needs
/// is here, so swapping in an LLM brain needs no other change. `now` is an
/// RFC 3339 timestamp with timezone offset (empty to omit the `<time>` section,
/// e.g. in tests).
fn assemble_prompt(
    persona: &AgentPersona,
    roster: &[RosterMember],
    directory: &[MemberInfo],
    summary: Option<&str>,
    transcript: &[ContextLine],
    events: &[String],
    now: &str,
) -> AgentPrompt {
    use std::fmt::Write as _;

    // Every section is wrapped in an XML-style element so the model can tell
    // exactly where each kind of data begins and ends (persona vs. inherited
    // variables vs. roster vs. the live conversation).
    let mut system = String::new();
    if !persona.system_prompt.is_empty() {
        let _ = writeln!(system, "<persona>\n{}\n</persona>", persona.system_prompt.trim());
    }
    if !persona.variables.is_empty() {
        // Sorted for a stable, reproducible prompt.
        let mut vars: Vec<(&String, &String)> = persona.variables.iter().collect();
        vars.sort_by(|a, b| a.0.cmp(b.0));
        system.push_str("\n<context>\n");
        for (key, value) in vars {
            let _ = writeln!(system, "- {key}: {value}");
        }
        system.push_str("</context>\n");
    }
    // Who is in the room, so the agent can address people by name and knows the
    // user is present. The self ("you") is the human this agent talks to.
    if !roster.is_empty() {
        system.push_str("\n<group_members>\n");
        for member in roster {
            // The human is flagged "(the user)" — never "you", which would read as
            // the agent itself; "you"/"你" is reserved for the user-facing UI.
            let marker = if member.is_self { " (the user)" } else { "" };
            write_member_line(&mut system, &member.name, marker, member.blurb.as_deref());
        }
        system.push_str("</group_members>\n");
    }
    // Everyone else in the workspace the agent may refer to by their unique name,
    // even though they aren't in this room. People already in <group_members> are
    // skipped so they aren't listed twice. This replaces a callable lookup tool:
    // as static context it keeps the decision a single completion (a tool loop's
    // follow-up turn 400s on providers like Gemini that require a per-call
    // `thought_signature` rig 0.40 can't round-trip).
    let present: HashSet<String> =
        roster.iter().map(|m| m.name.trim().to_ascii_lowercase()).collect();
    let mut others =
        directory.iter().filter(|m| !present.contains(&m.name.trim().to_ascii_lowercase())).peekable();
    if others.peek().is_some() {
        system.push_str("\n<directory>\n");
        for member in others {
            let marker = if member.is_user { " (the user)" } else { "" };
            write_member_line(&mut system, &member.name, marker, member.blurb.as_deref());
        }
        system.push_str("</directory>\n");
    }
    // The live transcript first, then the volatile bits (environment, time) last.
    // Ordering is deliberate for prompt caching: the transcript is append-only,
    // so its leading messages are byte-identical across an agent's turns and can
    // be served from the provider's prefix cache — but only when nothing volatile
    // precedes them. The per-turn <time> (and any <environment> events) therefore
    // trail the conversation rather than lead it, keeping the largest, most-
    // repeated section a stable, cacheable prefix.
    let mut conversation = String::new();
    // The compressed older history leads, standing in for the messages dropped
    // from the verbatim log below. It changes only when a compression runs (and
    // the tail shifts with it), so between compressions it stays a stable prefix —
    // just like the transcript — and doesn't defeat the provider's prefix cache.
    if let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        let _ = writeln!(conversation, "<summary>\n{summary}\n</summary>\n");
    }
    // Each message is its own XML element carrying the sender and the send time
    // (local, with timezone — same format as <time>), so the model can tell the
    // messages apart, attribute each to a speaker, and reason about when things
    // were said. Names and text are escaped so a stray tag can't break the frame.
    conversation.push_str("<conversation>\n");
    for line in transcript {
        let _ = writeln!(
            conversation,
            "<message from=\"{}\" time=\"{}\">{}</message>",
            xml_escape(&line.name),
            format_ts(line.ts),
            xml_escape(&line.text),
        );
    }
    conversation.push_str("</conversation>\n");
    if !events.is_empty() {
        conversation.push_str("\n<environment>\n");
        for event in events {
            let _ = writeln!(conversation, "- {event}");
        }
        conversation.push_str("</environment>\n");
    }
    // The current time (with timezone) trails everything, so the agent still
    // grounds "now" without a per-turn value invalidating the cacheable
    // transcript prefix in front of it.
    if !now.is_empty() {
        let _ = writeln!(conversation, "\n<time>\n{now}\n</time>");
    }

    AgentPrompt {
        system,
        conversation,
        persona_name: persona.name.clone(),
        last_line: transcript.last().map(|line| line.text.clone()),
        // Recallable memories are not part of the prompt *text* — they ride
        // alongside it as pull-tool data, set by the orchestrator after assembly.
        recallable_memories: Vec::new(),
    }
}

/// A tiny SplitMix64 PRNG: dependency-free and deterministic when seeded, so the
/// member shuffle can be reproduced in tests without pulling in `rand`.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
}

/// Fisher-Yates shuffle using [`Rng`].
fn shuffle<T>(items: &mut [T], rng: &mut Rng) {
    for i in (1..items.len()).rev() {
        items.swap(i, rng.below(i + 1));
    }
}

fn time_seed() -> u64 {
    now_ms() as u64 ^ 0x9E37_79B9_7F4A_7C15
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use tokio::sync::Notify;

    use crate::agent::brain::{
        AgentPrompt, Decision, SuggestionRequest, Suggestions, Summary, SummaryRequest,
    };
    use crate::agent::mock::RuleBrain;
    use crate::models::TokenUsage;
    use crate::state::StreamEvent;

    /// Deterministic, delay-free config so tests are reproducible and fast.
    fn cfg() -> LoopConfig {
        LoopConfig { pace_ms: 0, seed: Some(7), ..LoopConfig::default() }
    }

    fn app(brain: Arc<dyn AgentBrain>, config: LoopConfig) -> Arc<AppState> {
        Arc::new(AppState::with_runtime(AgentRuntime { brain, config, mock: true }))
    }

    /// Stores a user line the way the send handler does, returning its id.
    fn store_user(state: &AppState, group: &str, text: &str) -> String {
        let message = Message::conversation(group, "user-me", text, Some(vec![]));
        let id = message.id().to_string();
        state.store(group, message);
        id
    }

    /// Runs one turn with no interrupts (the sender is kept alive so the command
    /// channel never closes mid-turn).
    async fn run_once(state: &AppState, group: &str, trigger: Event) -> TurnOutcome {
        let (_tx, mut rx) = mpsc::channel::<Event>(8);
        run_turn(state, group, trigger, false, &mut rx).await
    }

    /// Counts how many times it is asked to decide (to prove round bounds).
    struct CountingReadBrain {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl AgentBrain for CountingReadBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Decision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Respond::read().into()
        }
    }

    /// Records every prompt it is given, then reads silently.
    struct RecordingBrain {
        seen: Arc<Mutex<Vec<AgentPrompt>>>,
    }
    #[async_trait]
    impl AgentBrain for RecordingBrain {
        async fn decide(&self, prompt: &AgentPrompt) -> Decision {
            self.seen.lock().unwrap().push(prompt.clone());
            Respond::read().into()
        }
    }

    /// Blocks inside `decide` until released, signalling when it has entered — so
    /// a test can interrupt an agent that is mid-inference.
    struct GatedBrain {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }
    #[async_trait]
    impl AgentBrain for GatedBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Decision {
            self.entered.notify_one();
            self.release.notified().await;
            Respond::speak("late").into()
        }
    }

    /// Always fails with a 429, to exercise the suspend-on-failure path.
    struct FailingBrain;
    #[async_trait]
    impl AgentBrain for FailingBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Decision {
            Decision {
                outcome: Outcome::Failed(BrainError {
                    status: Some(429),
                    reason: "Too Many Requests".into(),
                }),
                usage: None,
                remembered: Vec::new(),
            }
        }
    }

    /// A scripted brain: the 1st agent speaks, the 2nd fails (suspending the
    /// turn), and every later call speaks. Drives the resume test — the retry must
    /// re-run only the agent that failed, not the one that already spoke.
    struct ScriptedBrain {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl AgentBrain for ScriptedBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Decision {
            match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => Respond::speak("first").into(),
                1 => Decision {
                    outcome: Outcome::Failed(BrainError {
                        status: Some(429),
                        reason: "Too Many Requests".into(),
                    }),
                    usage: None,
                    remembered: Vec::new(),
                },
                _ => Respond::speak("resumed").into(),
            }
        }
    }

    /// Reads silently but reports a fact to remember every turn, to exercise the
    /// orchestrator's write-back path.
    struct RememberingBrain {
        fact: String,
    }
    #[async_trait]
    impl AgentBrain for RememberingBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Decision {
            Decision {
                outcome: Outcome::Responded(Respond::read()),
                usage: None,
                remembered: vec![self.fact.clone()],
            }
        }
    }

    /// Reads silently, but folds compression batches into the running summary by
    /// joining the lines (so a test can assert what was folded without a model).
    struct JoinBrain;
    #[async_trait]
    impl AgentBrain for JoinBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Decision {
            Respond::read().into()
        }
        async fn summarize(&self, request: &SummaryRequest) -> Result<Summary, BrainError> {
            let mut text = String::new();
            if let Some(prior) = &request.prior {
                text.push_str(prior);
                text.push('\n');
            }
            text.push_str(&request.lines.join(" | "));
            Ok(Summary { text, usage: None })
        }
    }

    /// The AI persona ids that have read a given message.
    fn readers(state: &AppState, group: &str, message_id: &str) -> Vec<String> {
        state
            .list(group)
            .into_iter()
            .find_map(|m| match m {
                Message::Conversation { id, read_by, .. } if id == message_id => read_by,
                _ => None,
            })
            .unwrap_or_default()
    }

    /// How many system (error) notices a group's log holds.
    fn system_count(state: &AppState, group: &str) -> usize {
        state.list(group).iter().filter(|m| matches!(m, Message::System { .. })).count()
    }

    #[tokio::test]
    async fn every_member_reads_and_turn_completes() {
        let state = app(Arc::new(RuleBrain::seeded(42)), cfg());
        let mid = store_user(&state, "lab", "hello aria");

        let outcome = run_once(&state, "lab", Event::User { message_id: mid.clone() }).await;
        assert!(matches!(outcome, TurnOutcome::Done));

        // Both AI members of "lab" processed the message (read receipts recorded).
        let readers = state
            .list("lab")
            .into_iter()
            .find_map(|m| match m {
                Message::Conversation { id, read_by, .. } if id == mid => read_by,
                _ => None,
            })
            .unwrap_or_default();
        assert!(readers.contains(&"aria".to_string()));
        assert!(readers.contains(&"nox".to_string()));
    }

    #[tokio::test]
    async fn silence_terminates_after_one_round() {
        let calls = Arc::new(AtomicUsize::new(0));
        let brain = Arc::new(CountingReadBrain { calls: calls.clone() });
        // Allow up to 3 rounds; a fully-silent round must still stop after one.
        let config = LoopConfig { max_rounds: 3, ..cfg() };
        let state = app(brain, config);
        let mid = store_user(&state, "lab", "hi");
        let mut stream = state.channel("lab").subscribe();

        let outcome = run_once(&state, "lab", Event::User { message_id: mid }).await;
        assert!(matches!(outcome, TurnOutcome::Done));

        // "lab" has 2 AI members; a silent round means exactly 2 inferences.
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Nothing spoken or shown — only read receipts on the stream.
        let (mut messages, mut reads) = (0, 0);
        while let Ok(event) = stream.try_recv() {
            match event {
                StreamEvent::Message(_) => messages += 1,
                StreamEvent::Read(_) => reads += 1,
                StreamEvent::Activity(_) | StreamEvent::Debug(_) | StreamEvent::Suggestions(_) => {}
            }
        }
        assert_eq!(messages, 0);
        assert_eq!(reads, 2);
    }

    #[tokio::test]
    async fn failed_inference_suspends_without_marking_read() {
        let state = app(Arc::new(FailingBrain), cfg());
        let mid = store_user(&state, "lab", "hi");

        let outcome = run_once(&state, "lab", Event::User { message_id: mid.clone() }).await;

        // The turn suspends at the first failing agent rather than completing.
        assert!(matches!(outcome, TurnOutcome::Suspended(Event::User { .. })));
        // Exactly one sanitized error notice reached the chat, carrying the code.
        assert_eq!(system_count(&state, "lab"), 1);
        let system = state
            .list("lab")
            .into_iter()
            .find_map(|m| match m {
                Message::System { status, reason, .. } => Some((status, reason)),
                _ => None,
            })
            .unwrap();
        assert_eq!(system.0, Some(429));
        assert_eq!(system.1, "Too Many Requests");
        // A failure is NOT a read: the message stays unread, so a retry can resume.
        assert!(readers(&state, "lab", &mid).is_empty());
    }

    #[tokio::test]
    async fn retry_resumes_only_the_unread_agent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = app(Arc::new(ScriptedBrain { calls: calls.clone() }), cfg());
        let mid = store_user(&state, "lab", "hi");

        // Initial turn: agent #1 speaks (reads), agent #2 fails → suspend.
        let outcome = run_once(&state, "lab", Event::User { message_id: mid.clone() }).await;
        assert!(matches!(outcome, TurnOutcome::Suspended(Event::User { .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(readers(&state, "lab", &mid).len(), 1); // only the speaker read
        assert_eq!(system_count(&state, "lab"), 1);

        // Retry (resuming): only the still-unread agent re-runs and now speaks.
        let (_tx, mut rx) = mpsc::channel::<Event>(8);
        let outcome =
            run_turn(&state, "lab", Event::User { message_id: mid.clone() }, true, &mut rx).await;
        assert!(matches!(outcome, TurnOutcome::Done));
        // One more decide call, not two — the agent that already spoke is skipped.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        // Both AI members have now read the message; the turn is fully resolved.
        assert_eq!(readers(&state, "lab", &mid).len(), 2);
        // No second error notice was raised.
        assert_eq!(system_count(&state, "lab"), 1);
    }

    #[tokio::test]
    async fn remembered_facts_are_persisted_under_the_current_identity() {
        let brain = Arc::new(RememberingBrain { fact: "the user prefers tea".into() });
        let state = app(brain, cfg());
        let mid = store_user(&state, "lab", "hi");

        let outcome = run_once(&state, "lab", Event::User { message_id: mid }).await;
        assert!(matches!(outcome, TurnOutcome::Done));

        // Aria remembered the fact under its own identity, recallable immediately
        // (stamped with the same hash recall filters on).
        let aria = state.workspace().recallable_memories("aria");
        assert_eq!(aria.len(), 1);
        assert_eq!(aria[0].content, "the user prefers tea");
    }

    #[test]
    fn moods_never_enter_the_transcript() {
        let state = AppState::with_runtime(AgentRuntime::mock());
        state.emit("lab", Message::conversation("lab", "aria", "hello there", None));
        state.emit("lab", Message::mood("lab", "nox", "🤔 thinking", None));

        let transcript = build_transcript(&state, "lab");

        // Only the spoken line survives; moods are filtered out of the context.
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].text, "hello there");
        assert_eq!(transcript[0].name, "Aria");
    }

    #[test]
    fn roster_lists_members_and_marks_the_user() {
        let persona = AgentPersona {
            name: "Aria".into(),
            system_prompt: "You are Aria.".into(),
            variables: std::collections::HashMap::new(),
        };
        let roster = vec![
            RosterMember { name: "You".into(), blurb: Some("Your own voice.".into()), is_self: true },
            RosterMember { name: "Nox".into(), blurb: Some("Dry strategist.".into()), is_self: false },
            RosterMember { name: "Sol".into(), blurb: None, is_self: false },
        ];
        let prompt = assemble_prompt(&persona, &roster, &[], None, &[], &[], "");
        // The user is present and flagged "(the user)"; blurbs render when set,
        // and a member without one still appears.
        assert!(prompt.system.contains("<group_members>"));
        assert!(prompt.system.contains("You (the user): Your own voice."));
        assert!(prompt.system.contains("Nox: Dry strategist."));
        assert!(prompt.system.contains("- Sol\n"));
    }

    #[test]
    fn directory_lists_workspace_members_outside_the_room() {
        let persona = AgentPersona {
            name: "Aria".into(),
            system_prompt: "You are Aria.".into(),
            variables: std::collections::HashMap::new(),
        };
        // Aria is in the room; the directory also carries the user and someone
        // not present.
        let roster = vec![RosterMember { name: "Aria".into(), blurb: None, is_self: false }];
        let directory = vec![
            MemberInfo { name: "Aria".into(), blurb: None, is_user: false },
            MemberInfo { name: "You".into(), blurb: Some("Your own voice.".into()), is_user: true },
            MemberInfo { name: "Nox".into(), blurb: Some("Dry strategist.".into()), is_user: false },
        ];
        let prompt = assemble_prompt(&persona, &roster, &directory, None, &[], &[], "");

        assert!(prompt.system.contains("<directory>"));
        // People not in the room are listed, the user flagged "(the user)".
        assert!(prompt.system.contains("You (the user): Your own voice."));
        assert!(prompt.system.contains("Nox: Dry strategist."));
        // Someone already in <group_members> isn't repeated in <directory>.
        let directory_section = prompt.system.split("<directory>").nth(1).unwrap();
        assert!(!directory_section.contains("Aria"));
    }

    #[test]
    fn injects_local_time_with_timezone() {
        let persona = AgentPersona {
            name: "Aria".into(),
            system_prompt: "You are Aria.".into(),
            variables: std::collections::HashMap::new(),
        };
        // A fixed RFC 3339 timestamp with offset renders in its own section.
        let prompt =
            assemble_prompt(&persona, &[], &[], None, &[], &[], "2026-07-25T15:30:00+08:00");
        assert!(prompt.conversation.contains("<time>"));
        assert!(prompt.conversation.contains("2026-07-25T15:30:00+08:00"));
    }

    #[test]
    fn transcript_messages_are_xml_isolated_with_a_timestamp() {
        let persona = AgentPersona {
            name: "Aria".into(),
            system_prompt: "You are Aria.".into(),
            variables: std::collections::HashMap::new(),
        };
        // 2026-07-25T00:00:00Z in epoch millis; the exact local rendering depends
        // on the test host's timezone, so assert the structure, not the offset.
        let transcript = vec![ContextLine {
            id: "m1".into(),
            name: "You".into(),
            text: "hi <there>".into(),
            ts: 1_774_396_800_000,
        }];
        let prompt = assemble_prompt(&persona, &[], &[], None, &transcript, &[], "");

        // Each line is its own element, tagged with the sender and a send time.
        assert!(prompt.conversation.contains("<message from=\"You\" time=\""));
        assert!(prompt.conversation.contains("2026-"));
        assert!(prompt.conversation.contains("</message>"));
        // Angle brackets in the body are escaped so they can't break the frame.
        assert!(prompt.conversation.contains("hi &lt;there&gt;"));
        assert!(!prompt.conversation.contains("hi <there>"));
    }

    #[tokio::test]
    async fn soft_event_injected_at_boundary() {
        let seen = Arc::new(Mutex::new(Vec::<AgentPrompt>::new()));
        let brain = Arc::new(RecordingBrain { seen: seen.clone() });
        let state = app(brain, cfg());
        let mid = store_user(&state, "lab", "hi");

        let (tx, mut rx) = mpsc::channel::<Event>(8);
        // Queue a soft event: the first agent's select picks it up and folds it
        // in at the boundary, so the second agent sees it.
        tx.send(Event::Environment { description: "It starts to rain.".into(), urgent: false })
            .await
            .unwrap();

        let outcome = run_turn(&state, "lab", Event::User { message_id: mid }, false, &mut rx).await;
        assert!(matches!(outcome, TurnOutcome::Done));

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(!seen[0].conversation.contains("rain"));
        assert!(seen[1].conversation.contains("rain"));
        drop(tx);
    }

    #[tokio::test]
    async fn hard_interrupt_discards_inflight_agent() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let brain = Arc::new(GatedBrain { entered: entered.clone(), release: release.clone() });
        let state = app(brain, cfg());
        let mid = store_user(&state, "lab", "hello");

        let (tx, mut rx) = mpsc::channel::<Event>(8);
        let running = state.clone();
        let handle = tokio::spawn(async move {
            run_turn(&running, "lab", Event::User { message_id: mid }, false, &mut rx).await
        });

        // Once an agent is mid-inference, fire a hard event (a new user message).
        entered.notified().await;
        tx.send(Event::User { message_id: "second".into() }).await.unwrap();

        let outcome = handle.await.unwrap();
        assert!(matches!(outcome, TurnOutcome::Preempted(Event::User { .. })));

        // The interrupted agent's line was discarded — never committed anywhere.
        let leaked = state
            .list("lab")
            .iter()
            .any(|m| matches!(m, Message::Conversation { text, .. } if text == "late"));
        assert!(!leaked);
        let _ = release; // kept alive; the gated future is dropped, not released
    }

    #[test]
    fn summary_section_leads_the_conversation() {
        let persona = AgentPersona {
            name: "Aria".into(),
            system_prompt: "You are Aria.".into(),
            variables: std::collections::HashMap::new(),
        };
        let transcript =
            vec![ContextLine { id: "m1".into(), name: "You".into(), text: "hi".into(), ts: 0 }];
        let prompt = assemble_prompt(
            &persona,
            &[],
            &[],
            Some("Earlier: they agreed on the plan."),
            &transcript,
            &[],
            "",
        );
        let convo = &prompt.conversation;
        assert!(convo.contains("<summary>"));
        assert!(convo.contains("Earlier: they agreed on the plan."));
        // The summary stands in for older history, so it precedes the live log.
        assert!(convo.find("<summary>").unwrap() < convo.find("<conversation>").unwrap());
    }

    #[test]
    fn tail_after_returns_only_lines_past_the_boundary() {
        let lines = |ids: &[&str]| {
            ids.iter()
                .map(|id| ContextLine {
                    id: (*id).into(),
                    name: "You".into(),
                    text: (*id).into(),
                    ts: 0,
                })
                .collect::<Vec<_>>()
        };
        let transcript = lines(&["a", "b", "c", "d"]);
        // Nothing compressed → the whole transcript is the tail.
        assert_eq!(tail_after(&transcript, None).len(), 4);
        // Compressed through "b" → only c, d remain live.
        let tail = tail_after(&transcript, Some("b"));
        assert_eq!(tail.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["c", "d"]);
        // A boundary that no longer exists falls back to the whole transcript.
        assert_eq!(tail_after(&transcript, Some("gone")).len(), 4);
    }

    #[tokio::test]
    async fn compression_folds_old_lines_and_shrinks_the_tail() {
        // High-water 3, keep 1: five lines must fold the oldest four, leave one.
        let config = LoopConfig { compress_after: 3, compress_keep: 1, ..cfg() };
        let state = Arc::new(AppState::with_runtime(AgentRuntime {
            brain: Arc::new(JoinBrain),
            config,
            mock: false,
        }));
        for i in 0..5 {
            state.emit("lab", Message::conversation("lab", "aria", format!("line{i}"), None));
        }

        maybe_compress(&state, "lab").await;

        let summary = state.summary("lab");
        assert!(summary.through_id.is_some(), "a compression should have run");
        // The four oldest lines are folded; the newest stays verbatim.
        assert!(summary.text.contains("line0"));
        assert!(summary.text.contains("line3"));
        assert!(!summary.text.contains("line4"));

        // The live tail the next turn sends is just the un-summarized line.
        let transcript = build_transcript(&state, "lab");
        let tail = tail_after(&transcript, summary.through_id.as_deref());
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].text, "line4");
    }

    #[tokio::test]
    async fn compression_is_skipped_below_the_high_water_mark() {
        let config = LoopConfig { compress_after: 10, compress_keep: 2, ..cfg() };
        let state = Arc::new(AppState::with_runtime(AgentRuntime {
            brain: Arc::new(JoinBrain),
            config,
            mock: false,
        }));
        for i in 0..4 {
            state.emit("lab", Message::conversation("lab", "aria", format!("line{i}"), None));
        }

        maybe_compress(&state, "lab").await;

        // Four lines is under the mark of ten: nothing compressed.
        assert!(state.summary("lab").through_id.is_none());
    }

    #[tokio::test]
    async fn mock_runtime_never_compresses() {
        // Same volume, but a mock runtime: compression must not run (no model).
        let config = LoopConfig { compress_after: 1, compress_keep: 0, ..cfg() };
        let state = app(Arc::new(RuleBrain::seeded(1)), config);
        for i in 0..5 {
            state.emit("lab", Message::conversation("lab", "aria", format!("line{i}"), None));
        }

        maybe_compress(&state, "lab").await;

        assert!(state.summary("lab").through_id.is_none());
    }

    /// A brain that returns scripted suggestions and counts how often it is asked,
    /// to exercise the generation + gating paths without a model.
    struct SuggestBrain {
        calls: Arc<AtomicUsize>,
        prompts: Vec<String>,
        usage: Option<TokenUsage>,
    }
    #[async_trait]
    impl AgentBrain for SuggestBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Decision {
            Respond::read().into()
        }
        async fn suggest(&self, request: &SuggestionRequest) -> Result<Suggestions, BrainError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Echo the context back the way the real brain reports it, so the test
            // can prove the recent transcript reaches the trace.
            Ok(Suggestions {
                prompts: self.prompts.clone(),
                usage: self.usage,
                system: "suggest-guidance".to_string(),
                context: request.recent.join("\n"),
            })
        }
    }

    /// Yields until the group's suggestions cache is populated (the spawned
    /// generation ran), or panics if it never does.
    async fn wait_for_suggestions(state: &Arc<AppState>, group: &str) {
        for _ in 0..1000 {
            if state.suggestions(group).generated_at != 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("suggestions were never generated");
    }

    #[test]
    fn time_of_day_buckets_the_clock() {
        assert_eq!(time_of_day(5), "morning");
        assert_eq!(time_of_day(10), "morning");
        assert_eq!(time_of_day(11), "afternoon");
        assert_eq!(time_of_day(16), "afternoon");
        assert_eq!(time_of_day(17), "evening");
        assert_eq!(time_of_day(21), "evening");
        // The night bucket wraps midnight and covers any out-of-range hour.
        assert_eq!(time_of_day(22), "night");
        assert_eq!(time_of_day(4), "night");
        assert_eq!(time_of_day(0), "night");
    }

    #[tokio::test]
    async fn mock_suggests_time_aware_openers() {
        let brain = RuleBrain::seeded(1);
        let request = |bucket: &str| SuggestionRequest {
            members: Vec::new(),
            summary: None,
            recent: Vec::new(),
            now: String::new(),
            time_of_day: bucket.to_string(),
            language: None,
            min_count: 3,
        };
        // Every bucket yields at least three openers, and they differ by time.
        let morning = brain.suggest(&request("morning")).await.unwrap().prompts;
        let evening = brain.suggest(&request("evening")).await.unwrap().prompts;
        assert!(morning.len() >= 3);
        assert!(evening.len() >= 3);
        assert_ne!(morning, evening);
        // An unknown bucket falls back to the night set rather than empty.
        assert!(!brain.suggest(&request("weird")).await.unwrap().prompts.is_empty());
        // The mock makes no LLM call, so it reports no usage.
        assert!(brain.suggest(&request("morning")).await.unwrap().usage.is_none());
    }

    #[tokio::test]
    async fn generate_suggestions_stores_streams_and_records_usage() {
        let calls = Arc::new(AtomicUsize::new(0));
        let brain = Arc::new(SuggestBrain {
            calls: calls.clone(),
            prompts: vec!["What's new?".into(), "How's everyone?".into(), "Any plans?".into()],
            usage: Some(TokenUsage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15, cached_prompt_tokens: 0 }),
        });
        let state = app(brain, cfg());
        state.emit("lab", Message::conversation("lab", "aria", "hi there", None));
        let mut stream = state.channel("lab").subscribe();

        generate_suggestions(state.clone(), "lab".to_string()).await;

        // The cache now holds the openers, stamped with a time and the last line.
        let cached = state.suggestions("lab");
        assert_eq!(cached.prompts.len(), 3);
        assert_ne!(cached.generated_at, 0);
        assert!(!cached.time_of_day.is_empty());
        assert!(cached.through_id.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A `suggestions` frame was pushed for open composers.
        let mut streamed = false;
        while let Ok(event) = stream.try_recv() {
            if let StreamEvent::Suggestions(s) = event {
                assert_eq!(s.prompts.len(), 3);
                streamed = true;
            }
        }
        assert!(streamed, "a suggestions frame should have been broadcast");

        // The usage-bearing pass counted as one request (feeds the cost panel).
        assert_eq!(state.debug_totals().requests, 1);

        // The trace carries the exact prompt the pass ran on — its system guidance
        // and the recent context — so the debug panel shows what informed the
        // openers, not an empty "public messages" box.
        let trace = state
            .debug_traces("lab")
            .into_iter()
            .find(|t| t.action == "suggest")
            .expect("the suggestion pass should record a trace");
        assert_eq!(trace.system, "suggest-guidance");
        assert!(trace.conversation.contains("Aria: hi there"), "context was {:?}", trace.conversation);
    }

    #[tokio::test]
    async fn request_suggestions_skips_when_fresh_and_rate_limits() {
        let calls = Arc::new(AtomicUsize::new(0));
        let brain = Arc::new(SuggestBrain {
            calls: calls.clone(),
            prompts: vec!["a".into(), "b".into(), "c".into()],
            usage: None,
        });
        let state = app(brain, cfg());
        state.emit("lab", Message::conversation("lab", "aria", "hello", None));

        // First request (stale: nothing cached) generates once.
        state.request_suggestions("lab", false);
        wait_for_suggestions(&state, "lab").await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The conversation hasn't moved and the bucket is the same, so a
        // non-forced request is a no-op — no second generation.
        state.request_suggestions("lab", false);
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Even a forced regenerate is rate-limited inside the cooldown window.
        state.request_suggestions("lab", true);
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
