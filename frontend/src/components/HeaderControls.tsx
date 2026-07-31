import { ActionIcon, Badge, Group, Menu, Tooltip, useMantineColorScheme } from '@mantine/core';
import {
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
  const { colorScheme, toggleColorScheme } = useMantineColorScheme();
  const backendUrl = useConnection((s) => s.backendUrl);
  const accessToken = useAuth((s) => s.accessToken);
  const username = useAuth((s) => s.username);
  const clearAuth = useAuth((s) => s.clear);
  const openLogin = useUi((s) => s.openLogin);

  return (
    <Group gap="xs">
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
          <ActionIcon variant="default" size="lg" onClick={openLogin} aria-label={t('auth.signIn')}>
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
            onClick={() => {
              clearAuth();
              navigate('/');
            }}
            aria-label={t('auth.signOut')}
          >
            <IconLogout size={18} />
          </ActionIcon>
        </Tooltip>
      )}
    </Group>
  );
}
