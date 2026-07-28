import { ActionIcon, Button, Group, Tooltip } from '@mantine/core';
import { IconRefresh } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';

interface Props {
  prompts: string[];
  /** Loads the chip's text into the composer — it is never sent directly. */
  onPick: (text: string) => void;
  /** Asks the backend for a fresh set (rate-limited server-side). */
  onRegenerate: () => void;
}

/**
 * Conversation-starter chips shown above the composer for a user who isn't sure
 * what to say. Clicking a chip only fills the input (the user still presses
 * send); the refresh button requests a new set from the backend.
 */
export function SuggestionChips({ prompts, onPick, onRegenerate }: Props) {
  const { t } = useTranslation();
  return (
    <Group gap="xs" mb="xs" wrap="wrap" align="center">
      <Tooltip label={t('chat.suggestRegenerate')} withArrow>
        <ActionIcon
          size="sm"
          radius="xl"
          variant="subtle"
          aria-label={t('chat.suggestRegenerate')}
          onClick={onRegenerate}
        >
          <IconRefresh size={14} />
        </ActionIcon>
      </Tooltip>
      {prompts.map((prompt) => (
        <Button
          key={prompt}
          size="xs"
          radius="xl"
          variant="light"
          onClick={() => onPick(prompt)}
          styles={{ label: { whiteSpace: 'normal' } }}
        >
          {prompt}
        </Button>
      ))}
    </Group>
  );
}
