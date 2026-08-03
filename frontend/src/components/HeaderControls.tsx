import {
  ActionIcon,
  Badge,
  Box,
  Divider,
  Group,
  Menu,
  SegmentedControl,
  Select,
  Tooltip,
  useMantineColorScheme,
} from '@mantine/core';
import {
  IconDotsVertical,
  IconEye,
  IconLanguage,
  IconLogin,
  IconLogout,
  IconMoon,
  IconSun,
} from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';
import { UI_LANGUAGES } from '../i18n';
import { signOut } from '../lib/api/authFetch';
import { useAuth } from '../store/auth';
import { useConnection } from '../store/connection';
import { useReadOnly } from '../store/readonly';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';
import type { UiLanguage } from '../types';

export function HeaderControls() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const uiLanguage = useWorkspace((s) => s.settings.uiLanguage);
  const updateSettings = useWorkspace((s) => s.updateSettings);
  const readOnly = useReadOnly((s) => s.readOnly);
  const { colorScheme, toggleColorScheme, setColorScheme } = useMantineColorScheme();
  const backendUrl = useConnection((s) => s.backendUrl);
  const accessToken = useAuth((s) => s.accessToken);
  const username = useAuth((s) => s.username);
  const openLogin = useUi((s) => s.openLogin);

  const handleSignOut = () => {
    // `signOut` clears the local session synchronously and revokes the tokens
    // server-side in the background, so navigating away immediately is safe.
    void signOut();
    navigate('/');
  };

  return (
    <>
      {/* `sm` and up: the full row of icons/badges fits comfortably beside the
          title, so each control stays independently reachable. */}
      <Group gap="xs" wrap="nowrap" visibleFrom="sm">
        {readOnly && (
          <Tooltip label={t('readonly.badgeTooltip')}>
            <Badge variant="light" color="gray" leftSection={<IconEye size={12} />}>
              {t('readonly.badge')}
            </Badge>
          </Tooltip>
        )}

        {/* The language menu writes the backend-shared setting, so it's hidden in
            read-only mode; the theme toggle below is a device concern and stays. */}
        {!readOnly && (
          <Menu shadow="md" width={160} position="bottom-end">
            <Menu.Target>
              <Tooltip label={t('settings.uiLanguage')}>
                <ActionIcon variant="default" size="lg" aria-label={t('settings.uiLanguage')}>
                  <IconLanguage size={18} />
                </ActionIcon>
              </Tooltip>
            </Menu.Target>
            <Menu.Dropdown>
              {UI_LANGUAGES.map((lang) => (
                <Menu.Item
                  key={lang.value}
                  onClick={() => updateSettings({ uiLanguage: lang.value as UiLanguage })}
                  fw={lang.value === uiLanguage ? 700 : 400}
                >
                  {lang.label}
                </Menu.Item>
              ))}
            </Menu.Dropdown>
          </Menu>
        )}

        <Tooltip label={t('settings.colorScheme')}>
          <ActionIcon
            variant="default"
            size="lg"
            onClick={toggleColorScheme}
            aria-label={t('settings.colorScheme')}
          >
            {colorScheme === 'dark' ? <IconSun size={18} /> : <IconMoon size={18} />}
          </ActionIcon>
        </Tooltip>

        {backendUrl && !accessToken && (
          <Tooltip label={t('auth.signIn')}>
            <ActionIcon
              variant="default"
              size="lg"
              onClick={openLogin}
              aria-label={t('auth.signIn')}
            >
              <IconLogin size={18} />
            </ActionIcon>
          </Tooltip>
        )}

        {accessToken && username && (
          <Tooltip label={t('auth.signedInAs', { username })}>
            <Badge variant="light" color="blue">
              {username}
            </Badge>
          </Tooltip>
        )}

        {accessToken && (
          <Tooltip label={t('auth.signOut')}>
            <ActionIcon
              variant="default"
              size="lg"
              onClick={handleSignOut}
              aria-label={t('auth.signOut')}
            >
              <IconLogout size={18} />
            </ActionIcon>
          </Tooltip>
        )}
      </Group>

      {/* Below `sm` the same controls don't fit next to the title without
          wrapping the header onto a second line, so they fold into one menu
          instead — the header stays a single row on any device width. */}
      <Group gap="xs" wrap="nowrap" hiddenFrom="sm">
        {readOnly && (
          <Badge variant="light" color="gray" leftSection={<IconEye size={12} />}>
            {t('readonly.badge')}
          </Badge>
        )}
        <Menu shadow="md" width={200} position="bottom-end">
          <Menu.Target>
            <ActionIcon variant="default" size="lg" aria-label={t('nav.settings')}>
              <IconDotsVertical size={18} />
            </ActionIcon>
          </Menu.Target>
          <Menu.Dropdown>
            {accessToken && username && (
              <Menu.Label>{t('auth.signedInAs', { username })}</Menu.Label>
            )}

            {!readOnly && (
              <>
                <Box px="sm" py={4}>
                  <Select
                    size="xs"
                    value={uiLanguage}
                    onChange={(v) => v && updateSettings({ uiLanguage: v as UiLanguage })}
                    allowDeselect={false}
                    data={UI_LANGUAGES}
                    leftSection={<IconLanguage size={14} />}
                  />
                </Box>
                <Divider />
              </>
            )}

            <Box px="sm" py={4}>
              <SegmentedControl
                fullWidth
                size="xs"
                value={colorScheme === 'dark' ? 'dark' : 'light'}
                onChange={(v) => setColorScheme(v as 'light' | 'dark')}
                data={[
                  { value: 'light', label: t('settings.colorSchemeLight') },
                  { value: 'dark', label: t('settings.colorSchemeDark') },
                ]}
              />
            </Box>
            <Divider />

            {backendUrl && !accessToken && (
              <Menu.Item leftSection={<IconLogin size={16} />} onClick={openLogin}>
                {t('auth.signIn')}
              </Menu.Item>
            )}

            {accessToken && (
              <Menu.Item color="red" leftSection={<IconLogout size={16} />} onClick={handleSignOut}>
                {t('auth.signOut')}
              </Menu.Item>
            )}
          </Menu.Dropdown>
        </Menu>
      </Group>
    </>
  );
}
