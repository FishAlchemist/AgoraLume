# Multi-Agent Conversation Loop

The engine that drives multiple AI personas talking in a single **text chatroom**.
It lives in `backend/src/agent/`. This document describes the _shape and intent_ of
the design; the code is the source of truth for every type, field, and constant.

> It is a text chatroom, not a face-to-face conversation. Moods are a UI concern
> and never enter the context other agents read (see [Two streams](#two-streams)).

The loop has three parts:

- **Global Loop** — per user message or environment event, run one _turn_.
- **Agent Local Loop** — within a turn, each agent decides once.
- **Event & Salience** — one appraisal deciding how any incoming event interrupts.

Code map: `brain.rs` (the seam), `event.rs` (events + salience), `mock.rs`
(the non-LLM rule brain), `llm.rs` (the OpenAI-compatible LLM brain), `turn.rs`
(the loop and prompt assembly).

---

## Design decisions

These are the _why_ behind the mechanism — the part not recoverable from the code.

| #   | Topic                        | Decision                                                                                                                                                                                                                                                                                                                                    |
| --- | ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | **Termination**              | A hard round budget. One message/event triggers a turn; a turn scans at most `max_rounds` rounds (default **1**). A round continues only if at least one agent spoke _and_ `round < max_rounds`; a fully-silent round ends the turn. Upper bound `max_rounds × N` inferences — no infinite chatter, no "stuck" appearance, no runaway cost. |
| 2   | **Serial, not parallel**     | No parallelism and no pre-computed priority queue. Members are **shuffled** each round and called **one by one**; each agent reads the context already updated by earlier speakers, so it converges naturally without re-runs.                                                                                                              |
| 3   | **Willingness is free**      | "Whether to speak" and "what to say" are the _same_ single inference. There is no separate willingness pass — exactly one inference per agent per slot.                                                                                                                                                                                     |
| 4   | **Hard interrupt discards**  | A preempted agent's output is dropped entirely; nothing is committed to context. A half-formed answer is neither known in full nor clean, so it is not kept.                                                                                                                                                                                |
| 5   | **One salience entry point** | A single `appraise(event) → Hard \| Soft \| Ignore`. Message conflict and environment-event interruption are the same judgement.                                                                                                                                                                                                            |
| 6   | **Moods are UI-only**        | Other agents' context never contains moods — deliberate, because this is a text chatroom.                                                                                                                                                                                                                                                   |

---

## Global Loop

```
 [ user message ] ───────┐
                         ├─► appraise (§ Event & Salience)
 [ environment event ] ──┘        Hard(preempt) / Soft(defer) / Ignore
                                        │ (passes → begin a turn)
                                        ▼
                   round r of max_rounds:
                     shuffle the N AI members            (random order each round)
                     for agent in shuffled:              (serial pipeline)
                         Agent Local Loop                (reads context updated by
                                                          earlier speakers this round)
                                        │
                   anyone spoke and r < max_rounds ?
                     yes → r += 1, reshuffle
                     no  → turn ends
```

---

## Agent Local Loop

Each agent runs **once** per slot and expresses exactly one of four actions.

```
 1. Gather     persona (+ inherited variables) + clean transcript
 2. Assemble   → AgentPrompt { system, conversation, … }        (orchestrator-owned)
 3. Decide     brain.decide(prompt) → Respond                   (the one inference)
 4. Route      fan the action out to the two streams            (see table below)
 5. Handover   at the boundary, inject any pending Soft events for the next agent
```

The four actions and where each lands:

| Action            | Context Stream |    UI View    |
| ----------------- | :------------: | :-----------: |
| `speak`           |    message     |    bubble     |
| `speak_with_mood` |    message     | bubble + mood |
| `mood`            |       —        |   mood only   |
| `read`            |       —        |       —       |

The `respond` tool is the entire brain output surface. Its exact fields are the
`Respond` type in `brain.rs`; do not restate them here.

---

## Event & Salience

One appraisal, regardless of whether the event is a user message or an environment
event (rain, time passing, an emergency):

- **Hard** — the risk of answering stale context outweighs the cost of discarding.
  Abort the in-flight agent, **drop its partial output** (decision 4), and restart
  appraisal. This is a `tokio::select!` race that drops the `decide` future.
- **Soft** — do not cut off the running agent. Queue the event and inject it into
  the context at the next agent boundary, balancing throughput and realism.
- **Ignore** — not salient; dropped.

Interrupt correctness is subtle: a soft event must _not_ drop the running future,
only a hard one does. The exact race lives in `turn.rs::run_turn`; the invariant is
covered by unit tests.

---

## Two streams

Information is deliberately segregated:

- **Context Stream** — server-internal, `message`-only. _Not_ a network stream: it
  is the filtered history used to build each agent's prompt. Moods and read receipts
  are excluded, keeping context maximally clean.
- **UI View Stream** — the outward SSE feed (`message` + mood), rendering bubbles
  and mood animations. Corresponds to `GET /groups/{id}/stream`.

> Per-agent long-term memory now lives behind this seam — added without changing
> the `AgentBrain` signature, exactly as the seam promised. See
> [Persona memory](#persona-memory) below.

---

## Persona memory

Personas carry private, long-term memories, exposed to the brain as two rig tools
rather than folded into the prompt:

- **`recall_memory`** — registered **only when** the persona has memories for its
  _current_ identity (the identity hash in
  [backend-architecture.md](./backend-architecture.md#persona-memory--identity)).
  It returns those memories' contents on demand.
- **`remember`** — always registered; lets a persona store a fact it wants to
  keep. The orchestrator writes it through the same `Workspace::add_memory` path
  the REST endpoint uses, so the brain stays a pure prompt → decision function.

Both are **pull** tools, and that is the point. A turn that reaches for one costs
a _second_ provider request (the model calls the tool, rig feeds the result back,
then the model finalizes); a turn that touches neither stays a single request.
Recall is rare, so injecting every persona's memories into every prompt would burn
the request and token budget on a payload almost never read. Registering a tool is
itself free — rig finalizes the instant the model calls `final_result` — so cost
is incurred only on actual use. Because a tool is always present, decisions always
run in rig's `Tool` output mode, and the agent's `max_turns` is raised above 1 so
a tool loop isn't cut short by a `MaxTurnsError`.

## Tools, structured output, and cost

`respond` is not a callable tool — it is the single-shot **structured output** that
finalizes every decision, kept that way deliberately for determinism even after
native-Gemini routing removed the general blocker to tool use. Real tools (memory,
and anything added later) coexist with that schema through rig-core's
`OutputMode::Auto`, which resolves to `Tool` mode wherever a provider's native
structured output would otherwise suppress tool calls: the schema is registered as
a synthetic `final_result` tool the model calls last, after freely using its real
tools. `respond`'s validation needs no changes for this — rig handles the switch.

Two rules follow from the cost model above:

- **A real tool earns its request only when static context can't serve the same
  fact for free.** Don't add a pull tool for something the prompt already carries
  (e.g. the roster / `<directory>` block).
- **External world-state is a push, not a pull.** Rain, time, an emergency —
  anything room-wide — belongs in `Event::Environment` (see
  [Event & Salience](#event--salience)), not a `get_*` tool that would open a
  second, competing, request-costing pathway for the same kind of fact.

---

## Testing without an LLM

The whole loop is orchestration _except_ the single inference in step 3, so the
entire loop is deterministically testable behind two injection seams:

1. **`AgentBrain`** — the only seam a real LLM replaces. The bundled `RuleBrain`
   scripts decisions (e.g. "speak when addressed, otherwise pick a mood/read");
   the `LlmBrain` in `llm.rs` is the real one, enabled with `AGORALUME_LLM=1`
   plus the `AGORALUME_LLM_*` endpoint settings.
2. **Clock / event source** — injectable delays (or `tokio::time::pause()`) so a
   hard interrupt can be fired precisely mid-inference without flakiness.

Assertable behaviours: single-pass shuffled pipeline, correct per-action stream
routing, `max_rounds` termination and silent-round end, Soft boundary injection,
Hard preemption (abort + discard + restart), and stream segregation (context
carries only messages).

> This matches the project stance: the API is production-grade; only the persistence
> layer (the in-memory store) is provisional. Replies come from either the mock
> `RuleBrain` (default) or the real `LlmBrain` (`AGORALUME_LLM=1`), kept cleanly
> separated behind an unchanged orchestrator by the `AgentBrain` seam.
