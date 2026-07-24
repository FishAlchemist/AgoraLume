import { ActionIcon, Group, Textarea } from '@mantine/core';
import { IconSend } from '@tabler/icons-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

interface Props {
  disabled?: boolean;
  /** Overrides the default placeholder (e.g. a locked/waiting reason). */
  placeholder?: string;
  onSend: (text: string) => void;
}

export function Composer({ disabled, placeholder, onSend }: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState('');

  const submit = () => {
    if (disabled) return;
    const text = value.trim();
    if (!text) return;
    onSend(text);
    setValue('');
  };

  return (
    <Group align="flex-end" gap="sm" wrap="nowrap">
      <Textarea
        flex={1}
        autosize
        minRows={1}
        maxRows={5}
        placeholder={placeholder ?? t('chat.placeholder')}
        value={value}
        disabled={disabled}
        onChange={(event) => setValue(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault();
            submit();
          }
        }}
      />
      <ActionIcon
        size="lg"
        radius="xl"
        variant="filled"
        aria-label={t('chat.send')}
        disabled={disabled}
        onClick={submit}
      >
        <IconSend size={18} />
      </ActionIcon>
    </Group>
  );
}
