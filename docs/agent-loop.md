# Multi-Agent Conversation Loop

The engine that drives multiple AI personas talking in a single **text chatroom**.
It lives in `backend/src/agent/`. This document describes the *shape and intent* of
the design; the code is the source of truth for every type, field, and constant.

> It is a text chatroom, not a face-to-face conversation. Moods are a UI concern
> and never enter the context other agents read (see [Two streams](#two-streams)).

The loop has three parts:

- **Global Loop** — per user message or environment event, run one *turn*.
- **Agent Local Loop** — within a turn, each agent decides once.
- **Event & Salience** — one appraisal deciding how any incoming event interrupts.

Code map: `brain.rs` (the LLM seam), `event.rs` (events + salience), `mock.rs`
(the non-LLM rule brain), `turn.rs` (the loop and prompt assembly).

---

## Design decisions

These are the *why* behind the mechanism — the part not recoverable from the code.

| # | Topic | Decision |
|---|-------|----------|
| 1 | **Termination** | A hard round budget. One message/event triggers a turn; a turn scans at most `max_rounds` rounds (default **1**). A round continues only if at least one agent spoke *and* `round < max_rounds`; a fully-silent round ends the turn. Upper bound `max_rounds × N` inferences — no infinite chatter, no "stuck" appearance, no runaway cost. |
| 2 | **Serial, not parallel** | No parallelism and no pre-computed priority queue. Members are **shuffled** each round and called **one by one**; each agent reads the context already updated by earlier speakers, so it converges naturally without re-runs. |
| 3 | **Willingness is free** | "Whether to speak" and "what to say" are the *same* single inference. There is no separate willingness pass — exactly one inference per agent per slot. |
| 4 | **Hard interrupt discards** | A preempted agent's output is dropped entirely; nothing is committed to context. A half-formed answer is neither known in full nor clean, so it is not kept. |
| 5 | **One salience entry point** | A single `appraise(event) → Hard \| Soft \| Ignore`. Message conflict and environment-event interruption are the same judgement. |
| 6 | **Moods are UI-only** | Other agents' context never contains moods — deliberate, because this is a text chatroom. |

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

| Action | Context Stream | UI View |
|--------|:--------------:|:-------:|
| `speak` | message | bubble |
| `speak_with_mood` | message | bubble + mood |
| `mood` | — | mood only |
| `read` | — | — |

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

Interrupt correctness is subtle: a soft event must *not* drop the running future,
only a hard one does. The exact race lives in `turn.rs::run_turn`; the invariant is
covered by unit tests.

---

## Two streams

Information is deliberately segregated:

- **Context Stream** — server-internal, `message`-only. *Not* a network stream: it
  is the filtered history used to build each agent's prompt. Moods and read receipts
  are excluded, keeping context maximally clean.
- **UI View Stream** — the outward SSE feed (`message` + mood), rendering bubbles
  and mood animations. Corresponds to `GET /groups/{id}/stream`.

> Per-agent long-term memory is intentionally **not** part of this build. Personas
> reason over the current transcript only; the conversation itself is the shared
> record. A private memory store can slot in behind the orchestrator later without
> changing the `AgentBrain` seam.

---

## Testing without an LLM

The whole loop is orchestration *except* the single inference in step 3, so the
entire loop is deterministically testable behind two injection seams:

1. **`AgentBrain`** — the only seam a real LLM replaces. The bundled `RuleBrain`
   scripts decisions (e.g. "speak when addressed, otherwise pick a mood/read").
2. **Clock / event source** — injectable delays (or `tokio::time::pause()`) so a
   hard interrupt can be fired precisely mid-inference without flakiness.

Assertable behaviours: single-pass shuffled pipeline, correct per-action stream
routing, `max_rounds` termination and silent-round end, Soft boundary injection,
Hard preemption (abort + discard + restart), and stream segregation (context
carries only messages).

> This matches the project stance: the API is production-grade; only the data layer
> (in-memory store + simulated replies) is provisional. The `AgentBrain` seam keeps
> the temporary mock brain and a future LLM adapter cleanly separated behind an
> unchanged orchestrator.
