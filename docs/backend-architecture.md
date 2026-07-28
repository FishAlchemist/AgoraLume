# Backend Architecture

The AgoraLume backend is an [axum](https://github.com/tokio-rs/axum) HTTP + SSE
server. This document describes the overall structure and the decisions behind it.
The **code is the source of truth** — for the HTTP contract see `openapi.yml`
(generated from the route annotations), and for every type and constant see the
Rust source. This document does not restate them.

## Positioning

- **The backend is the single source of truth (SSOT).** The workspace — personas,
  groups, memberships — is owned here; the frontend consumes it.
- **The API is production-grade; only the data behind it is provisional.** This
  build uses an in-memory store and a simulated (non-LLM) agent brain. Swapping in
  a database or a real LLM does not change the API surface. "Mock/simulated" applies
  to the _data layer_, never the API.

## Tech stack

- **axum** — HTTP routing and extractors.
- **utoipa** + **utoipa-axum** — OpenAPI is generated _from the code_ (route macros
  and `ToSchema` derives). `cargo run -- --dump-openapi` writes `openapi.yml`, which
  the frontend's type generation consumes. Never hand-edit `openapi.yml`.
- **tokio** — async runtime; `broadcast` for SSE fan-out, `mpsc` for per-group
  coordinators.
- **serde** — wire format. Rust is snake_case; the JSON contract is camelCase via
  `#[serde(rename_all = "camelCase")]`, matching the frontend types.

## Module map

Each file owns one concern; follow the link into the code for detail.

| Module         | Owns                                                                                                                                                                                                                                   |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `main.rs`      | Process entry, tracing, `--dump-openapi` one-shot, server bind.                                                                                                                                                                        |
| `config.rs`    | Environment-driven configuration (bind address, data dir, LLM). Loads a `.env` beside the executable (or the working dir in dev) at startup, so a bundle is configured with a file rather than shell exports; real env vars still win. |
| `models.rs`    | Wire types: `Persona` (with its identity `promptHash`), `Memory`, `PromptLabel`, `Message` (conversation / mood), `ReadReceipt`, `GroupSuggestions`, `ServerMeta`.                                                                        |
| `workspace.rs` | The SSOT: personas, groups, memberships; per-persona memories partitioned by identity hash; persona/variable resolution with org→dept→persona inheritance.                                                                             |
| `state.rs`     | In-memory `AppState`: workspace, per-group message logs, cached conversation suggestions (with a cooldown/single-flight gate), per-group SSE broadcast channels, per-group coordinators, and the agent runtime.                        |
| `routes/`      | HTTP surface. `chat.rs` holds the endpoints; `mod.rs` composes the router and the OpenAPI document.                                                                                                                                    |
| `agent/`       | The multi-agent conversation engine — see [agent-loop.md](./agent-loop.md).                                                                                                                                                            |

## Data flow

A group has two distinct flows, deliberately kept apart (see the agent-loop doc
for the rationale):

- **Context Stream** — server-internal, message-only history used to build agent
  prompts. Not exposed over the network.
- **UI View Stream** — the outward SSE feed (`GET /groups/{id}/stream`): default
  `message` frames (replies and moods) and named `read` frames (read receipts),
  fanned out per group via a tokio `broadcast` channel.

## Request → turn lifecycle

```
 POST /groups/{id}/messages ──► store user message (not broadcast; client already has it)
 POST /groups/{id}/events   ──► (environment event)
             │
             ▼
   AppState::dispatch(group, Event)
             │  lazily spawns a per-group coordinator task on first use
             ▼
   coordinator_loop  ── owns the mpsc receiver; runs one turn at a time
             │
             ▼
   the multi-agent turn (see agent-loop.md)
             │  emits over the group's broadcast channel as it goes
             ▼
   GET /groups/{id}/stream  ── SSE: `message` and `read` frames to every subscriber
```

Key properties:

- **One coordinator per group**, spawned lazily so idle groups run nothing. It owns
  the receiver and serializes turns, so a group never has two turns in flight.
- **Dispatch is fire-and-forget.** The POST returns immediately; agent output
  arrives asynchronously on the SSE stream. A full command buffer drops the newest
  event as back-pressure (turns are infrequent).
- **The user's own message is stored but not broadcast** — the client renders it
  from the POST response, so re-broadcasting would duplicate it.

## The one seam that matters

Everything above is production orchestration. The _only_ component a real LLM
replaces is the `AgentBrain` trait (`agent/brain.rs`): a pure prompt → decision
function. The orchestrator owns all context assembly, so swapping the bundled rule
brain (`agent/mock.rs`) for the LLM brain (`agent/llm.rs`) changes nothing else.
The LLM brain is a rig-core extractor over any OpenAI-compatible endpoint; enable
it with `AGORALUME_LLM=1` and set `AGORALUME_LLM_BASE_URL` / `AGORALUME_LLM_MODEL`
(and `AGORALUME_LLM_API_KEY` if the endpoint needs one) — via the shell or a
`.env` file beside the executable (see `.env.example`). This is what makes the
loop testable without an LLM and connectable to one without a rewrite — detailed
in [agent-loop.md](./agent-loop.md).

## Persona memory & identity

A persona's `system_prompt` is freely editable, which creates an out-of-character
hazard: rewrite who a character is, and old memories would feed back to someone who
no longer resembles the character that recorded them. The fix is a content
**identity hash** and per-version partitioning.

- **The hash covers the raw `system_prompt` text only** — not the fully-resolved
  `AgentPrompt.system` (which also carries the roster / `<directory>` and would
  churn when group membership changes), and **not variables**. Variables inherit
  org → department → persona, so hashing resolved values would let an org or
  department edit silently bump a persona's version from a UI its own editor never
  sees — an invisible failure. The hash is the text box the user is looking at when
  they think "I'm redefining this character."
- **Content-addressed and nameable.** Pasting the old prompt text back resolves to
  the same hash (a counter would call it a new version); a `{ hash, label }` side
  table lets a user name a version git-tag style ("night-shift 版") without
  polluting `Persona` itself.
- **Memories are stamped with the persona's current hash**, partitioning them per
  identity. Recall filters to the current hash: memories written by earlier
  versions are kept and still listed in the management UI, but held out of recall
  so the character stays true to who it is now. Memories cascade-delete with the
  persona.
- **Accepted limitation.** Shared org/department variables (designed to carry a
  common tone down the inheritance chain) can shift a persona's effective voice
  without changing its hash. This is not solved automatically; the manual memory
  UI is the escape hatch, deliberately in place of any auto-bump — bumping the
  version on an unrelated edit (a typo fix, a color change) would look like the
  character mysteriously losing its memory.

The write and recall mechanics — the two pull tools — live in
[agent-loop.md](./agent-loop.md#persona-memory); the REST surface for browsing and
managing memories is generated into `openapi.yml`.

## Conventions

- OpenAPI and the frontend types are generated, never hand-written. Regenerate
  after touching any route or schema.
- Clippy is clean with no `#[allow]` suppression — fix the structure instead.
- The frontend linter is Biome; `openapi-typescript` is pinned to v6.
