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

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::agent::brain::{Action, AgentBrain, AgentPersona, AgentPrompt, Respond};
use crate::agent::event::{Event, Salience, appraise};
use crate::agent::mock::RuleBrain;
use crate::models::{Message, now_ms};
use crate::state::AppState;

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
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self { max_rounds: 1, pace_ms: 450, seed: None }
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
}

/// A group's coordinator task: it owns the command channel and runs turns one at
/// a time, so there is never more than one turn in flight per group. A hard
/// interrupt returns here and immediately re-runs with the preempting event.
pub async fn coordinator_loop(state: Arc<AppState>, group_id: String, mut rx: mpsc::Receiver<Event>) {
    while let Some(event) = rx.recv().await {
        // The loop is busy for the whole turn (including any hard-interrupt
        // restarts); it goes idle only once fully done. The composer gates on
        // this so a new user message can't interleave with a running turn.
        state.set_active(&group_id, true);
        let mut trigger = event;
        loop {
            match run_turn(&state, &group_id, trigger, &mut rx).await {
                TurnOutcome::Done => break,
                TurnOutcome::Preempted(next) => trigger = next,
            }
        }
        state.set_active(&group_id, false);
    }
}

/// Runs one turn for a group. `rx` is watched throughout so events arriving
/// mid-turn are appraised: hard ones preempt (returning `Preempted`), soft ones
/// are folded into the context at the next member boundary.
async fn run_turn(
    state: &AppState,
    group_id: &str,
    trigger: Event,
    rx: &mut mpsc::Receiver<Event>,
) -> TurnOutcome {
    let runtime = &state.runtime;
    let cfg = &runtime.config;

    // The user's line every agent marks as read this turn (unlocks the composer).
    let read_target = match &trigger {
        Event::User { message_id, .. } => Some(message_id.clone()),
        Event::Environment { .. } => None,
    };

    // Events already folded into the context all agents can see. An environment
    // trigger seeds it; soft events drained at boundaries append to it.
    let mut injected_events: Vec<String> = Vec::new();
    if let Some(text) = trigger.as_context() {
        injected_events.push(text.to_string());
    }

    let mut rng = Rng::new(cfg.seed.unwrap_or_else(time_seed));

    for _round in 0..cfg.max_rounds.max(1) {
        // Phase 2: fresh membership, reshuffled each round.
        let Some((_self_id, member_ids)) = state.workspace().turn_members(group_id) else {
            return TurnOutcome::Done;
        };
        if member_ids.is_empty() {
            return TurnOutcome::Done;
        }
        let mut order = member_ids;
        shuffle(&mut order, &mut rng);

        let mut spoke_this_round = false;

        // Phase 3: serial pipeline — each agent decides on the updated context.
        for persona_id in order {
            let Some(persona) = build_persona(state, &persona_id) else {
                continue;
            };
            let transcript = build_transcript(state, group_id);

            // Assemble the prompt — the orchestrator owns context, so a brain is
            // just prompt-in/decision-out (the LLM boundary).
            let prompt = assemble_prompt(&persona, &transcript, &injected_events);
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
            let respond = {
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
                        resp = &mut fut => break resp,
                    }
                }
            };

            // Phase 4: route the decision to the two streams.
            let Respond { action, message, mood } = respond;
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

/// Emits a spoken line to the group (Context Stream + UI View).
fn emit_message(state: &AppState, group_id: &str, persona_id: &str, text: String) {
    state.emit(group_id, Message::conversation(group_id, persona_id, text, None));
}

/// Emits a mood to the group (UI View only — moods never enter the Context).
fn emit_mood(state: &AppState, group_id: &str, persona_id: &str, mood: String) {
    state.emit(group_id, Message::mood(group_id, persona_id, mood, None));
}

/// Cosmetic pacing between a mood and its message.
async fn pace(cfg: &LoopConfig) {
    if cfg.pace_ms > 0 {
        tokio::time::sleep(Duration::from_millis(cfg.pace_ms)).await;
    }
}

/// One line of the Context Stream — the filtered transcript agents read. Only
/// spoken lines appear; moods, read receipts and private notes never do.
struct ContextLine {
    name: String,
    text: String,
}

/// Builds the Context Stream: the group's log filtered to spoken lines only.
/// Moods and read receipts are excluded, so agents reason over clean context.
fn build_transcript(state: &AppState, group_id: &str) -> Vec<ContextLine> {
    let messages = state.list(group_id);
    let workspace = state.workspace();
    messages
        .into_iter()
        .filter_map(|message| match message {
            Message::Conversation { persona_id, text, .. } => {
                let name =
                    workspace.persona(&persona_id).map_or(persona_id, |p| p.name);
                Some(ContextLine { name, text })
            }
            Message::Mood { .. } => None,
        })
        .collect()
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

/// Renders the persona, clean transcript, and injected events into the prompt a
/// brain consumes. This is the self-managing context: everything a model needs
/// is here, so swapping in an LLM brain needs no other change.
fn assemble_prompt(
    persona: &AgentPersona,
    transcript: &[ContextLine],
    events: &[String],
) -> AgentPrompt {
    use std::fmt::Write as _;

    let mut system = String::new();
    if !persona.system_prompt.is_empty() {
        system.push_str(persona.system_prompt.trim());
        system.push('\n');
    }
    if !persona.variables.is_empty() {
        // Sorted for a stable, reproducible prompt.
        let mut vars: Vec<(&String, &String)> = persona.variables.iter().collect();
        vars.sort_by(|a, b| a.0.cmp(b.0));
        system.push_str("\nContext:\n");
        for (key, value) in vars {
            let _ = writeln!(system, "- {key}: {value}");
        }
    }
    let mut conversation = String::new();
    for line in transcript {
        let _ = writeln!(conversation, "{}: {}", line.name, line.text);
    }
    if !events.is_empty() {
        conversation.push_str("\n[Environment]\n");
        for event in events {
            let _ = writeln!(conversation, "- {event}");
        }
    }

    AgentPrompt {
        system,
        conversation,
        persona_name: persona.name.clone(),
        last_line: transcript.last().map(|line| line.text.clone()),
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

    use crate::agent::brain::AgentPrompt;
    use crate::agent::mock::RuleBrain;
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
        run_turn(state, group, trigger, &mut rx).await
    }

    /// Counts how many times it is asked to decide (to prove round bounds).
    struct CountingReadBrain {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl AgentBrain for CountingReadBrain {
        async fn decide(&self, _prompt: &AgentPrompt) -> Respond {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Respond::read()
        }
    }

    /// Records every prompt it is given, then reads silently.
    struct RecordingBrain {
        seen: Arc<Mutex<Vec<AgentPrompt>>>,
    }
    #[async_trait]
    impl AgentBrain for RecordingBrain {
        async fn decide(&self, prompt: &AgentPrompt) -> Respond {
            self.seen.lock().unwrap().push(prompt.clone());
            Respond::read()
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
        async fn decide(&self, _prompt: &AgentPrompt) -> Respond {
            self.entered.notify_one();
            self.release.notified().await;
            Respond::speak("late")
        }
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
                StreamEvent::Activity(_) => {}
            }
        }
        assert_eq!(messages, 0);
        assert_eq!(reads, 2);
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

        let outcome = run_turn(&state, "lab", Event::User { message_id: mid }, &mut rx).await;
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
            run_turn(&running, "lab", Event::User { message_id: mid }, &mut rx).await
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
}
