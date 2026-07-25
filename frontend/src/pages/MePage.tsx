import {
  Badge,
  Box,
  Button,
  Card,
  Group,
  Stack,
  Text,
  Textarea,
  TextInput,
  Title,
} from '@mantine/core';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AvatarUpload } from '../components/AvatarUpload';
import { ColorSelect } from '../components/ColorSelect';
import { isNameTaken, MAX_PERSONA_BLURB_LEN, MAX_PERSONA_NAME_LEN } from '../lib/persona';
import { useWorkspace } from '../store/workspace';
import type { Persona } from '../types';

/** The editable fields of the user's own profile. */
interface Draft {
  name: string;
  blurb: string;
  color: string;
  emoji: string;
  avatarUrl: string;
}

function toDraft(me: Persona | undefined): Draft {
  return {
    name: me?.name ?? '',
    blurb: me?.blurb ?? '',
    color: me?.color ?? 'gray',
    emoji: me?.emoji ?? '',
    avatarUrl: me?.avatarUrl ?? '',
  };
}

/** Whether the draft differs from the stored identity (so Save is meaningful). */
function isDirty(draft: Draft, me: Persona | undefined): boolean {
  const base = toDraft(me);
  return (
    draft.name !== base.name ||
    draft.blurb !== base.blurb ||
    draft.color !== base.color ||
    draft.emoji !== base.emoji ||
    draft.avatarUrl !== base.avatarUrl
  );
}

/** Maps a draft onto the persona patch sent to the store (trims, drops blanks). */
function toPatch(draft: Draft): Partial<Persona> {
  return {
    name: draft.name.trim(),
    blurb: draft.blurb.trim() || undefined,
    color: draft.color,
    emoji: draft.emoji.trim() || undefined,
    avatarUrl: draft.avatarUrl || undefined,
  };
}

/**
 * The user's own profile — a standalone editor for the single "you" identity
 * (name, bio, avatar, colour). Unlike the AI persona modal it edits in place, so
 * it reads like a personal-details page. The "you"/"你" wording lives only here,
 * in the UI; the stored identity carries a plain name.
 */
export function MePage() {
  const { t } = useTranslation();
  const personas = useWorkspace((s) => s.personas);
  const updatePersona = useWorkspace((s) => s.updatePersona);

  const me = personas.find((p) => p.kind === 'user');

  // Local draft, re-seeded whenever the identity itself changes (e.g. a backend
  // hydrate) — an adjust-state-during-render reset keyed on the persona id.
  const [seededId, setSeededId] = useState<string | undefined>(me?.id);
  const [draft, setDraft] = useState<Draft>(() => toDraft(me));
  if (me?.id !== seededId) {
    setSeededId(me?.id);
    setDraft(toDraft(me));
  }

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) =>
    setDraft((d) => ({ ...d, [key]: value }));

  const nameErr = isNameTaken(personas, draft.name, me?.id) ? t('personas.nameTaken') : undefined;
  const canSave = Boolean(me) && draft.name.trim().length > 0 && !nameErr && isDirty(draft, me);

  const handleSave = () => {
    if (me && canSave) updatePersona(me.id, toPatch(draft));
  };

  if (!me) {
    return (
      <Box p="lg" maw={560}>
        <Text c="dimmed">{t('me.missing')}</Text>
      </Box>
    );
  }

  return (
    <Box p="lg" maw={560}>
      <Stack gap={2} mb="lg">
        <Group gap="sm">
          <Title order={3}>{t('me.title')}</Title>
          <Badge variant="light" color={draft.color}>
            {t('me.you')}
          </Badge>
        </Group>
        <Text c="dimmed" size="sm">
          {t('me.subtitle')}
        </Text>
      </Stack>

      <Card withBorder radius="lg" padding="lg">
        <Stack gap="md">
          <AvatarUpload
            value={draft.avatarUrl || undefined}
            onChange={(v) => set('avatarUrl', v ?? '')}
            preview={{
              name: draft.name,
              color: draft.color,
              emoji: draft.emoji || undefined,
              gradient: me.gradient,
            }}
          />

          <TextInput
            label={t('personas.displayName')}
            value={draft.name}
            onChange={(e) => set('name', e.currentTarget.value)}
            error={nameErr}
            maxLength={MAX_PERSONA_NAME_LEN}
            data-autofocus
          />

          <Group grow align="flex-start">
            <ColorSelect
              label={t('common.color')}
              value={draft.color}
              onChange={(v) => set('color', v)}
            />
            <TextInput
              label={t('personas.emoji')}
              value={draft.emoji}
              onChange={(e) => set('emoji', e.currentTarget.value)}
              maxLength={4}
            />
          </Group>

          <Textarea
            label={t('personas.blurb')}
            value={draft.blurb}
            onChange={(e) => set('blurb', e.currentTarget.value)}
            maxLength={MAX_PERSONA_BLURB_LEN}
            autosize
            minRows={2}
            maxRows={5}
          />

          <Group justify="flex-end" mt="xs">
            <Button onClick={handleSave} disabled={!canSave}>
              {t('common.save')}
            </Button>
          </Group>
        </Stack>
      </Card>
    </Box>
  );
}
