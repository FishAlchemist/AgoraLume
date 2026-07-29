import type { GroupSuggestions, Message, Turn } from '../../types';

/**
 * Upper bound on the initial history page, matching the backend's `INITIAL_CAP`.
 * When a group is opened with more unread than this (e.g. many event-triggered
 * turns piled up while the user was away), the tail is loaded and the rest pages
 * in — so a page returned at exactly this length means the unread run was
 * truncated and more unread lines exist above the loaded window. The frontend
 * mock mirrors the same bound, and the read-tracking divider uses it to tell a
 * capped backlog from a page that simply starts at the first unread line.
 */
export const INITIAL_PAGE_CAP = 160;

export type MessageHandler = (message: Message) => void;

/** A turn snapshot: the current processing round's trigger + per-member progress. */
export type TurnHandler = (turn: Turn) => void;

/** Options for paging message history — see {@link ChatApi.listMessages}. */
export interface HistoryPage {
  /** Return only messages strictly older than this message id (page upward). */
  before?: string;
  /** Cap to the newest N of the selected range (the default page is the tail). */
  limit?: number;
  /**
   * The read mark (epoch millis). On the initial page (no `before`), the tail is
   * extended back to include every message newer than this — so the whole unread
   * run loads even when it exceeds `limit`, keeping the divider and count exact.
   */
  since?: number;
}

export type SuggestionsHandler = (suggestions: GroupSuggestions) => void;

/** A single AI persona acknowledging it successfully processed a message. */
export interface ReadReceipt {
  groupId: string;
  messageId: string;
  /** The AI persona that read (processed) the message. */
  personaId: string;
}

export type ReadHandler = (receipt: ReadReceipt) => void;

/** Turn activity: `true` when the agent loop starts a turn, `false` when idle. */
export type ActivityHandler = (active: boolean) => void;

/** Token usage for one LLM inference; absent for the mock brain. */
export interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  /** Prompt tokens served from the provider cache — the basis for cache savings. */
  cachedPromptTokens: number;
}

/**
 * A debug record of one agent inference: the exact system + context the
 * character's model received, what it decided, and the tokens it cost.
 */
export interface AgentTrace {
  ts: number;
  groupId: string;
  personaId: string;
  personaName: string;
  system: string;
  conversation: string;
  action: string;
  message?: string;
  mood?: string;
  usage?: TokenUsage | null;
}

export type DebugHandler = (trace: AgentTrace) => void;

/** An estimated cost breakdown; only present when the backend has pricing. */
export interface Cost {
  currency: string;
  input: number;
  cachedInput: number;
  output: number;
  total: number;
}

/** Cumulative LLM usage since the backend started — the "total usage" readout. */
export interface DebugUsage {
  requests: number;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cachedPromptTokens: number;
  /** Cached ÷ prompt tokens, `0..1`. */
  cacheHitRatio: number;
  estimatedCost?: Cost | null;
}

/**
 * What the data source offers. `mock` means no LLM and no persistence (the
 * in-memory build) — distinct from whether the backend is reachable. The
 * in-browser mock reports `mock: true` too.
 */
export interface ServerMeta {
  mock: boolean;
  llm: boolean;
  persistent: boolean;
  version?: string;
}

/**
 * The contract between the frontend and any AgoraLume message backend. The UI
 * depends only on this interface, never on a concrete transport, so chat runs
 * unchanged against the in-memory mock or a real HTTP backend.
 *
 * Workspace config (personas, organizations, groups, settings) lives in the
 * client-side workspace store, not here — so it stays editable and fully
 * offline. A real backend would later sync that store.
 */
export interface ChatApi {
  /**
   * Probes the data source: resolves its {@link ServerMeta} when reachable, or
   * `null` when unreachable. Combines liveness (null?) with mode (mock?), so the
   * UI can show "offline" and "mock" as separate facts. The in-browser mock
   * always resolves (never null).
   */
  probe(): Promise<ServerMeta | null>;

  /**
   * A page of message history, oldest first. With no options the full log is
   * returned; `limit` fetches the newest N (the initial tail); `before` walks
   * earlier pages from the oldest line already loaded.
   */
  listMessages(groupId: string, opts?: HistoryPage): Promise<Message[]>;

  /**
   * Sends a user message and resolves with the persisted message. `personaId`
   * is the active "you" identity to author it as; when omitted the backend
   * falls back to the group's stored self identity.
   */
  sendMessage(groupId: string, text: string, personaId?: string): Promise<Message>;

  /**
   * Resumes a turn suspended by a failed agent inference: the agents that have
   * not yet read the pending message respond to the current chat. A no-op when
   * nothing is suspended (e.g. the pending turn was voided by a newer message).
   * A no-op on the in-browser mock, whose rule brain never fails.
   */
  retry(groupId: string): Promise<void>;

  /**
   * Subscribes to messages the backend pushes for a group (AI replies, mood
   * updates). Returns an unsubscribe function. The HTTP implementation backs
   * this with Server-Sent Events.
   */
  subscribe(groupId: string, handler: MessageHandler): () => void;

  /**
   * Subscribes to read receipts: each event means one AI persona finished
   * processing a message (whether or not it chose to reply — agents may read
   * without replying). Returns an unsubscribe function.
   */
  subscribeReads(groupId: string, handler: ReadHandler): () => void;

  /**
   * Subscribes to the group's turn activity: `true` when the agent loop starts
   * a turn, `false` when it goes idle. The composer stays locked while active,
   * so a user message can never interleave with an in-flight turn. Returns an
   * unsubscribe function.
   */
  subscribeActivity(groupId: string, handler: ActivityHandler): () => void;

  /**
   * Subscribes to the group's current-turn snapshots: what triggered the round
   * (a user message, or an event with no message) and each AI member's progress
   * (pending / read / replied). The backend seeds the latest turn on connect and
   * pushes an update on every change, so the pinned progress bar reflects live
   * processing state independently of the loaded message window. Returns an
   * unsubscribe function.
   */
  subscribeTurn(groupId: string, handler: TurnHandler): () => void;

  /**
   * The group's cached conversation suggestions. Returns immediately from the
   * server-side cache; the backend regenerates in the background only when they
   * are stale (conversation advanced, or the time of day changed), pushing the
   * refreshed set on the stream's `suggestions` frame. The frontend only
   * fetches and displays — it never generates.
   */
  getSuggestions(groupId: string): Promise<GroupSuggestions>;

  /**
   * Asks the backend to regenerate this group's suggestions ("give me other
   * ideas"). Fire-and-forget: the fresh set arrives on the `suggestions` frame.
   * The backend rate-limits regeneration per group, so this can be spammed
   * safely. A no-op on the in-browser mock.
   */
  regenerateSuggestions(groupId: string): Promise<void>;

  /**
   * Subscribes to a group's suggestion updates — one frame whenever the backend
   * finishes a (re)generation. Returns an unsubscribe function. A no-op on the
   * in-browser mock.
   */
  subscribeSuggestions(groupId: string, handler: SuggestionsHandler): () => void;

  /** Cumulative LLM usage since the backend started (the debug panel's totals). */
  getUsage(): Promise<DebugUsage>;

  /**
   * Recent agent traces for a group — for hydrating the debug panel on open.
   * Live updates then arrive via {@link subscribeDebug}.
   */
  listTraces(groupId: string): Promise<AgentTrace[]>;

  /**
   * Subscribes to live debug traces for a group: one per agent inference, with
   * the prompt it saw, its decision, and token usage. Returns an unsubscribe
   * function. A no-op on the in-browser mock (which makes no LLM calls).
   */
  subscribeDebug(groupId: string, handler: DebugHandler): () => void;
}
