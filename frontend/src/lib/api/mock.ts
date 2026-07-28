import { useWorkspace } from '../../store/workspace';
import type { GroupSuggestions, Message } from '../../types';
import type {
  ActivityHandler,
  AgentTrace,
  ChatApi,
  DebugHandler,
  DebugUsage,
  MessageHandler,
  ReadHandler,
  ServerMeta,
  SuggestionsHandler,
} from './types';

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
  private activitySubs = new Map<string, Set<ActivityHandler>>();

  async probe(): Promise<ServerMeta> {
    // The mock runs entirely in-browser — always reachable, always mock mode.
    return { mock: true, llm: false, persistent: false };
  }

  async listMessages(groupId: string): Promise<Message[]> {
    // Return a copy so callers never alias (and mutate) our internal array.
    return [...(this.messages[groupId] ?? [])];
  }

  async sendMessage(groupId: string, text: string, personaId?: string): Promise<Message> {
    const state = useWorkspace.getState();
    const group = state.groups.find((g) => g.id === groupId);
    const selfId =
      personaId ||
      group?.selfPersonaId ||
      state.personas.find((p) => p.kind === 'user')?.id ||
      'user';
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

  // The rule brain never fails, so there is never a suspended turn to resume.
  async retry(_groupId: string): Promise<void> {}

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

  subscribeActivity(groupId: string, handler: ActivityHandler): () => void {
    const set = this.activitySubs.get(groupId) ?? new Set<ActivityHandler>();
    set.add(handler);
    this.activitySubs.set(groupId, set);
    return () => set.delete(handler);
  }

  // The in-browser mock makes no LLM calls, so there is nothing to report: zero
  // usage, no traces, and a debug subscription that never fires.
  async getUsage(): Promise<DebugUsage> {
    return {
      requests: 0,
      promptTokens: 0,
      completionTokens: 0,
      totalTokens: 0,
      cachedPromptTokens: 0,
      cacheHitRatio: 0,
    };
  }

  async listTraces(): Promise<AgentTrace[]> {
    return [];
  }

  subscribeDebug(_groupId: string, _handler: DebugHandler): () => void {
    return () => {};
  }

  // Canned, time-aware openers so the suggestion chips work with no backend.
  async getSuggestions(_groupId: string): Promise<GroupSuggestions> {
    return mockSuggestions();
  }

  // Nothing to regenerate in-browser — the openers are already time-aware.
  async regenerateSuggestions(_groupId: string): Promise<void> {}

  subscribeSuggestions(_groupId: string, _handler: SuggestionsHandler): () => void {
    return () => {};
  }

  private setActive(groupId: string, active: boolean): void {
    for (const handler of this.activitySubs.get(groupId) ?? []) handler(active);
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

    // The loop is busy until the last reader finishes; the composer gates on it.
    this.setActive(groupId, true);
    const doneAt = Math.max(1100, 400 + (readers.length - 1) * 160) + 50;
    setTimeout(() => this.setActive(groupId, false), doneAt);

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

/**
 * Time-aware conversation openers, mirroring the backend rule brain's canned
 * lines so the chips are usable offline. The part of day is bucketed the same
 * way the backend does (morning 5–10, afternoon 11–16, evening 17–21, else
 * night), so an evening opener isn't offered in the morning.
 */
function mockSuggestions(): GroupSuggestions {
  const hour = new Date().getHours();
  const { timeOfDay, prompts } =
    hour >= 5 && hour <= 10
      ? {
          timeOfDay: 'morning',
          prompts: [
            'Good morning! What is everyone up to today?',
            'Any plans for the morning?',
            "What's the first thing on your mind today?",
          ],
        }
      : hour >= 11 && hour <= 16
        ? {
            timeOfDay: 'afternoon',
            prompts: [
              'How is your afternoon going?',
              'What are you all working on right now?',
              'Anyone up for a quick chat?',
            ],
          }
        : hour >= 17 && hour <= 21
          ? {
              timeOfDay: 'evening',
              prompts: [
                "How was everyone's day?",
                'Any plans for tonight?',
                "What's the highlight of your day so far?",
              ],
            }
          : {
              timeOfDay: 'night',
              prompts: [
                'Still up? What is keeping you awake?',
                'How was your day overall?',
                'Any quiet thoughts before bed?',
              ],
            };
  return { prompts, generatedAt: Date.now(), timeOfDay };
}
