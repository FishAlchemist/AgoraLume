import { Button, FileButton, Group, Stack, Text } from '@mantine/core';
import { IconTrash, IconUpload } from '@tabler/icons-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AVATAR_MAX_INPUT_BYTES,
  type AvatarError,
  AvatarProcessingError,
  fileToAvatarDataUrl,
} from '../lib/image';
import type { Persona } from '../types';
import { PersonaAvatar } from './PersonaAvatar';

interface Props {
  value?: string;
  onChange: (next: string | undefined) => void;
  preview: Pick<Persona, 'name' | 'emoji' | 'gradient' | 'color'>;
  disabled?: boolean;
}

const ERROR_KEY: Record<AvatarError, string> = {
  'not-image': 'avatar.errorNotImage',
  'too-large': 'avatar.errorTooLarge',
  'decode-failed': 'avatar.errorDecode',
};

/** Uploads, size-limits, and downscales a persona avatar to a compact image. */
export function AvatarUpload({ value, onChange, preview, disabled }: Props) {
  const { t } = useTranslation();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const handleFile = async (file: File | null) => {
    if (!file) return;
    setError(null);
    setBusy(true);
    try {
      onChange(await fileToAvatarDataUrl(file));
    } catch (e) {
      const reason: AvatarError = e instanceof AvatarProcessingError ? e.reason : 'decode-failed';
      setError(t(ERROR_KEY[reason]));
    } finally {
      setBusy(false);
    }
  };

  const previewPersona: Persona = {
    id: 'avatar-preview',
    name: preview.name || 'Persona',
    kind: 'ai',
    color: preview.color,
    emoji: preview.emoji,
    gradient: preview.gradient,
    avatarUrl: value,
  };

  const maxMb = Math.round(AVATAR_MAX_INPUT_BYTES / (1024 * 1024));

  return (
    <Group align="center" gap="md" wrap="nowrap">
      <PersonaAvatar persona={previewPersona} size={72} />
      <Stack gap={6}>
        <Group gap="xs">
          <FileButton accept="image/*" onChange={(f) => void handleFile(f)} disabled={disabled}>
            {(props) => (
              <Button
                {...props}
                size="xs"
                variant="light"
                leftSection={<IconUpload size={14} />}
                loading={busy}
                disabled={disabled}
              >
                {t('avatar.upload')}
              </Button>
            )}
          </FileButton>
          {value && (
            <Button
              size="xs"
              variant="subtle"
              color="red"
              leftSection={<IconTrash size={14} />}
              disabled={disabled}
              onClick={() => {
                onChange(undefined);
                setError(null);
              }}
            >
              {t('avatar.remove')}
            </Button>
          )}
        </Group>
        <Text size="xs" c={error ? 'red' : 'dimmed'}>
          {error ?? t('avatar.hint', { size: maxMb })}
        </Text>
      </Stack>
    </Group>
  );
}
