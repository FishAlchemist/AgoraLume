import {
  Box,
  Button,
  Group,
  Paper,
  SegmentedControl,
  Select,
  Stack,
  Switch,
  Text,
  TextInput,
  Title,
  useMantineColorScheme,
} from '@mantine/core';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DataSourceBadge } from '../components/DataSourceBadge';
import { UI_LANGUAGES } from '../i18n';
import { useConnection } from '../store/connection';
import { useReadOnly } from '../store/readonly';
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
  const readOnly = useReadOnly((s) => s.readOnly);
  const setReadOnly = useReadOnly((s) => s.setReadOnly);
  const { colorScheme, setColorScheme } = useMantineColorScheme();

  return (
    <Box p="lg" maw={560}>
      <Title order={3} mb="lg">
        {t('settings.title')}
      </Title>
      <Stack gap="lg">
        <Switch
          checked={readOnly}
          onChange={(e) => setReadOnly(e.currentTarget.checked)}
          label={t('readonly.settingsLabel')}
          description={t('readonly.settingsHint')}
        />

        <Select
          label={t('settings.uiLanguage')}
          description={t('settings.uiLanguageHint')}
          value={settings.uiLanguage}
          onChange={(v) => v && updateSettings({ uiLanguage: v as UiLanguage })}
          allowDeselect={false}
          disabled={readOnly}
          data={UI_LANGUAGES}
        />

        <TextInput
          label={t('settings.nativeLanguage')}
          description={t('settings.nativeLanguageHint', { token: '{{user_language}}' })}
          value={settings.nativeLanguage}
          onChange={(e) => updateSettings({ nativeLanguage: e.currentTarget.value })}
          disabled={readOnly}
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
            disabled={readOnly}
            data={FONT_SIZES.map((f) => ({ value: f.value, label: t(f.labelKey) }))}
          />
          <Paper withBorder radius="md" p="sm" mt={4}>
            <Text style={{ fontSize: `${settings.chatFontSize ?? 15}px` }}>
              {t('settings.chatFontSample')}
            </Text>
          </Paper>
        </Stack>

        <ConnectionSettings />
      </Stack>
    </Box>
  );
}

/** Runtime choice of data source: the in-browser mock, or an HTTP backend. */
function ConnectionSettings() {
  const { t } = useTranslation();
  const backendUrl = useConnection((s) => s.backendUrl);
  const setBackendUrl = useConnection((s) => s.setBackendUrl);
  const [draft, setDraft] = useState(backendUrl ?? '');

  // Re-seed the field if the URL is changed elsewhere (e.g. reset to mock).
  const [lastUrl, setLastUrl] = useState(backendUrl);
  if (backendUrl !== lastUrl) {
    setLastUrl(backendUrl);
    setDraft(backendUrl ?? '');
  }

  const normalized = draft.trim().replace(/\/+$/, '');
  const canConnect = normalized.length > 0 && normalized !== backendUrl;

  return (
    <Stack gap={4}>
      <Group justify="space-between" align="center">
        <Title order={6}>{t('settings.connectionTitle')}</Title>
        <DataSourceBadge />
      </Group>
      <Text size="xs" c="dimmed">
        {t('settings.connectionHint')}
      </Text>
      <Group align="flex-end" gap="xs" wrap="nowrap">
        <TextInput
          flex={1}
          label={t('settings.backendUrl')}
          placeholder="http://127.0.0.1:8080"
          value={draft}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && canConnect) setBackendUrl(draft);
          }}
        />
        <Button onClick={() => setBackendUrl(draft)} disabled={!canConnect}>
          {t('settings.connect')}
        </Button>
      </Group>
      <Group justify="space-between" align="center" mt={4}>
        <Text size="xs" c="dimmed">
          {backendUrl ? t('settings.connectedTo', { url: backendUrl }) : t('settings.usingMock')}
        </Text>
        <Button
          variant="subtle"
          size="xs"
          onClick={() => setBackendUrl(null)}
          disabled={!backendUrl}
        >
          {t('settings.useMock')}
        </Button>
      </Group>
    </Stack>
  );
}
