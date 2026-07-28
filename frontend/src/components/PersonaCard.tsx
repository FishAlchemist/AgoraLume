import {
  Badge,
  Button,
  Code,
  Divider,
  Group,
  Modal,
  Paper,
  ScrollArea,
  Stack,
  Text,
} from '@mantine/core';
import { IconBrain, IconDownload, IconPencil } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import { resolveSystemPrompt, resolveVariables } from '../lib/prompt';
import { buildBundle, downloadBundle, slugify } from '../lib/transfer';
import { useReadOnly } from '../store/readonly';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';
import { CopyIconButton } from './CopyIconButton';
import { OrgTag } from './OrgTag';
import { PersonaAvatar } from './PersonaAvatar';

/** Read-only info card for a persona, opened by clicking its avatar anywhere. */
export function PersonaCard() {
  const { t } = useTranslation();
  const cardPersonaId = useUi((s) => s.cardPersonaId);
  const closeCard = useUi((s) => s.closeCard);
  const openEditor = useUi((s) => s.openEditor);
  const openMemory = useUi((s) => s.openMemory);
  const readOnly = useReadOnly((s) => s.readOnly);
  const personas = useWorkspace((s) => s.personas);
  const organizations = useWorkspace((s) => s.organizations);
  const departments = useWorkspace((s) => s.departments);
  const settings = useWorkspace((s) => s.settings);

  const persona = personas.find((p) => p.id === cardPersonaId);
  const org = persona ? organizations.find((o) => o.id === persona.organizationId) : undefined;
  const department = persona ? departments.find((d) => d.id === persona.departmentId) : undefined;

  const resolvedPrompt = persona ? resolveSystemPrompt(persona, org, department, settings) : '';
  const variables = persona ? resolveVariables(persona, org, department, settings) : {};

  const handleEdit = () => {
    if (!persona) return;
    closeCard();
    openEditor(persona.id);
  };

  const handleMemory = () => {
    if (!persona) return;
    closeCard();
    openMemory(persona.id);
  };

  const handleExport = () => {
    if (!persona) return;
    downloadBundle(
      buildBundle([persona], organizations, departments),
      `${slugify(persona.name)}.agora.json`,
    );
  };

  return (
    <Modal
      opened={Boolean(persona)}
      onClose={closeCard}
      size="lg"
      title={t('personas.card')}
      scrollAreaComponent={ScrollArea.Autosize}
    >
      {persona && (
        <Stack gap="md">
          <Group wrap="nowrap">
            <PersonaAvatar persona={persona} size={72} />
            <Stack gap={6} miw={0}>
              <Text fw={800} fz="xl" truncate>
                {persona.name}
              </Text>
              <Group gap={6}>
                <Badge variant="light" color={persona.color}>
                  {persona.kind === 'user' ? t('personas.kindUser') : t('personas.kindAi')}
                </Badge>
                <OrgTag organization={org} department={department} size="md" />
              </Group>
            </Stack>
          </Group>

          {persona.blurb && <Text c="dimmed">{persona.blurb}</Text>}

          {persona.kind !== 'user' && (
            <>
              <Divider />

              <Stack gap={4}>
                <Group justify="space-between" align="center" wrap="nowrap">
                  <Text fw={600} size="sm">
                    {t('personas.systemPrompt')}
                  </Text>
                  {resolvedPrompt && <CopyIconButton value={resolvedPrompt} />}
                </Group>
                <Paper withBorder p="sm" radius="md" bg="var(--mantine-color-body)">
                  <Text
                    size="sm"
                    style={{ whiteSpace: 'pre-wrap' }}
                    c={resolvedPrompt ? undefined : 'dimmed'}
                  >
                    {resolvedPrompt || '—'}
                  </Text>
                </Paper>
              </Stack>

              {Object.keys(variables).length > 0 && (
                <Stack gap={4}>
                  <Text fw={600} size="sm">
                    {t('personas.variables')}
                  </Text>
                  <Stack gap={2}>
                    {Object.entries(variables).map(([k, v]) => (
                      <Text key={k} size="xs">
                        <Code>{`{{${k}}}`}</Code> → {v || '—'}
                      </Text>
                    ))}
                  </Stack>
                </Stack>
              )}
            </>
          )}

          <Group justify="space-between" mt="xs">
            {persona.kind === 'user' ? (
              <span />
            ) : (
              <Group gap="xs">
                <Button
                  variant="light"
                  leftSection={<IconBrain size={16} />}
                  onClick={handleMemory}
                >
                  {t('memory.title')}
                </Button>
                <Button
                  variant="light"
                  color="gray"
                  leftSection={<IconDownload size={16} />}
                  onClick={handleExport}
                >
                  {t('transfer.exportOne')}
                </Button>
              </Group>
            )}
            {!readOnly && (
              <Button leftSection={<IconPencil size={16} />} onClick={handleEdit}>
                {t('common.edit')}
              </Button>
            )}
          </Group>
        </Stack>
      )}
    </Modal>
  );
}
