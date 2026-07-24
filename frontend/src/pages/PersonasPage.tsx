import {
  ActionIcon,
  Badge,
  Box,
  Button,
  Card,
  Divider,
  FileButton,
  Group,
  Menu,
  Select,
  SimpleGrid,
  Stack,
  Text,
  Title,
  Tooltip,
} from '@mantine/core';
import {
  IconChevronDown,
  IconDownload,
  IconPencil,
  IconPlus,
  IconTrash,
  IconUpload,
} from '@tabler/icons-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { OrgTag } from '../components/OrgTag';
import { PersonaAvatar } from '../components/PersonaAvatar';
import { buildBundle, downloadBundle, parseBundle, slugify } from '../lib/transfer';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';
import type { Persona } from '../types';

const ALL = '__all__';
const UNASSIGNED = '__none__';
const ALL_DEPT = '__all__';
const NO_DEPT = '__none__';

export function PersonasPage() {
  const { t } = useTranslation();
  const personas = useWorkspace((s) => s.personas);
  const organizations = useWorkspace((s) => s.organizations);
  const departments = useWorkspace((s) => s.departments);
  const deletePersona = useWorkspace((s) => s.deletePersona);
  const importBundle = useWorkspace((s) => s.importBundle);
  const openCard = useUi((s) => s.openCard);
  const openEditor = useUi((s) => s.openEditor);
  const askConfirm = useUi((s) => s.askConfirm);

  const [orgFilter, setOrgFilter] = useState<string>(ALL);
  const [deptFilter, setDeptFilter] = useState<string>(ALL_DEPT);
  const [notice, setNotice] = useState<{ error: boolean; text: string } | null>(null);

  const aiPersonas = personas.filter((p) => p.kind === 'ai');
  const userPersonas = personas.filter((p) => p.kind === 'user');

  const orgFilterActive = orgFilter !== ALL && orgFilter !== UNASSIGNED;
  const selectedOrg = organizations.find((o) => o.id === orgFilter);
  const orgDepartments = orgFilterActive
    ? departments.filter((d) => d.organizationId === orgFilter)
    : [];
  const deptFilterActive = orgFilterActive && deptFilter !== ALL_DEPT && deptFilter !== NO_DEPT;
  const selectedDept = departments.find((d) => d.id === deptFilter);

  const changeOrgFilter = (value: string) => {
    setOrgFilter(value);
    setDeptFilter(ALL_DEPT);
  };

  const matchesOrg = (p: Persona) => {
    if (orgFilter === ALL) return true;
    if (orgFilter === UNASSIGNED) return !p.organizationId;
    return p.organizationId === orgFilter;
  };
  const matchesDept = (p: Persona) => {
    if (!orgFilterActive || deptFilter === ALL_DEPT) return true;
    if (deptFilter === NO_DEPT) return !p.departmentId;
    return p.departmentId === deptFilter;
  };
  const filteredAi = aiPersonas.filter((p) => matchesOrg(p) && matchesDept(p));

  const handleDelete = (persona: Persona) => {
    askConfirm({
      message: t(
        persona.kind === 'user' ? 'personas.confirmDeleteIdentity' : 'personas.confirmDelete',
      ),
      confirmLabel: t('common.delete'),
      danger: true,
      onConfirm: () => deletePersona(persona.id),
    });
  };

  const exportList = (list: Persona[], filename: string) => {
    downloadBundle(buildBundle(list, organizations, departments), `${filename}.agora.json`);
  };
  const handleImport = async (file: File | null) => {
    if (!file) return;
    setNotice(null);
    try {
      const count = importBundle(parseBundle(await file.text()));
      setNotice({ error: false, text: t('transfer.imported', { count }) });
    } catch {
      setNotice({ error: true, text: t('transfer.importFailed') });
    }
  };

  return (
    <Box p="lg">
      <Group justify="space-between" align="flex-end" mb="md">
        <Stack gap={2}>
          <Title order={3}>{t('personas.title')}</Title>
          <Text c="dimmed" size="sm">
            {t('personas.subtitle')}
          </Text>
        </Stack>
        <Group gap="xs">
          <FileButton accept="application/json,.json" onChange={(f) => void handleImport(f)}>
            {(props) => (
              <Button {...props} variant="default" leftSection={<IconUpload size={16} />}>
                {t('transfer.import')}
              </Button>
            )}
          </FileButton>
          <Menu position="bottom-end" withinPortal>
            <Menu.Target>
              <Button
                variant="default"
                leftSection={<IconDownload size={16} />}
                rightSection={<IconChevronDown size={14} />}
              >
                {t('transfer.export')}
              </Button>
            </Menu.Target>
            <Menu.Dropdown>
              <Menu.Item onClick={() => exportList(aiPersonas, 'agoralume-personas')}>
                {t('transfer.exportAll')}
              </Menu.Item>
              {orgFilterActive && selectedOrg && (
                <Menu.Item
                  onClick={() =>
                    exportList(
                      aiPersonas.filter((p) => p.organizationId === selectedOrg.id),
                      slugify(selectedOrg.name),
                    )
                  }
                >
                  {t('transfer.exportNamed', { name: selectedOrg.name })}
                </Menu.Item>
              )}
              {deptFilterActive && selectedDept && (
                <Menu.Item
                  onClick={() =>
                    exportList(
                      aiPersonas.filter((p) => p.departmentId === selectedDept.id),
                      slugify(selectedDept.name),
                    )
                  }
                >
                  {t('transfer.exportNamed', { name: selectedDept.name })}
                </Menu.Item>
              )}
            </Menu.Dropdown>
          </Menu>
          <Button leftSection={<IconPlus size={16} />} onClick={() => openEditor(null, 'ai')}>
            {t('personas.add')}
          </Button>
        </Group>
      </Group>

      <Group mb="lg" gap="md" align="flex-end">
        <Select
          w={220}
          value={orgFilter}
          onChange={(v) => v && changeOrgFilter(v)}
          allowDeselect={false}
          label={t('personas.filterByOrg')}
          data={[
            { value: ALL, label: t('personas.filterAll') },
            { value: UNASSIGNED, label: t('personas.noOrg') },
            ...organizations.map((o) => ({ value: o.id, label: o.name })),
          ]}
        />
        {orgFilterActive && orgDepartments.length > 0 && (
          <Select
            w={220}
            value={deptFilter}
            onChange={(v) => v && setDeptFilter(v)}
            allowDeselect={false}
            label={t('personas.filterByDept')}
            data={[
              { value: ALL_DEPT, label: t('personas.filterAllDept') },
              { value: NO_DEPT, label: t('personas.noDepartment') },
              ...orgDepartments.map((d) => ({ value: d.id, label: d.name })),
            ]}
          />
        )}
        {notice && (
          <Text size="sm" c={notice.error ? 'red' : 'teal'}>
            {notice.text}
          </Text>
        )}
      </Group>

      {filteredAi.length === 0 ? (
        <Text c="dimmed">{t('personas.empty')}</Text>
      ) : (
        <SimpleGrid cols={{ base: 1, sm: 2, lg: 3 }} spacing="md">
          {filteredAi.map((persona) => {
            const org = organizations.find((o) => o.id === persona.organizationId);
            const dept = departments.find((d) => d.id === persona.departmentId);
            return (
              <Card key={persona.id} withBorder radius="lg" padding="md">
                <Group justify="space-between" wrap="nowrap" align="flex-start">
                  <Group wrap="nowrap" miw={0}>
                    <PersonaAvatar
                      persona={persona}
                      size={48}
                      onClick={() => openCard(persona.id)}
                    />
                    <Stack gap={4} miw={0}>
                      <Text fw={700} truncate>
                        {persona.name}
                      </Text>
                      <Group gap={6}>
                        <OrgTag organization={org} department={dept} />
                      </Group>
                    </Stack>
                  </Group>
                  <Group gap={2} wrap="nowrap">
                    <Tooltip label={t('common.edit')}>
                      <ActionIcon
                        variant="subtle"
                        onClick={() => openEditor(persona.id)}
                        aria-label={t('common.edit')}
                      >
                        <IconPencil size={16} />
                      </ActionIcon>
                    </Tooltip>
                    <Tooltip label={t('common.delete')}>
                      <ActionIcon
                        variant="subtle"
                        color="red"
                        onClick={() => handleDelete(persona)}
                        aria-label={t('common.delete')}
                      >
                        <IconTrash size={16} />
                      </ActionIcon>
                    </Tooltip>
                  </Group>
                </Group>

                {persona.blurb && (
                  <Text size="sm" c="dimmed" mt="sm" lineClamp={2}>
                    {persona.blurb}
                  </Text>
                )}
                {persona.systemPrompt && (
                  <Text size="xs" c="dimmed" mt="xs" lineClamp={2} ff="monospace">
                    {persona.systemPrompt}
                  </Text>
                )}
              </Card>
            );
          })}
        </SimpleGrid>
      )}

      <Divider my="xl" />

      <Group justify="space-between" align="flex-end" mb="md">
        <Stack gap={2}>
          <Title order={4}>{t('personas.identitiesTitle')}</Title>
          <Text c="dimmed" size="sm">
            {t('personas.identitiesSubtitle')}
          </Text>
        </Stack>
        <Button
          variant="light"
          leftSection={<IconPlus size={16} />}
          onClick={() => openEditor(null, 'user')}
        >
          {t('personas.addIdentity')}
        </Button>
      </Group>

      <SimpleGrid cols={{ base: 1, sm: 2, lg: 3 }} spacing="md">
        {userPersonas.map((persona) => (
          <Card key={persona.id} withBorder radius="lg" padding="md">
            <Group justify="space-between" wrap="nowrap" align="flex-start">
              <Group wrap="nowrap" miw={0}>
                <PersonaAvatar persona={persona} size={48} onClick={() => openCard(persona.id)} />
                <Stack gap={4} miw={0}>
                  <Text fw={700} truncate>
                    {persona.name}
                  </Text>
                  <Badge size="xs" variant="light" color={persona.color}>
                    {t('personas.kindUser')}
                  </Badge>
                </Stack>
              </Group>
              <Group gap={2} wrap="nowrap">
                <Tooltip label={t('common.edit')}>
                  <ActionIcon
                    variant="subtle"
                    onClick={() => openEditor(persona.id)}
                    aria-label={t('common.edit')}
                  >
                    <IconPencil size={16} />
                  </ActionIcon>
                </Tooltip>
                {userPersonas.length > 1 && (
                  <Tooltip label={t('common.delete')}>
                    <ActionIcon
                      variant="subtle"
                      color="red"
                      onClick={() => handleDelete(persona)}
                      aria-label={t('common.delete')}
                    >
                      <IconTrash size={16} />
                    </ActionIcon>
                  </Tooltip>
                )}
              </Group>
            </Group>
            {persona.blurb && (
              <Text size="sm" c="dimmed" mt="sm" lineClamp={2}>
                {persona.blurb}
              </Text>
            )}
          </Card>
        ))}
      </SimpleGrid>
    </Box>
  );
}
