import { Box, Center, Loader, ScrollArea, Stack, Text } from '@mantine/core';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '../lib/api';
import { useConnection } from '../store/connection';
import { useReadOnly } from '../store/readonly';
import { useWorkspace } from '../store/workspace';
import type { Group, Message, Persona } from '../types';
import { Composer } from './Composer';
import { MessageItem } from './MessageItem';
import { SuggestionChips } from './SuggestionChips';

interface Props {
  group: Group;
  personas: Map<string, Persona>;
}

// A manual "suggest other ideas" always shows the loader for at least this long,
// so a fast backend swap reads as a real refresh rather than an unchanged flicker.
const REGEN_MIN_MS = 1000;
// If no fresh set arrives within this window — the server coalesced the request
// inside its cooldown, so no `suggestions` frame follows — restore what we cleared
// rather than leaving the user on a spinner forever.
const REGEN_TIMEOUT_MS = 8000;

/** Appends a streamed message, replacing any existing entry with the same id. */
function appendMessage(prev: Message[] | null, message: Message): Message[] {
  if (!prev) return [message];
  if (prev.some((m) => m.id === message.id)) {
    return prev.map((m) => (m.id === message.id ? message : m));
  }
  return [...prev, message];
}

/**
 * Merges buffered read receipts into whichever messages are present. A receipt
 * can arrive before its target message — the turn streams reads as soon as it
 * runs, which races the POST response that inserts the user's line, and the two
 * travel over separate connections — so every receipt is buffered and re-applied
 * on each update, and a late message still picks up the reads it already earned.
 */
function applyReads(messages: Message[] | null, reads: Map<string, Set<string>>): Message[] | null {
  if (!messages) return messages;
  return messages.map((m) => {
    if (m.kind !== 'conversation') return m;
    const seen = reads.get(m.id);
    if (!seen || seen.size === 0) return m;
    const merged = new Set(m.readBy ?? []);
    const before = merged.size;
    for (const id of seen) merged.add(id);
    return merged.size === before ? m : { ...m, readBy: [...merged] };
  });
}

/** The trailing sequence in an `m<ts>-<seq>` id — a process-monotonic counter. */
function seqOf(id: string): number {
  const n = Number(id.slice(id.lastIndexOf('-') + 1));
  return Number.isFinite(n) ? n : 0;
}

/**
 * Orders chat lines oldest-first. Streamed frames and the POST response race
 * across separate connections, so arrival order is unreliable; the server clock
 * (shared with the client) is authoritative. An instant mock brain can stamp
 * several messages in the same millisecond, so the monotonic id sequence breaks
 * ties — keeping the user's line ahead of the replies it triggered.
 */
function compareMessages(a: Message, b: Message): number {
  return a.ts - b.ts || seqOf(a.id) - seqOf(b.id);
}

/** Local midnight for a timestamp, so two messages compare equal iff same calendar day. */
function startOfDay(ts: number): number {
  const d = new Date(ts);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/**
 * A day-separator label for the divider between messages that cross midnight:
 * "Today"/"Yesterday" for the two most recent days, otherwise a full local date
 * (year dropped when it's the current year) so the reader can place each message.
 */
function formatDayLabel(ts: number, locale: string, t: (key: string) => string): string {
  const day = startOfDay(ts);
  const today = startOfDay(Date.now());
  const diffDays = Math.round((today - day) / 86_400_000);
  if (diffDays === 0) return t('chat.today');
  if (diffDays === 1) return t('chat.yesterday');
  const d = new Date(ts);
  return d.toLocaleDateString(locale, {
    year: d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
    month: 'long',
    day: 'numeric',
    weekday: 'long',
  });
}

export function ChatView({ group, personas }: Props) {
  const { t, i18n } = useTranslation();
  const fontSize = useWorkspace((s) => s.settings.chatFontSize ?? 15);
  const readOnly = useReadOnly((s) => s.readOnly);
  // Re-bind history + streams when the active data source changes.
  const backendUrl = useConnection((s) => s.backendUrl);

  // Fall back to any user identity so a group without an explicit self still works.
  const selfId = group.selfPersonaId || [...personas.values()].find((p) => p.kind === 'user')?.id;
  // AI members drive read receipts, replies, and the "locked" state.
  const aiMembers = useMemo(
    () =>
      group.personaIds
        .map((id) => personas.get(id))
        .filter((p): p is Persona => !!p && p.kind === 'ai'),
    [group.personaIds, personas],
  );
  const aiMemberCount = aiMembers.length;
  const locked = aiMemberCount === 0;

  const [messages, setMessages] = useState<Message[] | null>(null);
  // The agent loop's busy/idle state, driven by the backend's activity signal.
  // The composer stays locked while busy, so a message can't interleave a turn.
  const [busy, setBusy] = useState(false);
  // Server-generated conversation openers; the frontend only fetches & displays.
  const [suggestions, setSuggestions] = useState<string[]>([]);
  // A chip click pushes its text into the composer (never sends). The nonce lets
  // the same suggestion re-fill after it was edited or cleared.
  const [fill, setFill] = useState<{ text: string; nonce: number }>();
  // True while an explicit "suggest other ideas" is in flight: the old chips are
  // cleared and a loader shows until the fresh set arrives (min REGEN_MIN_MS).
  const [regenerating, setRegenerating] = useState(false);
  // The in-flight manual regenerate: when it started, the openers to restore if it
  // yields nothing, and its safety timer. null when no manual refresh is pending.
  const regen = useRef<{ startedAt: number; prev: string[]; timer: number } | null>(null);
  const viewport = useRef<HTMLDivElement>(null);
  // Buffered read receipts, keyed by message id — decouples receipts from the
  // arrival of their target message so none are lost to the SSE/POST race.
  const reads = useRef<Map<string, Set<string>>>(new Map());

  // Ends an in-flight manual regenerate: cancels its safety timer and drops the
  // loader. Leaves whatever suggestions are currently shown in place.
  const finishRegen = useCallback(() => {
    const r = regen.current;
    if (r) window.clearTimeout(r.timer);
    regen.current = null;
    setRegenerating(false);
  }, []);

  // Applies a freshly-arrived suggestion set (delivered on the SSE frame). When a
  // manual refresh is pending, hold the loader for at least REGEN_MIN_MS so the
  // swap is perceptible; otherwise (open/turn-end regen) apply it immediately.
  const applySuggestions = useCallback(
    (prompts: string[]) => {
      const r = regen.current;
      if (!r) {
        setSuggestions(prompts);
        return;
      }
      const wait = Math.max(0, REGEN_MIN_MS - (Date.now() - r.startedAt));
      window.setTimeout(() => {
        setSuggestions(prompts);
        finishRegen();
      }, wait);
    },
    [finishRegen],
  );

  // The refresh button: clear the current chips, show the loader, and ask the
  // backend for a new set (rate-limited server-side). The fresh set arrives on the
  // `suggestions` frame; a safety timer restores the old chips if none does.
  const handleRegenerate = useCallback(() => {
    if (regen.current) return; // one already pending
    const timer = window.setTimeout(() => {
      const r = regen.current;
      if (r) setSuggestions(r.prev);
      finishRegen();
    }, REGEN_TIMEOUT_MS);
    regen.current = { startedAt: Date.now(), prev: suggestions, timer };
    setRegenerating(true);
    setSuggestions([]);
    void api.regenerateSuggestions(group.id).catch(() => {
      const r = regen.current;
      if (r) setSuggestions(r.prev);
      finishRegen();
    });
  }, [suggestions, group.id, finishRegen]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: backendUrl is an intentional trigger — a change re-runs this so history and subscriptions rebind to the newly selected data source.
  useEffect(() => {
    let active = true;
    setMessages(null);
    setBusy(false);
    setSuggestions([]);
    finishRegen();
    reads.current = new Map();

    void api.listMessages(group.id).then((initial) => {
      if (active) setMessages(applyReads(initial, reads.current));
    });

    // Fetch the cached openers on open; a stale set kicks a background regen on
    // the backend, whose result then arrives on the `suggestions` frame below.
    void api.getSuggestions(group.id).then((s) => {
      if (active) setSuggestions(s.prompts);
    });

    const unsubscribeSuggestions = api.subscribeSuggestions(group.id, (s) => {
      applySuggestions(s.prompts);
    });

    const unsubscribe = api.subscribe(group.id, (message) => {
      setMessages((prev) => applyReads(appendMessage(prev, message), reads.current));
    });

    const unsubscribeReads = api.subscribeReads(group.id, (receipt) => {
      // Buffer first, then re-apply — a receipt that beats its message still lands.
      const seen = reads.current.get(receipt.messageId) ?? new Set<string>();
      seen.add(receipt.personaId);
      reads.current.set(receipt.messageId, seen);
      setMessages((prev) => applyReads(prev, reads.current));
    });

    // The composer unlocks only when the whole agent loop reports idle.
    const unsubscribeActivity = api.subscribeActivity(group.id, setBusy);

    return () => {
      active = false;
      unsubscribe();
      unsubscribeReads();
      unsubscribeActivity();
      unsubscribeSuggestions();
      finishRegen();
    };
  }, [group.id, backendUrl]);

  // When a turn ends (busy → idle) the conversation has advanced, so the openers
  // are stale. Re-fetch to nudge the backend into regenerating for the new state;
  // the fresh set streams back on the `suggestions` frame.
  const wasBusy = useRef(false);
  useEffect(() => {
    if (wasBusy.current && !busy) {
      void api
        .getSuggestions(group.id)
        .then((s) => setSuggestions(s.prompts))
        .catch(() => {});
    }
    wasBusy.current = busy;
  }, [busy, group.id]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: messages is the intended trigger — scroll to the bottom whenever the list changes.
  useEffect(() => {
    viewport.current?.scrollTo({ top: viewport.current.scrollHeight, behavior: 'smooth' });
  }, [messages]);

  const handleSend = async (text: string) => {
    if (locked || busy) return;
    // Lock immediately; the backend's idle activity signal clears it once the
    // whole turn is done. On failure, unlock so the user can retry.
    setBusy(true);
    try {
      const message = await api.sendMessage(group.id, text, selfId);
      setMessages((prev) => applyReads(appendMessage(prev, message), reads.current));
    } catch {
      setBusy(false);
    }
  };

  // Resumes a turn suspended by a failed agent. We don't lock optimistically:
  // the backend's `activity` signal locks the composer only if there is actually
  // something to resume, so a stale retry (already voided) can't wedge the UI.
  const handleRetry = () => {
    if (busy) return;
    void api.retry(group.id).catch(() => {});
  };

  const ordered = useMemo(
    () => (messages ? [...messages].sort(compareMessages) : messages),
    [messages],
  );

  // For each of your messages, which AI ids replied to it (until the next of yours).
  const replyMap = useMemo(() => {
    const map = new Map<string, Set<string>>();
    if (!ordered) return map;
    const aiIdSet = new Set(aiMembers.map((p) => p.id));
    let anchor: string | null = null;
    for (const m of ordered) {
      if (m.kind === 'conversation' && m.personaId === selfId) {
        anchor = m.id;
        map.set(anchor, new Set());
      } else if (m.kind === 'conversation' && anchor && aiIdSet.has(m.personaId)) {
        map.get(anchor)?.add(m.personaId);
      }
    }
    return map;
  }, [ordered, selfId, aiMembers]);

  // Bucket the ordered lines into consecutive same-day runs. Each run renders as
  // its own block with a sticky date header, so the day stays pinned at the top
  // of the viewport the whole time you're reading that day — and is handed off to
  // the next day only once you scroll its block away.
  const dayGroups = useMemo(() => {
    if (!ordered) return null;
    const groups: { day: number; ts: number; items: Message[] }[] = [];
    for (const m of ordered) {
      const day = startOfDay(m.ts);
      const last = groups.at(-1);
      if (last && last.day === day) last.items.push(m);
      else groups.push({ day, ts: m.ts, items: [m] });
    }
    return groups;
  }, [ordered]);
  // The id of the very last line overall — the only one that may offer a retry.
  const lastId = ordered?.at(-1)?.id;

  return (
    <Stack h="100%" gap={0}>
      <ScrollArea flex={1} viewportRef={viewport} p="md">
        {dayGroups === null ? (
          <Center h={200}>
            <Loader />
          </Center>
        ) : dayGroups.length === 0 ? (
          <Center h={200}>
            <Text c="dimmed">No messages yet — say hi 👋</Text>
          </Center>
        ) : (
          <Stack gap="md">
            {dayGroups.map((group) => (
              <Stack key={group.day} gap="md" pos="relative">
                <DayHeader label={formatDayLabel(group.ts, i18n.language, t)} />
                {group.items.map((message) => (
                  <ChatMessage
                    key={message.id}
                    message={message}
                    persona={personas.get(message.personaId)}
                    selfId={selfId}
                    fontSize={fontSize}
                    aiMembers={aiMembers}
                    repliedBy={replyMap.get(message.id)}
                    canRetry={message.id === lastId && !busy}
                    onRetry={handleRetry}
                  />
                ))}
              </Stack>
            ))}
          </Stack>
        )}
      </ScrollArea>
      <Box p="md" style={{ borderTop: '1px solid var(--mantine-color-default-border)' }}>
        {readOnly ? (
          <Text size="sm" c="dimmed" ta="center">
            {t('readonly.chatNotice')}
          </Text>
        ) : (
          <>
            {!locked && !busy && (regenerating || suggestions.length > 0) && (
              <SuggestionChips
                prompts={suggestions}
                loading={regenerating}
                onPick={(text) => setFill({ text, nonce: Date.now() })}
                onRegenerate={handleRegenerate}
              />
            )}
            <Composer
              onSend={handleSend}
              disabled={locked || busy}
              placeholder={locked ? t('chat.locked') : busy ? t('chat.waiting') : undefined}
              fill={fill}
            />
          </>
        )}
      </Box>
    </Stack>
  );
}

interface ChatMessageProps {
  message: Message;
  persona: Persona | undefined;
  selfId: string | undefined;
  fontSize: number;
  aiMembers: Persona[];
  repliedBy: Set<string> | undefined;
  /** This is the latest line and the loop is idle — a `system` error may retry. */
  canRetry: boolean;
  onRetry: () => void;
}

/** A single chat line: resolves its persona and derives the display-only props. */
function ChatMessage({
  message,
  persona,
  selfId,
  fontSize,
  aiMembers,
  repliedBy,
  canRetry,
  onRetry,
}: ChatMessageProps) {
  if (!persona) return null;
  return (
    <MessageItem
      message={message}
      persona={persona}
      isSelf={message.personaId === selfId}
      fontSize={fontSize}
      aiMembers={aiMembers}
      repliedBy={repliedBy ? [...repliedBy] : undefined}
      onRetry={message.kind === 'system' ? onRetry : undefined}
      canRetry={canRetry}
    />
  );
}

/**
 * The date label for a day's block. It sticks to the top of the scroll viewport
 * so the day you're reading stays visible the whole time, and is pushed off only
 * when its block scrolls away and the next day's header takes over. `pointer-events`
 * is off so the floating pill never swallows clicks on the message beneath it.
 */
function DayHeader({ label }: { label: string }) {
  return (
    <Center pos="sticky" top={4} style={{ zIndex: 2, pointerEvents: 'none' }}>
      <Box
        style={{
          fontSize: 12,
          fontWeight: 600,
          color: 'var(--mantine-color-dimmed)',
          background: 'color-mix(in srgb, var(--mantine-color-body) 82%, transparent)',
          backdropFilter: 'blur(6px)',
          border: '1px solid var(--mantine-color-default-border)',
          borderRadius: 999,
          padding: '2px 12px',
          boxShadow: '0 2px 8px rgba(0, 0, 0, 0.08)',
        }}
      >
        {label}
      </Box>
    </Center>
  );
}
