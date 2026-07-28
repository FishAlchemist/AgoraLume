import { Badge, Button, Group, Paper, Stack, Text, UnstyledButton } from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { IconAlertTriangle, IconChecks, IconRefresh } from '@tabler/icons-react';
import { memo } from 'react';
import { useTranslation } from 'react-i18next';
import { useUi } from '../store/ui';
import type { Message, Persona, SystemMessage } from '../types';
import { PersonaAvatar } from './PersonaAvatar';
import { ReadReceiptsModal } from './ReadReceiptsModal';

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
  /** Resumes a suspended turn; only wired for a `system` error message. */
  onRetry?: () => void;
  /** Whether the retry button is actionable (this error is the latest, loop idle). */
  canRetry?: boolean;
}

/** Short clock time (e.g. "15:30") for the message header, in the UI locale. */
function formatTime(ts: number, locale: string): string {
  return new Date(ts).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
}

/**
 * A single chat line. Memoized because an active multi-AI turn streams replies in
 * quickly, re-rendering the whole list each time — with stable props, existing
 * bubbles (all AI lines, which carry no changing receipt data) skip re-rendering
 * and only genuinely-changed lines repaint.
 */
export const MessageItem = memo(function MessageItem({
  message,
  persona,
  isSelf,
  fontSize,
  aiMembers,
  repliedBy,
  onRetry,
  canRetry,
}: Props) {
  const { t, i18n } = useTranslation();
  const openCard = useUi((s) => s.openCard);
  // Short time shown inline; the full local date-time is on hover (title).
  const time = formatTime(message.ts, i18n.language);
  const fullTime = new Date(message.ts).toLocaleString(i18n.language);

  if (message.kind === 'system') {
    return (
      <SystemNotice message={message} persona={persona} onRetry={onRetry} canRetry={canRetry} />
    );
  }

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
          className="agora-msg-bubble"
          style={{
            background: isSelf
              ? 'linear-gradient(135deg, var(--mantine-color-indigo-6), var(--mantine-color-cyan-5))'
              : 'color-mix(in srgb, var(--mantine-color-body) 80%, transparent)',
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
});

interface SystemNoticeProps {
  message: SystemMessage;
  persona: Persona;
  onRetry?: () => void;
  canRetry?: boolean;
}

/**
 * A failed-inference notice: the persona that failed, the sanitized HTTP status
 * + reason (never the provider body), and a retry button that resumes the
 * suspended turn while this error is still the latest line.
 */
function SystemNotice({ message, persona, onRetry, canRetry }: SystemNoticeProps) {
  const { t, i18n } = useTranslation();
  const detail = message.status ? `HTTP ${message.status} · ${message.reason}` : message.reason;
  const fullTime = new Date(message.ts).toLocaleString(i18n.language);
  return (
    <Group justify="center" my={6}>
      <Paper
        px="md"
        py={8}
        radius="md"
        withBorder
        style={{
          borderColor: 'var(--mantine-color-red-4)',
          background: 'color-mix(in srgb, var(--mantine-color-red-6) 8%, transparent)',
          maxWidth: 'min(90%, 520px)',
        }}
      >
        <Group gap="sm" wrap="nowrap" align="center">
          <IconAlertTriangle size={18} color="var(--mantine-color-red-6)" aria-hidden />
          <Stack gap={0}>
            <Text size="sm" fw={600}>
              {t('chat.errorReplyFailed', { name: persona.name })}
            </Text>
            <Text size="xs" c="dimmed" title={fullTime}>
              {detail}
            </Text>
          </Stack>
          {onRetry && canRetry && (
            <Button
              size="xs"
              variant="light"
              color="red"
              leftSection={<IconRefresh size={14} />}
              onClick={onRetry}
            >
              {t('chat.retry')}
            </Button>
          )}
        </Group>
      </Paper>
    </Group>
  );
}

interface ReadReceiptsProps {
  readBy?: string[];
  aiMembers: Persona[];
  repliedBy?: string[];
}

/**
 * Read-receipt badge for your own message. Clicking it opens a persistent modal
 * listing who read it. The same modal is reused by the pinned read-progress bar.
 */
function ReadReceipts({ readBy, aiMembers, repliedBy }: ReadReceiptsProps) {
  const { t } = useTranslation();
  const [opened, { open, close }] = useDisclosure(false);
  const readCount = new Set(readBy ?? []).size;

  return (
    <>
      <UnstyledButton onClick={open} aria-label={t('chat.readReceiptsTitle')}>
        <Group gap={4} pr={6} wrap="nowrap" c="dimmed">
          <IconChecks size={13} />
          <Text size="xs">
            {t('chat.readCount', { count: readCount, total: aiMembers.length })}
          </Text>
        </Group>
      </UnstyledButton>
      <ReadReceiptsModal
        opened={opened}
        onClose={close}
        readBy={readBy}
        aiMembers={aiMembers}
        repliedBy={repliedBy}
      />
    </>
  );
}
