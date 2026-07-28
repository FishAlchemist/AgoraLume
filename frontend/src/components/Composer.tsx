import { ActionIcon, Group, Textarea } from '@mantine/core';
import { IconSend } from '@tabler/icons-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

interface Props {
  disabled?: boolean;
  /** Overrides the default placeholder (e.g. a locked/waiting reason). */
  placeholder?: string;
  onSend: (text: string) => void;
  /**
   * Text to load into the input without sending (a suggestion chip click). The
   * input state stays local — this only pushes a value in, so the user still
   * edits and presses send. Bump `nonce` to re-fill even with identical text.
   */
  fill?: { text: string; nonce: number };
}

export function Composer({ disabled, placeholder, onSend, fill }: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const ref = useRef<HTMLTextAreaElement>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: fill.nonce is the intended trigger — re-fill (and refocus) even when the text is unchanged; reading fill.text here is deliberate.
  useEffect(() => {
    if (!fill) return;
    setValue(fill.text);
    // Focus and drop the caret at the end so the filled text reads as a start to
    // continue from, not a finished draft to send. rAF waits for React to commit
    // the new value so the caret lands past the last character.
    requestAnimationFrame(() => {
      const el = ref.current;
      if (!el) return;
      el.focus();
      const end = el.value.length;
      el.setSelectionRange(end, end);
    });
  }, [fill?.nonce]);

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
        ref={ref}
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
