import { Badge, Group, HoverCard, Paper, Stack, Text } from '@mantine/core';
import { IconChecks } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import { useUi } from '../store/ui';
import type { Message, Persona } from '../types';
import { PersonaAvatar } from './PersonaAvatar';

interface Props {
  message: Message;
  persona: Persona;
  isSelf: boolean;
  /** Chat font size (px) from settings, applied to message + mood text. */
  fontSize: number;
  /** The group's AI members — the audience for read receipts. */
  aiMembers: Persona[];
  /** AI ids that replied to this message (a subset of those who read it). */
  repliedBy?: string[];
}

/** Short clock time (e.g. "15:30") for the message header, in the UI locale. */
function formatTime(ts: number, locale: string): string {
  return new Date(ts).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
}

export function MessageItem({ message, persona, isSelf, fontSize, aiMembers, repliedBy }: Props) {
  const { t, i18n } = useTranslation();
  const openCard = useUi((s) => s.openCard);
  // Short time shown inline; the full local date-time is on hover (title).
  const time = formatTime(message.ts, i18n.language);
  const fullTime = new Date(message.ts).toLocaleString(i18n.language);

  if (message.kind === 'mood') {
    return (
      <Group justify="center" my={6}>
        <Badge
          size="lg"
          radius="xl"
          variant="gradient"
          gradient={{ from: persona.color, to: 'indigo', deg: 135 }}
          leftSection={<span aria-hidden>✨</span>}
          style={{
            boxShadow: '0 4px 12px rgba(0, 0, 0, 0.12)',
            textTransform: 'none',
            fontSize: `${fontSize - 2}px`,
            height: 'auto',
            paddingBlock: 4,
          }}
        >
          {persona.name} · {message.mood}
          {message.note ? ` — ${message.note}` : ''}
        </Badge>
      </Group>
    );
  }

  return (
    <Group align="flex-start" justify={isSelf ? 'flex-end' : 'flex-start'} wrap="nowrap" gap="sm">
      {!isSelf && <PersonaAvatar persona={persona} onClick={() => openCard(persona.id)} />}
      <Stack gap={3} maw="min(74%, 560px)" align={isSelf ? 'flex-end' : 'flex-start'}>
        {!isSelf && (
          <Group gap={6} pl={6} wrap="nowrap">
            <Text size="xs" c="dimmed" fw={700}>
              {persona.name}
            </Text>
            {persona.kind === 'ai' && (
              <Badge size="xs" variant="light" color={persona.color} radius="sm">
                {t('chat.aiTag')}
              </Badge>
            )}
            <Text size="xs" c="dimmed" title={fullTime}>
              {time}
            </Text>
          </Group>
        )}
        <Paper
          px="md"
          py={9}
          radius="lg"
          withBorder={!isSelf}
          c={isSelf ? 'white' : undefined}
          style={{
            background: isSelf
              ? 'linear-gradient(135deg, var(--mantine-color-indigo-6), var(--mantine-color-cyan-5))'
              : 'color-mix(in srgb, var(--mantine-color-body) 80%, transparent)',
            backdropFilter: 'blur(6px)',
            boxShadow: '0 6px 18px rgba(0, 0, 0, 0.10)',
            borderColor: isSelf
              ? undefined
              : `color-mix(in srgb, var(--mantine-color-${persona.color}-4) 45%, transparent)`,
          }}
        >
          <Text style={{ whiteSpace: 'pre-wrap', fontSize: `${fontSize}px` }}>{message.text}</Text>
        </Paper>
        {isSelf && (
          <Group gap={8} pr={6} wrap="nowrap" align="center">
            <Text size="xs" c="dimmed" title={fullTime}>
              {time}
            </Text>
            {message.kind === 'conversation' && aiMembers.length > 0 && (
              <ReadReceipts readBy={message.readBy} aiMembers={aiMembers} repliedBy={repliedBy} />
            )}
          </Group>
        )}
      </Stack>
      {isSelf && <PersonaAvatar persona={persona} onClick={() => openCard(persona.id)} />}
    </Group>
  );
}

interface ReadReceiptsProps {
  readBy?: string[];
  aiMembers: Persona[];
  repliedBy?: string[];
}

/** Read-receipt badge for your own message, with a hover list of who read it. */
function ReadReceipts({ readBy, aiMembers, repliedBy }: ReadReceiptsProps) {
  const { t } = useTranslation();
  const readSet = new Set(readBy ?? []);
  const repliedSet = new Set(repliedBy ?? []);
  const replied = aiMembers.filter((p) => repliedSet.has(p.id));
  const readSilent = aiMembers.filter((p) => readSet.has(p.id) && !repliedSet.has(p.id));
  const unread = aiMembers.filter((p) => !readSet.has(p.id) && !repliedSet.has(p.id));

  return (
    <HoverCard width={230} position="top-end" withArrow shadow="md" openDelay={120}>
      <HoverCard.Target>
        <Group gap={4} pr={6} wrap="nowrap" c="dimmed" style={{ cursor: 'default' }}>
          <IconChecks size={13} />
          <Text size="xs">
            {t('chat.readCount', { count: readSet.size, total: aiMembers.length })}
          </Text>
        </Group>
      </HoverCard.Target>
      <HoverCard.Dropdown>
        <Stack gap="sm">
          <ReaderGroup label={t('chat.readReplied')} list={replied} />
          <ReaderGroup label={t('chat.readSilent')} list={readSilent} />
          <ReaderGroup label={t('chat.readUnread')} list={unread} dim />
        </Stack>
      </HoverCard.Dropdown>
    </HoverCard>
  );
}

function ReaderGroup({ label, list, dim }: { label: string; list: Persona[]; dim?: boolean }) {
  if (list.length === 0) return null;
  return (
    <Stack gap={4}>
      <Text size="xs" fw={700} c="dimmed">
        {label} · {list.length}
      </Text>
      {list.map((p) => (
        <Group key={p.id} gap={6} wrap="nowrap" style={{ opacity: dim ? 0.5 : 1 }}>
          <PersonaAvatar persona={p} size={20} />
          <Text size="xs" truncate>
            {p.name}
          </Text>
        </Group>
      ))}
    </Stack>
  );
}
