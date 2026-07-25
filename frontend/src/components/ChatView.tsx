import { Box, Center, Loader, ScrollArea, Stack, Text } from '@mantine/core';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '../lib/api';
import { useConnection } from '../store/connection';
import { useReadOnly } from '../store/readonly';
import { useWorkspace } from '../store/workspace';
import type { Group, Message, Persona } from '../types';
import { Composer } from './Composer';
import { MessageItem } from './MessageItem';

interface Props {
  group: Group;
  personas: Map<string, Persona>;
}

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

export function ChatView({ group, personas }: Props) {
  const { t } = useTranslation();
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
  const viewport = useRef<HTMLDivElement>(null);
  // Buffered read receipts, keyed by message id — decouples receipts from the
  // arrival of their target message so none are lost to the SSE/POST race.
  const reads = useRef<Map<string, Set<string>>>(new Map());

  // biome-ignore lint/correctness/useExhaustiveDependencies: backendUrl is an intentional trigger — a change re-runs this so history and subscriptions rebind to the newly selected data source.
  useEffect(() => {
    let active = true;
    setMessages(null);
    setBusy(false);
    reads.current = new Map();

    void api.listMessages(group.id).then((initial) => {
      if (active) setMessages(applyReads(initial, reads.current));
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
    };
  }, [group.id, backendUrl]);

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

  return (
    <Stack h="100%" gap={0}>
      <ScrollArea flex={1} viewportRef={viewport} p="md">
        {ordered === null ? (
          <Center h={200}>
            <Loader />
          </Center>
        ) : ordered.length === 0 ? (
          <Center h={200}>
            <Text c="dimmed">No messages yet — say hi 👋</Text>
          </Center>
        ) : (
          <Stack gap="md">
            {ordered.map((message) => {
              const persona = personas.get(message.personaId);
              if (!persona) return null;
              const replied = replyMap.get(message.id);
              return (
                <MessageItem
                  key={message.id}
                  message={message}
                  persona={persona}
                  isSelf={message.personaId === selfId}
                  fontSize={fontSize}
                  aiMembers={aiMembers}
                  repliedBy={replied ? [...replied] : undefined}
                />
              );
            })}
          </Stack>
        )}
      </ScrollArea>
      <Box p="md" style={{ borderTop: '1px solid var(--mantine-color-default-border)' }}>
        {readOnly ? (
          <Text size="sm" c="dimmed" ta="center">
            {t('readonly.chatNotice')}
          </Text>
        ) : (
          <Composer
            onSend={handleSend}
            disabled={locked || busy}
            placeholder={locked ? t('chat.locked') : busy ? t('chat.waiting') : undefined}
          />
        )}
      </Box>
    </Stack>
  );
}
