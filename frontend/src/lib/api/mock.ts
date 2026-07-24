import { useWorkspace } from '../../store/workspace';
import type { Message } from '../../types';
import type { ChatApi, MessageHandler, ReadHandler } from './types';

let seq = 0;
const nextId = () => `m${Date.now()}-${seq++}`;

const MOODS = ['🙂 pleased', '🤔 thinking', '✨ inspired', '😆 amused'];

/**
 * A self-contained backend that lives entirely in the browser. It lets the
 * whole UI be developed, demoed, and deployed with no server at all. Personas
 * and groups are read live from the workspace store, so edits take effect
 * immediately.
 */
export class MockChatApi implements ChatApi {
  private messages: Record<string, Message[]> = seed();
  private subs = new Map<string, Set<MessageHandler>>();
  private readSubs = new Map<string, Set<ReadHandler>>();

  async listMessages(groupId: string): Promise<Message[]> {
    // Return a copy so callers never alias (and mutate) our internal array.
    return [...(this.messages[groupId] ?? [])];
  }

  async sendMessage(groupId: string, text: string): Promise<Message> {
    const state = useWorkspace.getState();
    const group = state.groups.find((g) => g.id === groupId);
    const selfId =
      group?.selfPersonaId || state.personas.find((p) => p.kind === 'user')?.id || 'user';
    const msg: Message = {
      id: nextId(),
      groupId,
      personaId: selfId,
      kind: 'conversation',
      text,
      ts: Date.now(),
      readBy: [],
    };
    this.messages[groupId] ??= [];
    this.messages[groupId].push(msg);
    this.scheduleTurn(groupId, msg.id, text);
    return msg;
  }

  subscribe(groupId: string, handler: MessageHandler): () => void {
    const set = this.subs.get(groupId) ?? new Set<MessageHandler>();
    set.add(handler);
    this.subs.set(groupId, set);
    return () => set.delete(handler);
  }

  subscribeReads(groupId: string, handler: ReadHandler): () => void {
    const set = this.readSubs.get(groupId) ?? new Set<ReadHandler>();
    set.add(handler);
    this.readSubs.set(groupId, set);
    return () => set.delete(handler);
  }

  private emit(groupId: string, message: Message): void {
    this.messages[groupId] ??= [];
    this.messages[groupId].push(message);
    for (const handler of this.subs.get(groupId) ?? []) handler(message);
  }

  /** Marks a message read by one AI persona (deduped) and notifies listeners. */
  private markRead(groupId: string, messageId: string, personaId: string): void {
    const msg = this.messages[groupId]?.find((m) => m.id === messageId);
    if (msg?.kind === 'conversation') {
      const set = new Set(msg.readBy ?? []);
      if (set.has(personaId)) return;
      set.add(personaId);
      msg.readBy = [...set];
    }
    for (const handler of this.readSubs.get(groupId) ?? []) {
      handler({ groupId, messageId, personaId });
    }
  }

  /**
   * Simulates the agents' turn: every AI member reads (processes) the message,
   * but only a random one replies — the rest read without replying. Each
   * persona's read receipt fires when its turn finishes, so "all read" lines up
   * with "everyone is done".
   */
  private scheduleTurn(groupId: string, messageId: string, userText: string): void {
    const state = useWorkspace.getState();
    const group = state.groups.find((g) => g.id === groupId);
    const aiIds = new Set(state.personas.filter((p) => p.kind === 'ai').map((p) => p.id));
    const readers = (group?.personaIds ?? []).filter((id) => aiIds.has(id));
    if (readers.length === 0) return;

    const replier = readers[Math.floor(Math.random() * readers.length)];
    const mood = MOODS[Math.floor(Math.random() * MOODS.length)];

    let i = 0;
    for (const id of readers) {
      if (id === replier) {
        setTimeout(() => {
          this.emit(groupId, {
            id: nextId(),
            groupId,
            personaId: id,
            kind: 'mood',
            mood,
            ts: Date.now(),
          });
        }, 500);
        setTimeout(() => {
          this.emit(groupId, {
            id: nextId(),
            groupId,
            personaId: id,
            kind: 'conversation',
            text: mockReply(userText),
            ts: Date.now(),
          });
          this.markRead(groupId, messageId, id);
        }, 1100);
      } else {
        // Read-but-don't-reply: acknowledge processing without a message.
        setTimeout(() => this.markRead(groupId, messageId, id), 400 + i * 160);
      }
      i += 1;
    }
  }
}

function seed(): Record<string, Message[]> {
  const now = Date.now();
  return {
    lounge: [
      {
        id: nextId(),
        groupId: 'lounge',
        personaId: 'aria',
        kind: 'mood',
        mood: '😌 relaxed',
        note: 'settling into the lounge',
        ts: now - 60_000,
      },
      {
        id: nextId(),
        groupId: 'lounge',
        personaId: 'aria',
        kind: 'conversation',
        text: 'Welcome to AgoraLume! Ask us anything — Nox and Sol are here too.',
        ts: now - 55_000,
      },
      {
        id: nextId(),
        groupId: 'lounge',
        personaId: 'nox',
        kind: 'conversation',
        text: 'A multi-persona group chat. Efficient. I approve.',
        ts: now - 50_000,
      },
    ],
    lab: [
      {
        id: nextId(),
        groupId: 'lab',
        personaId: 'nox',
        kind: 'mood',
        mood: '🤔 focused',
        ts: now - 40_000,
      },
    ],
  };
}

function mockReply(userText: string): string {
  const text = userText.trim();
  if (!text) return 'Hmm?';
  return `You said “${text}”. (Mock reply — set VITE_API_BASE_URL to talk to a real backend.)`;
}
