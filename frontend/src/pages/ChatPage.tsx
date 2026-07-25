import {
  ActionIcon,
  Avatar,
  Badge,
  Box,
  Button,
  Center,
  Group,
  Menu,
  Stack,
  Text,
  Tooltip,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { IconBug, IconChevronDown, IconDownload, IconPencil, IconTrash } from '@tabler/icons-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useParams } from 'react-router-dom';
import { ChatView } from '../components/ChatView';
import { DebugPanel } from '../components/DebugPanel';
import { GroupFormModal } from '../components/GroupFormModal';
import { PersonaAvatar } from '../components/PersonaAvatar';
import { buildGroupBundle, downloadBundle, slugify } from '../lib/transfer';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';
import type { Persona } from '../types';

export function ChatPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { groupId } = useParams();
  const groups = useWorkspace((s) => s.groups);
  const personas = useWorkspace((s) => s.personas);
  const organizations = useWorkspace((s) => s.organizations);
  const departments = useWorkspace((s) => s.departments);
  const deleteGroup = useWorkspace((s) => s.deleteGroup);
  const updateGroup = useWorkspace((s) => s.updateGroup);
  const openCard = useUi((s) => s.openCard);
  const askConfirm = useUi((s) => s.askConfirm);
  const [editOpened, editHandlers] = useDisclosure(false);
  const [debugOpen, debugHandlers] = useDisclosure(false);

  const group = groups.find((g) => g.id === groupId);

  const personaMap = useMemo(() => {
    const map = new Map<string, Persona>();
    for (const p of personas) map.set(p.id, p);
    return map;
  }, [personas]);

  if (!group) {
    return (
      <Center h="100%">
        <Text c="dimmed">{t('chat.noGroup')}</Text>
      </Center>
    );
  }

  const members = group.personaIds
    .map((id) => personaMap.get(id))
    .filter((p): p is Persona => Boolean(p));

  const userPersonas = personas.filter((p) => p.kind === 'user');
  const selfPersona = personaMap.get(group.selfPersonaId) ?? userPersonas[0] ?? undefined;

  const handleExport = () => {
    downloadBundle(
      buildGroupBundle([group], personas, organizations, departments),
      `${slugify(group.name)}.group.agora.json`,
    );
  };

  const handleDelete = () => {
    askConfirm({
      message: t('groups.confirmDelete'),
      confirmLabel: t('common.delete'),
      danger: true,
      onConfirm: () => {
        deleteGroup(group.id);
        navigate('/');
      },
    });
  };

  return (
    <Stack h="100%" gap={0}>
      <Group
        justify="space-between"
        px="md"
        py="xs"
        wrap="nowrap"
        style={{ borderBottom: '1px solid var(--mantine-color-default-border)' }}
      >
        <Group gap="sm" wrap="nowrap" miw={0}>
          <Text fw={700} truncate>
            {group.name}
          </Text>
          <Avatar.Group>
            {members.slice(0, 5).map((p) => (
              <Avatar
                key={p.id}
                src={p.avatarUrl ?? null}
                size={26}
                radius="xl"
                onClick={() => openCard(p.id)}
                style={{ background: p.avatarUrl ? undefined : p.gradient, cursor: 'pointer' }}
              >
                {p.emoji ?? p.name.slice(0, 1)}
              </Avatar>
            ))}
          </Avatar.Group>
          <Text size="xs" c="dimmed">
            {t('common.members', { count: members.length })}
          </Text>
          {members.length === 0 && (
            <Badge size="sm" variant="light" color="gray">
              {t('chat.lockedBadge')}
            </Badge>
          )}
        </Group>
        <Group gap="xs" wrap="nowrap">
          {selfPersona && (
            <Menu position="bottom-end" withinPortal>
              <Menu.Target>
                <Button
                  variant="default"
                  size="xs"
                  radius="xl"
                  px={8}
                  leftSection={<PersonaAvatar persona={selfPersona} size={20} />}
                  rightSection={<IconChevronDown size={14} />}
                >
                  {selfPersona.name}
                </Button>
              </Menu.Target>
              <Menu.Dropdown>
                <Menu.Label>{t('chat.speakingAs')}</Menu.Label>
                {userPersonas.map((p) => (
                  <Menu.Item
                    key={p.id}
                    leftSection={<PersonaAvatar persona={p} size={20} />}
                    onClick={() => updateGroup(group.id, { selfPersonaId: p.id })}
                  >
                    {p.name}
                  </Menu.Item>
                ))}
              </Menu.Dropdown>
            </Menu>
          )}
          <Tooltip label={t('transfer.exportGroup')}>
            <ActionIcon
              variant="subtle"
              onClick={handleExport}
              aria-label={t('transfer.exportGroup')}
            >
              <IconDownload size={18} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label={t('debug.toggle')}>
            <ActionIcon
              variant={debugOpen ? 'light' : 'subtle'}
              color={debugOpen ? 'blue' : 'gray'}
              onClick={debugHandlers.toggle}
              aria-label={t('debug.toggle')}
            >
              <IconBug size={18} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label={t('groups.edit')}>
            <ActionIcon variant="subtle" onClick={editHandlers.open} aria-label={t('groups.edit')}>
              <IconPencil size={18} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label={t('common.delete')}>
            <ActionIcon
              variant="subtle"
              color="red"
              onClick={handleDelete}
              aria-label={t('common.delete')}
            >
              <IconTrash size={18} />
            </ActionIcon>
          </Tooltip>
        </Group>
      </Group>

      <Box flex={1} mih={0}>
        <Stack h="100%" gap={0}>
          {debugOpen && <DebugPanel groupId={group.id} personas={personaMap} />}
          <Box flex={1} mih={0}>
            <ChatView key={group.id} group={group} personas={personaMap} />
          </Box>
        </Stack>
      </Box>

      <GroupFormModal opened={editOpened} onClose={editHandlers.close} group={group} />
    </Stack>
  );
}
