import { ActionIcon, Group, Menu, Tooltip, useMantineColorScheme } from '@mantine/core';
import { IconLanguage, IconMoon, IconSun } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import { UI_LANGUAGES } from '../i18n';
import { useWorkspace } from '../store/workspace';
import type { UiLanguage } from '../types';

export function HeaderControls() {
  const { t } = useTranslation();
  const uiLanguage = useWorkspace((s) => s.settings.uiLanguage);
  const updateSettings = useWorkspace((s) => s.updateSettings);
  const { colorScheme, toggleColorScheme } = useMantineColorScheme();

  return (
    <Group gap="xs">
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
    </Group>
  );
}
