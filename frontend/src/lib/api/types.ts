import type { Message } from '../../types';

export type MessageHandler = (message: Message) => void;

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

  listMessages(groupId: string): Promise<Message[]>;

  /**
   * Sends a user message and resolves with the persisted message. `personaId`
   * is the active "you" identity to author it as; when omitted the backend
   * falls back to the group's stored self identity.
   */
  sendMessage(groupId: string, text: string, personaId?: string): Promise<Message>;

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
}
