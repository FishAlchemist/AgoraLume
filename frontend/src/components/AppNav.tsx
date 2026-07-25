import {
  ActionIcon,
  Divider,
  FileButton,
  Group,
  NavLink,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import {
  IconBuildingCommunity,
  IconDownload,
  IconMessageCircle,
  IconPlus,
  IconSettings,
  IconUpload,
  IconUser,
  IconUsers,
} from '@tabler/icons-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useLocation, useNavigate } from 'react-router-dom';
import { buildGroupBundle, downloadBundle, parseGroupBundle } from '../lib/transfer';
import { useWorkspace } from '../store/workspace';
import { GroupFormModal } from './GroupFormModal';

interface Props {
  onNavigate?: () => void;
}

export function AppNav({ onNavigate }: Props) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const groups = useWorkspace((s) => s.groups);
  const personas = useWorkspace((s) => s.personas);
  const organizations = useWorkspace((s) => s.organizations);
  const departments = useWorkspace((s) => s.departments);
  const importGroupBundle = useWorkspace((s) => s.importGroupBundle);
  const [createOpened, createHandlers] = useDisclosure(false);
  const [notice, setNotice] = useState<{ error: boolean; text: string } | null>(null);

  const go = (to: string) => {
    navigate(to);
    onNavigate?.();
  };

  const exportAllGroups = () => {
    downloadBundle(
      buildGroupBundle(groups, personas, organizations, departments),
      'agoralume-groups.agora.json',
    );
  };

  const handleImportGroups = async (file: File | null) => {
    if (!file) return;
    setNotice(null);
    try {
      const count = importGroupBundle(parseGroupBundle(await file.text()));
      setNotice({ error: false, text: t('transfer.importedGroups', { count }) });
    } catch {
      setNotice({ error: true, text: t('transfer.importFailed') });
    }
  };

  const sections = [
    {
      to: '/',
      label: t('nav.chat'),
      icon: IconMessageCircle,
      match: (p: string) => p === '/' || p.startsWith('/g/'),
    },
    {
      to: '/personas',
      label: t('nav.personas'),
      icon: IconUsers,
      match: (p: string) => p.startsWith('/personas'),
    },
    {
      to: '/organizations',
      label: t('nav.organizations'),
      icon: IconBuildingCommunity,
      match: (p: string) => p.startsWith('/organizations'),
    },
    {
      to: '/me',
      label: t('nav.me'),
      icon: IconUser,
      match: (p: string) => p.startsWith('/me'),
    },
    {
      to: '/settings',
      label: t('nav.settings'),
      icon: IconSettings,
      match: (p: string) => p.startsWith('/settings'),
    },
  ];

  const onChat = location.pathname === '/' || location.pathname.startsWith('/g/');

  return (
    <>
      <Stack gap="xs" h="100%">
        <Stack gap={2}>
          {sections.map((s) => (
            <NavLink
              key={s.to}
              active={s.match(location.pathname)}
              label={s.label}
              leftSection={<s.icon size={18} />}
              onClick={() => go(s.to)}
              variant="light"
            />
          ))}
        </Stack>

        {onChat && (
          <>
            <Divider />
            <Group justify="space-between" px="xs" wrap="nowrap">
              <Text size="xs" c="dimmed" tt="uppercase" fw={700}>
                {t('groups.title')}
              </Text>
              <Group gap={2} wrap="nowrap">
                <FileButton
                  accept="application/json,.json"
                  onChange={(f) => void handleImportGroups(f)}
                >
                  {(props) => (
                    <Tooltip label={t('transfer.importGroups')}>
                      <ActionIcon
                        {...props}
                        size="sm"
                        variant="subtle"
                        aria-label={t('transfer.importGroups')}
                      >
                        <IconUpload size={16} />
                      </ActionIcon>
                    </Tooltip>
                  )}
                </FileButton>
                {groups.length > 0 && (
                  <Tooltip label={t('transfer.exportGroups')}>
                    <ActionIcon
                      size="sm"
                      variant="subtle"
                      onClick={exportAllGroups}
                      aria-label={t('transfer.exportGroups')}
                    >
                      <IconDownload size={16} />
                    </ActionIcon>
                  </Tooltip>
                )}
                <Tooltip label={t('groups.add')}>
                  <ActionIcon
                    size="sm"
                    variant="subtle"
                    onClick={createHandlers.open}
                    aria-label={t('groups.add')}
                  >
                    <IconPlus size={16} />
                  </ActionIcon>
                </Tooltip>
              </Group>
            </Group>
            {notice && (
              <Text size="xs" c={notice.error ? 'red' : 'teal'} px="xs">
                {notice.text}
              </Text>
            )}
            <ScrollArea flex={1}>
              <Stack gap={2}>
                {groups.length === 0 ? (
                  <Text size="sm" c="dimmed" px="xs">
                    {t('groups.empty')}
                  </Text>
                ) : (
                  groups.map((group) => (
                    <NavLink
                      key={group.id}
                      active={location.pathname === `/g/${group.id}`}
                      label={group.name}
                      description={t('common.members', { count: group.personaIds.length })}
                      onClick={() => go(`/g/${group.id}`)}
                      variant="filled"
                    />
                  ))
                )}
              </Stack>
            </ScrollArea>
          </>
        )}
      </Stack>

      <GroupFormModal opened={createOpened} onClose={createHandlers.close} />
    </>
  );
}
