import {
  ActionIcon,
  Divider,
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
  IconMessageCircle,
  IconPlus,
  IconSettings,
  IconUsers,
} from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import { useLocation, useNavigate } from 'react-router-dom';
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
  const [createOpened, createHandlers] = useDisclosure(false);

  const go = (to: string) => {
    navigate(to);
    onNavigate?.();
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
            <Group justify="space-between" px="xs">
              <Text size="xs" c="dimmed" tt="uppercase" fw={700}>
                {t('groups.title')}
              </Text>
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
