import { useWorkspace } from '../../store/workspace';
import type {
  ConversationMessage,
  GroupSuggestions,
  Message,
  Turn,
  TurnMember,
  TurnMemberState,
} from '../../types';
import type {
  ActivityHandler,
  AgentTrace,
  ChatApi,
  DebugHandler,
  DebugUsage,
  HistoryPage,
  MessageHandler,
  ReadHandler,
  ServerMeta,
  SuggestionsHandler,
  TurnHandler,
} from './types';
import { INITIAL_PAGE_CAP } from './types';

let seq = 0;
const nextId = () => `m${Date.now()}-${seq++}`;

const MOODS = ['🙂 pleased', '🤔 thinking', '✨ inspired', '😆 amused'];

/** Orders member states so a turn update only ever advances a member. */
const STATE_RANK: Record<TurnMemberState, number> = { pending: 0, read: 1, replied: 2 };

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
  private turnSubs = new Map<string, Set<TurnHandler>>();
  // The current (or most recent) turn per group — mirrors the backend's live turn
  // state that drives the pinned progress bar.
  private turns: Record<string, Turn> = {};

  async probe(): Promise<ServerMeta> {
    // The mock runs entirely in-browser — always reachable, always mock mode.
    return { mock: true, llm: false, persistent: false };
  }

  async listMessages(groupId: string, opts?: HistoryPage): Promise<Message[]> {
    // The log is kept oldest-first (seed order + appends), matching the server,
    // so history paging is plain index slicing. Return a copy so callers never
    // alias (and mutate) our internal array.
    const all = this.messages[groupId] ?? [];
    // "Load earlier": everything strictly before the cursor, capped to newest N.
    // An unknown cursor yields an empty page ("no more history").
    if (opts?.before) {
      const idx = all.findIndex((m) => m.id === opts.before);
      if (idx <= 0) return [];
      const slice = all.slice(0, idx);
      return opts.limit != null ? slice.slice(Math.max(0, slice.length - opts.limit)) : slice;
    }
    // Initial page: the newest `limit`, extended back to include everything newer
    // than `since` (the read mark) so the unread run is present — but capped at
    // INITIAL_CAP so a large backlog doesn't load whole (older lines page in).
    const byLimit = opts?.limit != null ? Math.max(0, all.length - opts.limit) : 0;
    const bySince =
      opts?.since != null
        ? (() => {
            const i = all.findIndex((m) => m.ts > (opts.since as number));
            return i === -1 ? all.length : i;
          })()
        : all.length;
    const byCap = Math.max(0, all.length - INITIAL_PAGE_CAP);
    return all.slice(Math.max(Math.min(byLimit, bySince), byCap));
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
    this.scheduleTurn(groupId, msg, text);
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

  subscribeTurn(groupId: string, handler: TurnHandler): () => void {
    const set = this.turnSubs.get(groupId) ?? new Set<TurnHandler>();
    set.add(handler);
    this.turnSubs.set(groupId, set);
    // Seed the just-subscribed handler with the current turn, matching the HTTP
    // backend's connect-time seed: the live turn if one is running/held, else one
    // reconstructed from the log (the last line "you" sent) so the bar shows up.
    const seed = this.turns[groupId] ?? this.reconstructTurn(groupId);
    if (seed) handler(seed);
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

  /** Stores a turn as the group's current one and notifies turn subscribers. */
  private emitTurn(groupId: string, turn: Turn): void {
    this.turns[groupId] = turn;
    for (const handler of this.turnSubs.get(groupId) ?? []) handler(turn);
  }

  /**
   * Advances one member's progress in the group's current turn (never walks it
   * back) and re-emits the snapshot — the mock counterpart to the backend's
   * per-member turn updates.
   */
  private updateTurnMember(
    groupId: string,
    personaId: string,
    state: TurnMemberState,
    replyId?: string,
  ): void {
    const turn = this.turns[groupId];
    if (!turn) return;
    const members = turn.members.map((m) => {
      if (m.personaId !== personaId || STATE_RANK[state] < STATE_RANK[m.state]) return m;
      return { ...m, state, replyId: m.replyId ?? replyId };
    });
    this.emitTurn(groupId, { ...turn, members });
  }

  /** Rebuilds a message-triggered turn from the log — the last line "you" sent,
   * plus who read/replied — so a fresh subscriber sees the bar even with no live
   * turn. Mirrors the backend's reconstruction. */
  private reconstructTurn(groupId: string): Turn | undefined {
    const all = this.messages[groupId] ?? [];
    const state = useWorkspace.getState();
    const group = state.groups.find((g) => g.id === groupId);
    if (!group) return undefined;
    const selfId = group.selfPersonaId || state.personas.find((p) => p.kind === 'user')?.id;
    if (!selfId) return undefined;
    const aiIds = group.personaIds.filter(
      (id) => state.personas.find((p) => p.id === id)?.kind === 'ai',
    );
    const trigger = lastSelfMessage(all, selfId);
    if (!trigger) return undefined;
    const replyOf = firstRepliesAfter(all, trigger.id, selfId, aiIds);
    const readBy = new Set(trigger.readBy ?? []);
    const members: TurnMember[] = aiIds.map((id) =>
      turnMember(id, replyOf.get(id), readBy.has(id)),
    );
    return {
      id: trigger.id,
      groupId,
      trigger: { kind: 'message', messageId: trigger.id, personaId: selfId, text: trigger.text },
      startedAt: trigger.ts,
      active: false,
      members,
    };
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
  private scheduleTurn(groupId: string, trigger: Message, userText: string): void {
    const state = useWorkspace.getState();
    const group = state.groups.find((g) => g.id === groupId);
    const aiIds = new Set(state.personas.filter((p) => p.kind === 'ai').map((p) => p.id));
    const readers = (group?.personaIds ?? []).filter((id) => aiIds.has(id));
    if (readers.length === 0) return;

    const replier = readers[Math.floor(Math.random() * readers.length)];
    const mood = MOODS[Math.floor(Math.random() * MOODS.length)];

    // Open the turn: every reader starts pending. Drives the pinned progress bar,
    // independently of the loaded message window.
    this.emitTurn(groupId, {
      id: `t${nextId()}`,
      groupId,
      trigger:
        trigger.kind === 'conversation'
          ? { kind: 'message', messageId: trigger.id, personaId: trigger.personaId, text: userText }
          : { kind: 'event', label: userText },
      startedAt: Date.now(),
      active: true,
      members: readers.map((id) => ({ personaId: id, state: 'pending' as TurnMemberState })),
    });

    // The loop is busy until the last reader finishes; the composer gates on it.
    this.setActive(groupId, true);
    const doneAt = Math.max(1100, 400 + (readers.length - 1) * 160) + 50;
    setTimeout(() => {
      this.setActive(groupId, false);
      // Mark the turn finished, keeping each member's final state.
      const turn = this.turns[groupId];
      if (turn) this.emitTurn(groupId, { ...turn, active: false });
    }, doneAt);

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
          const reply: Message = {
            id: nextId(),
            groupId,
            personaId: id,
            kind: 'conversation',
            text: mockReply(userText),
            ts: Date.now(),
          };
          this.emit(groupId, reply);
          this.markRead(groupId, trigger.id, id);
          this.updateTurnMember(groupId, id, 'replied', reply.id);
        }, 1100);
      } else {
        // Read-but-don't-reply: acknowledge processing without a message.
        setTimeout(
          () => {
            this.markRead(groupId, trigger.id, id);
            this.updateTurnMember(groupId, id, 'read');
          },
          400 + i * 160,
        );
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

/** The last line the given identity sent — the trigger a reconstructed turn pins. */
function lastSelfMessage(all: Message[], selfId: string): ConversationMessage | undefined {
  for (let i = all.length - 1; i >= 0; i--) {
    const m = all[i];
    if (m.kind === 'conversation' && m.personaId === selfId) return m;
  }
  return undefined;
}

/** Each AI member's first reply after `triggerId`, until the user speaks again. */
function firstRepliesAfter(
  all: Message[],
  triggerId: string,
  selfId: string,
  aiIds: string[],
): Map<string, string> {
  const replyOf = new Map<string, string>();
  const start = all.findIndex((m) => m.id === triggerId);
  if (start === -1) return replyOf;
  for (let i = start + 1; i < all.length; i++) {
    const m = all[i];
    if (m.kind !== 'conversation') continue;
    if (m.personaId === selfId) break;
    if (aiIds.includes(m.personaId) && !replyOf.has(m.personaId)) replyOf.set(m.personaId, m.id);
  }
  return replyOf;
}

/** One reconstructed member's progress: replied (with jump target) > read > pending. */
function turnMember(personaId: string, replyId: string | undefined, read: boolean): TurnMember {
  const state: TurnMemberState = replyId ? 'replied' : read ? 'read' : 'pending';
  return { personaId, state, replyId };
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
