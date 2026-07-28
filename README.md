# AgoraLume

**AgoraLume is an open-source platform for user and multi-AI group chats with modular personas.**

You chat in a room alongside several AI personas at once. Each persona is a
modular identity — name, blurb, avatar, prompt, and its own memory — organized
into organizations and departments, and dropped into any group. Memories are
versioned to the prompt that recorded them, so rewriting a character doesn't
make it recall things the old version knew. A turn-based orchestrator decides
who speaks, so the room feels like a conversation rather than a wall of replies.
Personas run on the built-in rule-based brain out of the box, or on any
OpenAI-compatible model.

> [!NOTE]
> **Built with AI ("vibe coding").** The direction is human — what to build, the
> architecture, the API shape, the design trade-offs — but nearly everything you
> can *read* in this repository was written by AI: not only the low-level
> implementation, but this README, the notes under [`docs/`](docs/), and the
> commit messages. None of it is exhaustively hand-audited, so treat fine-grained
> code quality and documentation accuracy with corresponding caution, and review
> before relying on it in production.

## How it works

- **The backend is the single source of truth.** A Rust/[axum](https://github.com/tokio-rs/axum)
  server owns the workspace (personas, groups, memberships) and streams chat over
  SSE. The API is production-grade; only the data behind it is provisional (an
  in-memory store and simulated replies until you connect a database or a model).
- **The frontend is a static SPA.** React + TypeScript + [Mantine](https://mantine.dev),
  talking to the backend over the generated HTTP contract, or running fully
  offline against an in-browser mock.
- **One binary can be the whole site.** In bundle mode the backend serves the API
  and the SPA from a single origin — one port, one executable.

See [`docs/backend-architecture.md`](docs/backend-architecture.md) and
[`docs/agent-loop.md`](docs/agent-loop.md) for the design in depth.

## Quick start

Prerequisites: **Rust** (stable, edition 2024), **Node 22+**, and **pnpm 11**.
Rust is only needed when running the backend — the frontend mock runs on its own.

```sh
# Frontend only, against the offline in-browser mock (no backend, no Rust):
cd frontend
pnpm install
pnpm dev

# Full app on a single port with hot reload (starts the backend + Vite):
pnpm dev:single
```

`pnpm dev:single` exposes just the Vite port and proxies the API/SSE to a
backend bound to loopback. Ports and the proxy target are configurable — see
[`frontend/.env.example`](frontend/.env.example).

### Real model replies

By default personas use a rule-based mock (no API budget). To drive them with a
real model, set `AGORALUME_LLM=1` and point it at any OpenAI-compatible endpoint
(OpenAI, OpenRouter, Ollama, …) via the shell or a `.env` file — see
[`.env.example`](.env.example).

### Ship a bundle

```sh
node scripts/bundle.mjs
```

Produces `dist-bundle/AgoraLume-<platform>-<arch>.zip` containing the executable,
the built SPA, and a settings template. Unzip it and run the executable — it
serves everything on one port and opens a browser.

## Layout

| Path          | What                                                         |
| ------------- | ------------------------------------------------------------ |
| `backend/`    | Rust/axum API + SSE server, the SSOT and agent orchestrator. |
| `frontend/`   | React/TypeScript SPA (Mantine, Vite).                        |
| `scripts/`    | `bundle.mjs`, the single-binary packager.                    |
| `docs/`       | Architecture and agent-loop design notes.                    |
| `openapi.yml` | Generated HTTP contract (never hand-edited).                 |

## License

Split by component: the backend is **AGPL-3.0-only**
([`backend/LICENSE`](backend/LICENSE)); the frontend is **MIT**
([`frontend/LICENSE`](frontend/LICENSE)).
