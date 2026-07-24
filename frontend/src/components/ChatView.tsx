import { Box, Center, Loader, ScrollArea, Stack, Text } from '@mantine/core';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '../lib/api';
import { useWorkspace } from '../store/workspace';
import type { Group, Message, Persona } from '../types';
import { Composer } from './Composer';
import { MessageItem } from './MessageItem';

interface Props {
  group: Group;
  personas: Map<string, Persona>;
}

export function ChatView({ group, personas }: Props) {
  const { t } = useTranslation();
  const fontSize = useWorkspace((s) => s.settings.chatFontSize ?? 15);

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
  const [awaitingReply, setAwaitingReply] = useState(false);
  // The message we're waiting on; the composer unlocks once every AI has read it.
  const [pendingId, setPendingId] = useState<string | null>(null);
  const viewport = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let active = true;
    setMessages(null);
    setAwaitingReply(false);
    setPendingId(null);

    void api.listMessages(group.id).then((initial) => {
      if (active) setMessages(initial);
    });

    const unsubscribe = api.subscribe(group.id, (message) => {
      setMessages((prev) => (prev ? [...prev, message] : [message]));
    });

    const unsubscribeReads = api.subscribeReads(group.id, (receipt) => {
      setMessages((prev) => {
        if (!prev) return prev;
        return prev.map((m) => {
          if (m.id !== receipt.messageId || m.kind !== 'conversation') return m;
          if (m.readBy?.includes(receipt.personaId)) return m;
          return { ...m, readBy: [...(m.readBy ?? []), receipt.personaId] };
        });
      });
    });

    return () => {
      active = false;
      unsubscribe();
      unsubscribeReads();
    };
  }, [group.id]);

  // Unlock the composer once all AI members have processed the pending message
  // (whether they replied or read without replying).
  useEffect(() => {
    if (!pendingId || !messages) return;
    const msg = messages.find((m) => m.id === pendingId);
    if (msg?.kind === 'conversation' && (msg.readBy?.length ?? 0) >= aiMemberCount) {
      setAwaitingReply(false);
      setPendingId(null);
    }
  }, [messages, pendingId, aiMemberCount]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: messages is the intended trigger — scroll to the bottom whenever the list changes.
  useEffect(() => {
    viewport.current?.scrollTo({ top: viewport.current.scrollHeight, behavior: 'smooth' });
  }, [messages]);

  const handleSend = async (text: string) => {
    if (locked || awaitingReply) return;
    setAwaitingReply(true);
    const message = await api.sendMessage(group.id, text, selfId);
    setMessages((prev) => (prev ? [...prev, message] : [message]));
    setPendingId(message.id);
  };

  // For each of your messages, which AI ids replied to it (until the next of yours).
  const replyMap = useMemo(() => {
    const map = new Map<string, Set<string>>();
    if (!messages) return map;
    const aiIdSet = new Set(aiMembers.map((p) => p.id));
    let anchor: string | null = null;
    for (const m of messages) {
      if (m.kind === 'conversation' && m.personaId === selfId) {
        anchor = m.id;
        map.set(anchor, new Set());
      } else if (m.kind === 'conversation' && anchor && aiIdSet.has(m.personaId)) {
        map.get(anchor)?.add(m.personaId);
      }
    }
    return map;
  }, [messages, selfId, aiMembers]);

  return (
    <Stack h="100%" gap={0}>
      <ScrollArea flex={1} viewportRef={viewport} p="md">
        {messages === null ? (
          <Center h={200}>
            <Loader />
          </Center>
        ) : messages.length === 0 ? (
          <Center h={200}>
            <Text c="dimmed">No messages yet — say hi 👋</Text>
          </Center>
        ) : (
          <Stack gap="md">
            {messages.map((message) => {
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
        <Composer
          onSend={handleSend}
          disabled={locked || awaitingReply}
          placeholder={locked ? t('chat.locked') : awaitingReply ? t('chat.waiting') : undefined}
        />
      </Box>
    </Stack>
  );
}
