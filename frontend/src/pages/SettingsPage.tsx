import {
  Box,
  Paper,
  SegmentedControl,
  Select,
  Stack,
  Text,
  TextInput,
  Title,
  useMantineColorScheme,
} from '@mantine/core';
import { useTranslation } from 'react-i18next';
import { UI_LANGUAGES } from '../i18n';
import { useWorkspace } from '../store/workspace';
import type { UiLanguage } from '../types';

const FONT_SIZES = [
  { value: '13', labelKey: 'settings.fontS' },
  { value: '15', labelKey: 'settings.fontM' },
  { value: '18', labelKey: 'settings.fontL' },
  { value: '22', labelKey: 'settings.fontXl' },
] as const;

export function SettingsPage() {
  const { t } = useTranslation();
  const settings = useWorkspace((s) => s.settings);
  const updateSettings = useWorkspace((s) => s.updateSettings);
  const { colorScheme, setColorScheme } = useMantineColorScheme();

  return (
    <Box p="lg" maw={560}>
      <Title order={3} mb="lg">
        {t('settings.title')}
      </Title>
      <Stack gap="lg">
        <Select
          label={t('settings.uiLanguage')}
          description={t('settings.uiLanguageHint')}
          value={settings.uiLanguage}
          onChange={(v) => v && updateSettings({ uiLanguage: v as UiLanguage })}
          allowDeselect={false}
          data={UI_LANGUAGES}
        />

        <TextInput
          label={t('settings.nativeLanguage')}
          description={t('settings.nativeLanguageHint', { token: '{{user_language}}' })}
          value={settings.nativeLanguage}
          onChange={(e) => updateSettings({ nativeLanguage: e.currentTarget.value })}
          placeholder="繁體中文"
        />

        <Stack gap={4}>
          <Title order={6}>{t('settings.colorScheme')}</Title>
          <SegmentedControl
            value={colorScheme === 'dark' ? 'dark' : 'light'}
            onChange={(v) => setColorScheme(v as 'light' | 'dark')}
            data={[
              { value: 'light', label: t('settings.colorSchemeLight') },
              { value: 'dark', label: t('settings.colorSchemeDark') },
            ]}
          />
        </Stack>

        <Stack gap={4}>
          <Title order={6}>{t('settings.chatFontSize')}</Title>
          <Text size="xs" c="dimmed">
            {t('settings.chatFontSizeHint')}
          </Text>
          <SegmentedControl
            value={String(settings.chatFontSize ?? 15)}
            onChange={(v) => updateSettings({ chatFontSize: Number(v) })}
            data={FONT_SIZES.map((f) => ({ value: f.value, label: t(f.labelKey) }))}
          />
          <Paper withBorder radius="md" p="sm" mt={4}>
            <Text style={{ fontSize: `${settings.chatFontSize ?? 15}px` }}>
              {t('settings.chatFontSample')}
            </Text>
          </Paper>
        </Stack>
      </Stack>
    </Box>
  );
}
