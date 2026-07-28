import { Box, Group, Modal, Stack, Text, UnstyledButton } from '@mantine/core';
import { IconArrowBackUp } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import type { Persona } from '../types';
import { PersonaAvatar } from './PersonaAvatar';

/** Splits the AI audience into replied / read-but-silent / not-yet-read. */
export function classifyReaders(
  readBy: string[] | undefined,
  aiMembers: Persona[],
  repliedBy: string[] | undefined,
) {
  const readSet = new Set(readBy ?? []);
  const repliedSet = new Set(repliedBy ?? []);
  const replied = aiMembers.filter((p) => repliedSet.has(p.id));
  const readSilent = aiMembers.filter((p) => readSet.has(p.id) && !repliedSet.has(p.id));
  const unread = aiMembers.filter((p) => !readSet.has(p.id) && !repliedSet.has(p.id));
  return { readSet, repliedSet, replied, readSilent, unread };
}

interface Props {
  opened: boolean;
  onClose: () => void;
  readBy?: string[];
  aiMembers: Persona[];
  repliedBy?: string[];
  /** persona id → the message id of their reply. Rows with a target become jumpable. */
  replyTargets?: Map<string, string>;
  /** Scrolls the conversation to a reply. Given, replied rows jump (and close). */
  onJumpToMessage?: (id: string) => void;
}

/**
 * A persistent modal listing who read a given message — split into replied,
 * read-but-silent, and not-yet-read. A modal (rather than a hover card) so it
 * survives the chat scrolling while you compare names against who's in the room.
 * Shared by the per-message receipt badge and the pinned read-progress bar; when
 * reply targets are supplied, each replier's row jumps to their reply.
 */
export function ReadReceiptsModal({
  opened,
  onClose,
  readBy,
  aiMembers,
  repliedBy,
  replyTargets,
  onJumpToMessage,
}: Props) {
  const { t } = useTranslation();
  const { replied, readSilent, unread } = classifyReaders(readBy, aiMembers, repliedBy);
  // Jump then close, so the target line is what you land on.
  const jump = onJumpToMessage
    ? (id: string) => {
        onClose();
        onJumpToMessage(id);
      }
    : undefined;
  return (
    <Modal opened={opened} onClose={onClose} title={t('chat.readReceiptsTitle')} size="sm" centered>
      <Stack gap="sm">
        <ReaderGroup
          label={t('chat.readReplied')}
          list={replied}
          jumpTargets={replyTargets}
          onJump={jump}
        />
        <ReaderGroup label={t('chat.readSilent')} list={readSilent} />
        <ReaderGroup label={t('chat.readUnread')} list={unread} dim />
      </Stack>
    </Modal>
  );
}

interface ReaderGroupProps {
  label: string;
  list: Persona[];
  dim?: boolean;
  jumpTargets?: Map<string, string>;
  onJump?: (id: string) => void;
}

function ReaderGroup({ label, list, dim, jumpTargets, onJump }: ReaderGroupProps) {
  if (list.length === 0) return null;
  return (
    <Stack gap={4}>
      <Text size="xs" fw={700} c="dimmed">
        {label} · {list.length}
      </Text>
      {list.map((p) => (
        <ReaderRow
          key={p.id}
          persona={p}
          dim={dim}
          target={jumpTargets?.get(p.id)}
          onJump={onJump}
        />
      ))}
    </Stack>
  );
}

interface ReaderRowProps {
  persona: Persona;
  dim?: boolean;
  /** Reply message id to jump to, when this reader replied. */
  target?: string;
  onJump?: (id: string) => void;
}

/** One reader row. Jumpable (a button to their reply) when a target is given. */
function ReaderRow({ persona, dim, target, onJump }: ReaderRowProps) {
  const { t } = useTranslation();
  const jumpable = target !== undefined && onJump !== undefined;
  const content = (
    <Group gap={6} wrap="nowrap" style={{ opacity: dim ? 0.5 : 1 }}>
      <PersonaAvatar persona={persona} size={20} />
      <Text size="xs" truncate style={{ flex: 1, minWidth: 0 }}>
        {persona.name}
      </Text>
      {jumpable && <IconArrowBackUp size={14} color="var(--mantine-color-dimmed)" aria-hidden />}
    </Group>
  );
  if (!jumpable) return <Box>{content}</Box>;
  return (
    <UnstyledButton
      onClick={() => onJump(target)}
      aria-label={`${persona.name} · ${t('chat.jumpToReply')}`}
    >
      {content}
    </UnstyledButton>
  );
}
