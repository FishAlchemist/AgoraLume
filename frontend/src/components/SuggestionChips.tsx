import { ActionIcon, Button, Group, Text, Tooltip } from '@mantine/core';
import { IconRefresh } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';

interface Props {
  prompts: string[];
  /** While true, the old chips are cleared and a loader shows in their place. */
  loading?: boolean;
  /** Loads the chip's text into the composer — it is never sent directly. */
  onPick: (text: string) => void;
  /** Asks the backend for a fresh set (rate-limited server-side). */
  onRegenerate: () => void;
}

/**
 * Conversation-starter chips shown above the composer for a user who isn't sure
 * what to say. Clicking a chip only fills the input (the user still presses
 * send); the refresh button requests a new set from the backend. While a refresh
 * is in flight the chips give way to a spinner so the swap is visible.
 */
export function SuggestionChips({ prompts, loading, onPick, onRegenerate }: Props) {
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
          loading={loading}
          disabled={loading}
        >
          <IconRefresh size={14} />
        </ActionIcon>
      </Tooltip>
      {loading ? (
        <Text size="xs" c="dimmed">
          {t('chat.suggestLoading')}
        </Text>
      ) : (
        prompts.map((prompt) => (
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
        ))
      )}
    </Group>
  );
}
