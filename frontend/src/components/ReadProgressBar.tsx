import { Box, Group, Text, Tooltip, UnstyledButton } from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { IconArrowBackUp, IconChecks } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import type { Persona, Turn } from '../types';
import { PersonaAvatar } from './PersonaAvatar';
import { classifyReaders, ReadReceiptsModal } from './ReadReceiptsModal';

// How many reader avatars to show inline before collapsing the rest into a
// "+K" chip — keeps the bar a fixed, mobile-safe width however big the group is.
const MAX_AVATARS = 5;

interface Props {
  /**
   * The current processing round: what triggered it and how far each AI member
   * has got. Comes from live turn state (not the loaded message window), so the
   * bar shows regardless of how much history is loaded — and works for an event
   * trigger that has no user message.
   */
  turn: Turn;
  /** Resolves the turn's member (and trigger) persona ids to personas. */
  personas: Map<string, Persona>;
  /** Scrolls the conversation to a given line (a reply, or your own message). */
  onJumpToMessage: (id: string) => void;
}

/**
 * A slim bar pinned above the composer showing how far the group has got through
 * the current round — a processed count, and one avatar per member tinted by
 * state (replied / read-silently / still working). You never have to scroll back
 * to check progress. Clicking the count opens the full receipt list; clicking a
 * member who replied jumps to their reply; the return button (message triggers
 * only) jumps back to the line that started the round.
 *
 * It conceptually reflects the *agents' processing progress*, not a specific
 * message's read receipts — so a future event trigger, which has no user
 * message, still shows a round's progress here.
 */
export function ReadProgressBar({ turn, personas, onJumpToMessage }: Props) {
  const { t } = useTranslation();
  const [opened, { open, close }] = useDisclosure(false);

  // Resolve the turn's members to personas (in turn order), and derive the
  // read/replied sets the shared classifier + receipts modal already understand,
  // so the bar draws from live turn state while reusing the receipt UI unchanged.
  const aiMembers = turn.members
    .map((m) => personas.get(m.personaId))
    .filter((p): p is Persona => !!p);
  const readBy = turn.members.filter((m) => m.state !== 'pending').map((m) => m.personaId);
  const repliedBy = turn.members.filter((m) => m.state === 'replied').map((m) => m.personaId);
  const replyTargets = new Map(
    turn.members
      .filter((m): m is typeof m & { replyId: string } => m.replyId != null)
      .map((m) => [m.personaId, m.replyId]),
  );

  const { readSet, repliedSet, replied, readSilent, unread } = classifyReaders(
    readBy,
    aiMembers,
    repliedBy,
  );
  // Show the readers furthest along first (replied → read → pending), so the ones
  // that matter survive the cap; the rest collapse into a "+K" chip.
  const orderedReaders = [...replied, ...readSilent, ...unread];
  const shown = orderedReaders.slice(0, MAX_AVATARS);
  const overflow = orderedReaders.length - shown.length;

  // What the round is about, and — for a message trigger only — the line to jump
  // back to. An event trigger has just a label and nothing to jump to.
  const triggerText = turn.trigger.kind === 'message' ? turn.trigger.text : turn.trigger.label;
  const triggerMessageId = turn.trigger.kind === 'message' ? turn.trigger.messageId : undefined;

  return (
    <Box px="md" py={6} style={{ borderTop: '1px solid var(--mantine-color-default-border)' }}>
      <Box
        style={{ display: 'flex', alignItems: 'center', gap: 12, minWidth: 0, overflow: 'hidden' }}
      >
        <UnstyledButton
          onClick={open}
          aria-label={t('chat.readReceiptsTitle')}
          style={{ flexShrink: 0 }}
        >
          <Group gap="xs" wrap="nowrap" align="center" c="dimmed">
            <IconChecks size={15} />
            <Text size="xs" fw={600} style={{ whiteSpace: 'nowrap' }}>
              {t('chat.readProgress')}
            </Text>
            <Text size="xs" style={{ whiteSpace: 'nowrap' }}>
              {readSet.size}/{aiMembers.length}
            </Text>
          </Group>
        </UnstyledButton>

        <Group gap={2} wrap="nowrap" style={{ flexShrink: 0 }}>
          {shown.map((p) => {
            const replied = repliedSet.has(p.id);
            const read = readSet.has(p.id);
            const target = replyTargets.get(p.id);
            const status = replied
              ? t('chat.readReplied')
              : read
                ? t('chat.readSilent')
                : t('chat.readUnread');
            const avatar = (
              <Box
                style={{
                  borderRadius: '50%',
                  outline: replied ? `2px solid var(--mantine-color-${p.color}-5)` : 'none',
                  opacity: read ? 1 : 0.35,
                  filter: read ? 'none' : 'grayscale(0.6)',
                  transition: 'opacity 150ms ease',
                }}
              >
                <PersonaAvatar persona={p} size={20} />
              </Box>
            );
            return (
              <Tooltip
                key={p.id}
                label={`${p.name} · ${status}${target ? ` · ${t('chat.jumpToReply')}` : ''}`}
                withArrow
              >
                {target ? (
                  <UnstyledButton
                    onClick={() => onJumpToMessage(target)}
                    aria-label={`${p.name} · ${t('chat.jumpToReply')}`}
                    style={{ lineHeight: 0, borderRadius: '50%' }}
                  >
                    {avatar}
                  </UnstyledButton>
                ) : (
                  avatar
                )}
              </Tooltip>
            );
          })}
          {overflow > 0 && (
            <UnstyledButton
              onClick={open}
              aria-label={t('chat.readReceiptsTitle')}
              style={{
                width: 20,
                height: 20,
                borderRadius: '50%',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: 10,
                fontWeight: 700,
                color: 'var(--mantine-color-dimmed)',
                background: 'var(--mantine-color-default-hover)',
                border: '1px solid var(--mantine-color-default-border)',
              }}
            >
              +{overflow}
            </UnstyledButton>
          )}
        </Group>

        <Text size="xs" c="dimmed" truncate style={{ flex: 1, minWidth: 0 }} title={triggerText}>
          {triggerText}
        </Text>

        {triggerMessageId && (
          <Tooltip label={t('chat.jumpToMine')} withArrow>
            <UnstyledButton
              onClick={() => onJumpToMessage(triggerMessageId)}
              aria-label={t('chat.jumpToMine')}
              style={{ flexShrink: 0, color: 'var(--mantine-color-dimmed)', lineHeight: 0 }}
            >
              <IconArrowBackUp size={16} />
            </UnstyledButton>
          </Tooltip>
        )}
      </Box>

      <ReadReceiptsModal
        opened={opened}
        onClose={close}
        readBy={readBy}
        aiMembers={aiMembers}
        repliedBy={repliedBy}
        replyTargets={replyTargets}
        onJumpToMessage={onJumpToMessage}
      />
    </Box>
  );
}
