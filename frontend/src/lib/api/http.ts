import type { GroupSuggestions, Message, Turn } from '../../types';
import { authFetch } from './authFetch';
import { FetchEventStream } from './eventStream';
import { jsonOrThrow, throwIfNotOk } from './problem';
import type {
  ActivityHandler,
  AgentTrace,
  ChatApi,
  DebugHandler,
  DebugUsage,
  HistoryWindow,
  MessageHandler,
  PersonaUsage,
  ReadHandler,
  ReadReceipt,
  ServerMeta,
  SuggestionsHandler,
  TurnHandler,
} from './types';
import { versionedBase } from './version';

/** Parses an SSE frame's JSON payload, yielding null on malformed data. */
function parseFrame<T>(data: string): T | null {
  try {
    return JSON.parse(data) as T;
  } catch {
    return null;
  }
}

/**
 * Per-event-name frame handling, keyed the same way the backend names its SSE
 * events. Each entry parses its payload and fans it out to that event kind's
 * subscribers on the given stream. `debug` is deliberately not here — it
 * never arrives on this connection at all, see {@link DebugStream}.
 */
const FRAME_HANDLERS: Record<string, (stream: GroupStream, data: string) => void> = {
  message: (stream, data) => {
    const parsed = parseFrame<Message>(data);
    if (parsed) for (const h of stream.message) h(parsed);
  },
  read: (stream, data) => {
    const parsed = parseFrame<ReadReceipt>(data);
    if (parsed) for (const h of stream.read) h(parsed);
  },
  activity: (stream, data) => {
    const parsed = parseFrame<{ active: boolean }>(data);
    if (parsed) for (const h of stream.activity) h(parsed.active);
  },
  turn: (stream, data) => {
    const parsed = parseFrame<Turn>(data);
    if (parsed) for (const h of stream.turn) h(parsed);
  },
  suggestions: (stream, data) => {
    const parsed = parseFrame<GroupSuggestions>(data);
    if (parsed) for (const h of stream.suggestions) h(parsed);
  },
};

/** Routes one parsed SSE frame to its event kind's handler, if recognized. */
function dispatchFrame(stream: GroupStream, eventName: string, data: string): void {
  FRAME_HANDLERS[eventName]?.(stream, data);
}

/**
 * The set of live subscribers on one group's SSE connection, split by event
 * kind. The connection stays open while any set is non-empty.
 */
interface GroupStream {
  source: FetchEventStream;
  message: Set<MessageHandler>;
  read: Set<ReadHandler>;
  activity: Set<ActivityHandler>;
  turn: Set<TurnHandler>;
  suggestions: Set<SuggestionsHandler>;
  /** Pending close from a grace period; cleared if a subscriber returns first. */
  closeTimer?: ReturnType<typeof setTimeout>;
  /** Set once the stream has connected; a later `open` event means a reconnect. */
  connected?: boolean;
}

/**
 * The debug panel's own connection (`?debug=true`), separate from
 * {@link GroupStream} — see that interface's sibling doc comment on
 * {@link SharedStreams} for why. Its only subscribers are debug handlers, so
 * there's nothing to split further within it.
 */
interface DebugStream {
  source: FetchEventStream;
  debug: Set<DebugHandler>;
  closeTimer?: ReturnType<typeof setTimeout>;
}

/** How long an idle group stream lingers before closing (see {@link SharedStreams}). */
const LINGER_MS = 300;

/**
 * One SSE connection per group, shared by every subscriber except the debug
 * panel. The backend multiplexes replies, read receipts, activity, and turn
 * progress as named events on a single `/stream`, so those four subscription
 * kinds ride one connection instead of opening four. The connection opens on
 * the first subscriber and — after a short grace period — closes when the
 * last one leaves. The grace period lets a React StrictMode remount or a
 * quick group switch-back reuse the live connection rather than drop and
 * reopen it, which a tunnel (cloudflared) would otherwise log as a stream
 * cancellation.
 *
 * `debug` frames carry a step closer to raw prompt/reasoning content than
 * anything else this API sends, so they ride a second, separate connection
 * (`?debug=true`, opened only while a debug-panel subscriber exists) instead
 * of being pushed to every tab that merely has a group open — see
 * `backend/src/routes/chat.rs`'s `StreamQuery`.
 */
class SharedStreams {
  private readonly byGroup = new Map<string, GroupStream>();
  private readonly byGroupDebug = new Map<string, DebugStream>();

  constructor(private readonly baseUrl: string) {}

  subscribeMessage(groupId: string, handler: MessageHandler): () => void {
    const stream = this.open(groupId);
    stream.message.add(handler);
    return () => this.drop(groupId, stream, stream.message, handler);
  }

  subscribeRead(groupId: string, handler: ReadHandler): () => void {
    const stream = this.open(groupId);
    stream.read.add(handler);
    return () => this.drop(groupId, stream, stream.read, handler);
  }

  subscribeActivity(groupId: string, handler: ActivityHandler): () => void {
    const stream = this.open(groupId);
    stream.activity.add(handler);
    return () => this.drop(groupId, stream, stream.activity, handler);
  }

  subscribeTurn(groupId: string, handler: TurnHandler): () => void {
    const stream = this.open(groupId);
    stream.turn.add(handler);
    return () => this.drop(groupId, stream, stream.turn, handler);
  }

  subscribeDebug(groupId: string, handler: DebugHandler): () => void {
    const stream = this.openDebug(groupId);
    stream.debug.add(handler);
    return () => this.dropDebug(groupId, stream, handler);
  }

  subscribeSuggestions(groupId: string, handler: SuggestionsHandler): () => void {
    const stream = this.open(groupId);
    stream.suggestions.add(handler);
    return () => this.drop(groupId, stream, stream.suggestions, handler);
  }

  /** Reuses the group's live stream, or opens one and wires its event listeners. */
  private open(groupId: string): GroupStream {
    const existing = this.byGroup.get(groupId);
    if (existing) {
      if (existing.closeTimer) {
        clearTimeout(existing.closeTimer);
        existing.closeTimer = undefined;
      }
      return existing;
    }

    // `source` is assigned right below — the dispatcher and onOpen callbacks
    // close over `stream` itself, so the object has to exist first.
    const stream = {
      message: new Set<MessageHandler>(),
      read: new Set<ReadHandler>(),
      activity: new Set<ActivityHandler>(),
      turn: new Set<TurnHandler>(),
      suggestions: new Set<SuggestionsHandler>(),
    } as GroupStream;
    // The default (unnamed) event carries a Message; the rest are named events.
    stream.source = new FetchEventStream(
      `${this.baseUrl}/groups/${groupId}/stream`,
      (eventName, data) => dispatchFrame(stream, eventName, data),
      () => {
        // The first `open` is the initial connect — the caller (ChatView's effect)
        // has already loaded history, so nothing to do. Every later `open` is a
        // reconnect after a drop, common through a tunnel, and the stream missed
        // whatever was broadcast while it was down (the channel keeps no backlog).
        // Reconcile so the UI heals on its own instead of needing a page refresh.
        if (stream.connected) this.resync(groupId, stream);
        else stream.connected = true;
      },
    );
    this.byGroup.set(groupId, stream);
    return stream;
  }

  /**
   * Refetches a group's messages after a reconnect and replays them through the
   * live message handlers. The existing merge (dedupe by id) drops anything the
   * client already has and adds what it missed; read receipts ride along on each
   * message's `readBy`, so missed reads heal too. Best-effort — a failure just
   * waits for the next reconnect.
   */
  private resync(groupId: string, stream: GroupStream): void {
    void authFetch(`${this.baseUrl}/groups/${groupId}/messages`, {
      headers: { Accept: 'application/json' },
    })
      .then((res) => (res.ok ? (res.json() as Promise<Message[]>) : null))
      .then((messages) => {
        if (!messages) return;
        for (const message of messages) {
          for (const handler of stream.message) handler(message);
        }
      })
      .catch(() => {
        // Offline / transient — the next reconnect reconciles again.
      });
  }

  /** Removes one handler, then closes the stream (after a grace period) if idle. */
  private drop<H>(groupId: string, stream: GroupStream, set: Set<H>, handler: H): void {
    set.delete(handler);
    if (this.isBusy(stream)) return;
    stream.closeTimer = setTimeout(() => {
      // A subscriber may have returned during the grace window; only close if
      // this is still the group's current, now-idle stream.
      if (this.isBusy(stream) || this.byGroup.get(groupId) !== stream) return;
      stream.source.close();
      this.byGroup.delete(groupId);
    }, LINGER_MS);
  }

  private isBusy(stream: GroupStream): boolean {
    return (
      stream.message.size > 0 ||
      stream.read.size > 0 ||
      stream.activity.size > 0 ||
      stream.turn.size > 0 ||
      stream.suggestions.size > 0
    );
  }

  /** {@link open}'s counterpart for the debug-only connection. */
  private openDebug(groupId: string): DebugStream {
    const existing = this.byGroupDebug.get(groupId);
    if (existing) {
      if (existing.closeTimer) {
        clearTimeout(existing.closeTimer);
        existing.closeTimer = undefined;
      }
      return existing;
    }

    const stream = { debug: new Set<DebugHandler>() } as DebugStream;
    stream.source = new FetchEventStream(
      `${this.baseUrl}/groups/${groupId}/stream?debug=true`,
      (eventName, data) => {
        if (eventName !== 'debug') return;
        const parsed = parseFrame<AgentTrace>(data);
        if (parsed) for (const h of stream.debug) h(parsed);
      },
      () => {
        // No resync on reconnect: unlike messages, a missed trace has no
        // "refetch and replay" story — the panel just picks up live traces
        // again from whenever the reconnect completes.
      },
    );
    this.byGroupDebug.set(groupId, stream);
    return stream;
  }

  /** {@link drop}'s counterpart for the debug-only connection. */
  private dropDebug(groupId: string, stream: DebugStream, handler: DebugHandler): void {
    stream.debug.delete(handler);
    if (stream.debug.size > 0) return;
    stream.closeTimer = setTimeout(() => {
      if (stream.debug.size > 0 || this.byGroupDebug.get(groupId) !== stream) return;
      stream.source.close();
      this.byGroupDebug.delete(groupId);
    }, LINGER_MS);
  }
}

/**
 * Talks to an AgoraLume backend over HTTP. The base URL is injected via
 * VITE_API_BASE_URL, so the frontend stays fully decoupled: point it at any
 * compatible backend, or leave it unset to fall back to the in-memory mock.
 *
 * The endpoints below follow the backend's OpenAPI contract (`openapi.yml` at
 * the repo root); the generated request/response types live in `./schema.d.ts`
 * (regenerate both with `pnpm gen:api`).
 */
export class HttpChatApi implements ChatApi {
  // All four subscription kinds share one SSE connection per group.
  private readonly streams: SharedStreams;
  private readonly baseUrl: string;

  constructor(rawBaseUrl: string) {
    this.baseUrl = versionedBase(rawBaseUrl);
    this.streams = new SharedStreams(this.baseUrl);
  }

  private async getJson<T>(path: string): Promise<T> {
    const res = await authFetch(`${this.baseUrl}${path}`, {
      headers: { Accept: 'application/json' },
    });
    return jsonOrThrow<T>(res, `GET ${path}`);
  }

  async probe(): Promise<ServerMeta | null> {
    try {
      const res = await fetch(`${this.baseUrl}/meta`, { headers: { Accept: 'application/json' } });
      if (!res.ok) return null;
      return (await res.json()) as ServerMeta;
    } catch {
      // Network error / server down.
      return null;
    }
  }

  listMessages(groupId: string, opts?: HistoryWindow): Promise<Message[]> {
    const params = new URLSearchParams();
    if (opts?.anchor) params.set('anchor', opts.anchor);
    if (opts?.before != null) params.set('before', String(opts.before));
    if (opts?.after != null) params.set('after', String(opts.after));
    if (opts?.since != null) params.set('since', String(opts.since));
    const qs = params.toString();
    return this.getJson<Message[]>(`/groups/${groupId}/messages${qs ? `?${qs}` : ''}`);
  }

  async sendMessage(groupId: string, text: string, personaId?: string): Promise<Message> {
    const res = await authFetch(`${this.baseUrl}/groups/${groupId}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text, personaId }),
    });
    // The server validates length and authorship (a message may only be
    // authored as a user identity) and explains a rejection in the problem
    // document, so surfacing its `detail` is what lets the composer say why.
    return jsonOrThrow<Message>(res, 'sendMessage');
  }

  async retry(groupId: string): Promise<void> {
    const res = await authFetch(`${this.baseUrl}/groups/${groupId}/retry`, { method: 'POST' });
    await throwIfNotOk(res, 'retry');
  }

  subscribe(groupId: string, handler: MessageHandler): () => void {
    return this.streams.subscribeMessage(groupId, handler);
  }

  subscribeReads(groupId: string, handler: ReadHandler): () => void {
    return this.streams.subscribeRead(groupId, handler);
  }

  subscribeActivity(groupId: string, handler: ActivityHandler): () => void {
    return this.streams.subscribeActivity(groupId, handler);
  }

  subscribeTurn(groupId: string, handler: TurnHandler): () => void {
    return this.streams.subscribeTurn(groupId, handler);
  }

  // Usage is two questions — "what did this cost" and "which character spent
  // it" — each answerable for one group or for everything. The backend used to
  // spell that as four routes under a `debug/` prefix; it is now two routes
  // with an optional `groupId`, and these four methods are the two scopes of
  // each.
  getUsage(): Promise<DebugUsage> {
    return this.getJson<DebugUsage>('/usage');
  }

  getGroupUsage(groupId: string): Promise<DebugUsage> {
    return this.getJson<DebugUsage>(`/usage?groupId=${encodeURIComponent(groupId)}`);
  }

  getPersonaUsage(groupId: string): Promise<PersonaUsage[]> {
    return this.getJson<PersonaUsage[]>(`/usage/by-persona?groupId=${encodeURIComponent(groupId)}`);
  }

  getGlobalPersonaUsage(): Promise<PersonaUsage[]> {
    return this.getJson<PersonaUsage[]>('/usage/by-persona');
  }

  listTraces(groupId: string): Promise<AgentTrace[]> {
    return this.getJson<AgentTrace[]>(`/groups/${groupId}/traces`);
  }

  subscribeDebug(groupId: string, handler: DebugHandler): () => void {
    return this.streams.subscribeDebug(groupId, handler);
  }

  getSuggestions(groupId: string): Promise<GroupSuggestions> {
    return this.getJson<GroupSuggestions>(`/groups/${groupId}/suggestions`);
  }

  /** POST to the same collection the GET reads — it creates a new set. */
  async regenerateSuggestions(groupId: string): Promise<void> {
    const res = await authFetch(`${this.baseUrl}/groups/${groupId}/suggestions`, {
      method: 'POST',
    });
    await throwIfNotOk(res, 'regenerateSuggestions');
  }

  subscribeSuggestions(groupId: string, handler: SuggestionsHandler): () => void {
    return this.streams.subscribeSuggestions(groupId, handler);
  }
}
