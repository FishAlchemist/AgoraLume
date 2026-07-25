import { Box, Button, Card, Group, Stack, Text, Title } from '@mantine/core';
import { IconPencil } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import { PersonaAvatar } from '../components/PersonaAvatar';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';

/**
 * The user's own identity. There is exactly one "you"; this page edits its name,
 * blurb and avatar (via the shared persona editor). It replaces the old
 * multi-identity list, which was confusing and had no real use.
 */
export function MePage() {
  const { t } = useTranslation();
  const personas = useWorkspace((s) => s.personas);
  const openEditor = useUi((s) => s.openEditor);

  const me = personas.find((p) => p.kind === 'user');

  return (
    <Box p="lg" maw={560}>
      <Stack gap={2} mb="lg">
        <Title order={3}>{t('me.title')}</Title>
        <Text c="dimmed" size="sm">
          {t('me.subtitle')}
        </Text>
      </Stack>

      {me ? (
        <Card withBorder radius="lg" padding="lg">
          <Group justify="space-between" wrap="nowrap" align="flex-start">
            <Group wrap="nowrap" miw={0}>
              <PersonaAvatar persona={me} size={64} />
              <Stack gap={4} miw={0}>
                <Text fw={700} size="lg" truncate>
                  {me.name}
                </Text>
                <Text size="sm" c={me.blurb ? undefined : 'dimmed'}>
                  {me.blurb || t('me.noBlurb')}
                </Text>
              </Stack>
            </Group>
            <Button
              variant="light"
              leftSection={<IconPencil size={16} />}
              onClick={() => openEditor(me.id)}
            >
              {t('common.edit')}
            </Button>
          </Group>
        </Card>
      ) : (
        <Text c="dimmed">{t('me.missing')}</Text>
      )}
    </Box>
  );
}
