import type { GroupSuggestions, Message, Turn } from '../../types';
import { versionedBase } from './version';
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

/** Parses an SSE frame's JSON payload, yielding null on malformed data. */
function parseFrame<T>(data: string): T | null {
  try {
    return JSON.parse(data) as T;
  } catch {
    return null;
  }
}

/**
 * The set of live subscribers on one group's SSE connection, split by event
 * kind. The connection stays open while any set is non-empty.
 */
interface GroupStream {
  source: EventSource;
  message: Set<MessageHandler>;
  read: Set<ReadHandler>;
  activity: Set<ActivityHandler>;
  turn: Set<TurnHandler>;
  debug: Set<DebugHandler>;
  suggestions: Set<SuggestionsHandler>;
  /** Pending close from a grace period; cleared if a subscriber returns first. */
  closeTimer?: ReturnType<typeof setTimeout>;
  /** Set once the stream has connected; a later `open` event means a reconnect. */
  connected?: boolean;
}

/** How long an idle group stream lingers before closing (see {@link SharedStreams}). */
const LINGER_MS = 300;

/**
 * One SSE connection per group, shared by every subscriber. The backend
 * multiplexes replies, read receipts, activity, and debug traces as named
 * events on a single `/stream`, so all four subscription kinds ride one
 * EventSource instead of opening four. The connection opens on the first
 * subscriber and — after a short grace period — closes when the last one
 * leaves. The grace period lets a React StrictMode remount or a quick
 * group switch-back reuse the live connection rather than drop and reopen it,
 * which a tunnel (cloudflared) would otherwise log as a stream cancellation.
 */
class SharedStreams {
  private readonly byGroup = new Map<string, GroupStream>();

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
    const stream = this.open(groupId);
    stream.debug.add(handler);
    return () => this.drop(groupId, stream, stream.debug, handler);
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

    const source = new EventSource(`${this.baseUrl}/groups/${groupId}/stream`);
    const stream: GroupStream = {
      source,
      message: new Set(),
      read: new Set(),
      activity: new Set(),
      turn: new Set(),
      debug: new Set(),
      suggestions: new Set(),
    };
    // The default (unnamed) event carries a Message; the rest are named events.
    source.addEventListener('message', (e: MessageEvent<string>) => {
      const data = parseFrame<Message>(e.data);
      if (data) for (const h of stream.message) h(data);
    });
    source.addEventListener('read', (e: MessageEvent<string>) => {
      const data = parseFrame<ReadReceipt>(e.data);
      if (data) for (const h of stream.read) h(data);
    });
    source.addEventListener('activity', (e: MessageEvent<string>) => {
      const data = parseFrame<{ active: boolean }>(e.data);
      if (data) for (const h of stream.activity) h(data.active);
    });
    source.addEventListener('turn', (e: MessageEvent<string>) => {
      const data = parseFrame<Turn>(e.data);
      if (data) for (const h of stream.turn) h(data);
    });
    source.addEventListener('debug', (e: MessageEvent<string>) => {
      const data = parseFrame<AgentTrace>(e.data);
      if (data) for (const h of stream.debug) h(data);
    });
    source.addEventListener('suggestions', (e: MessageEvent<string>) => {
      const data = parseFrame<GroupSuggestions>(e.data);
      if (data) for (const h of stream.suggestions) h(data);
    });
    source.onopen = () => {
      // The first `open` is the initial connect — the caller (ChatView's effect)
      // has already loaded history, so nothing to do. Every later `open` is a
      // reconnect after a drop, common through a tunnel, and the stream missed
      // whatever was broadcast while it was down (the channel keeps no backlog).
      // Reconcile so the UI heals on its own instead of needing a page refresh.
      if (stream.connected) this.resync(groupId, stream);
      else stream.connected = true;
    };
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
    void fetch(`${this.baseUrl}/groups/${groupId}/messages`, {
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
      stream.debug.size > 0 ||
      stream.suggestions.size > 0
    );
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
    const res = await fetch(`${this.baseUrl}${path}`, {
      headers: { Accept: 'application/json' },
    });
    if (!res.ok) throw new Error(`GET ${path} failed: ${res.status}`);
    return (await res.json()) as T;
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
    const res = await fetch(`${this.baseUrl}/groups/${groupId}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text, personaId }),
    });
    if (!res.ok) throw new Error(`sendMessage failed: ${res.status}`);
    return (await res.json()) as Message;
  }

  async retry(groupId: string): Promise<void> {
    const res = await fetch(`${this.baseUrl}/groups/${groupId}/retry`, { method: 'POST' });
    if (!res.ok) throw new Error(`retry failed: ${res.status}`);
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

  getUsage(): Promise<DebugUsage> {
    return this.getJson<DebugUsage>('/debug/usage');
  }

  getGroupUsage(groupId: string): Promise<DebugUsage> {
    return this.getJson<DebugUsage>(`/groups/${groupId}/debug/usage`);
  }

  getPersonaUsage(groupId: string): Promise<PersonaUsage[]> {
    return this.getJson<PersonaUsage[]>(`/groups/${groupId}/debug/usage/by-persona`);
  }

  getGlobalPersonaUsage(): Promise<PersonaUsage[]> {
    return this.getJson<PersonaUsage[]>('/debug/usage/by-persona');
  }

  listTraces(groupId: string): Promise<AgentTrace[]> {
    return this.getJson<AgentTrace[]>(`/groups/${groupId}/debug/traces`);
  }

  subscribeDebug(groupId: string, handler: DebugHandler): () => void {
    return this.streams.subscribeDebug(groupId, handler);
  }

  getSuggestions(groupId: string): Promise<GroupSuggestions> {
    return this.getJson<GroupSuggestions>(`/groups/${groupId}/suggestions`);
  }

  async regenerateSuggestions(groupId: string): Promise<void> {
    const res = await fetch(`${this.baseUrl}/groups/${groupId}/suggestions/regenerate`, {
      method: 'POST',
    });
    if (!res.ok) throw new Error(`regenerateSuggestions failed: ${res.status}`);
  }

  subscribeSuggestions(groupId: string, handler: SuggestionsHandler): () => void {
    return this.streams.subscribeSuggestions(groupId, handler);
  }
}
