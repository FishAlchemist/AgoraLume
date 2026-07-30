import {
  Accordion,
  Alert,
  Box,
  Button,
  Group,
  NumberInput,
  Paper,
  PasswordInput,
  SegmentedControl,
  Select,
  Stack,
  Switch,
  Text,
  TextInput,
  Title,
  useMantineColorScheme,
} from '@mantine/core';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DataSourceBadge } from '../components/DataSourceBadge';
import { UsageSummary } from '../components/UsageSummary';
import { UI_LANGUAGES } from '../i18n';
import { api } from '../lib/api';
import { getLlmSettings, updateLlmSettings } from '../lib/api/llmSettings';
import type { DebugUsage, LlmSettingsPatch, LlmSettingsView, ServerMeta } from '../lib/api/types';
import { useConnection } from '../store/connection';
import { useReadOnly } from '../store/readonly';
import { useWorkspace } from '../store/workspace';
import type { UiLanguage } from '../types';

/** How often to re-poll the LLM cost readout while Settings is open. No global
 * SSE channel exists (each `debug` stream is per-group), so this just polls,
 * matching `useBackendStatus`'s cadence. */
const USAGE_POLL_MS = 8_000;

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

        <UsageSettings />

        <LlmProviderSettings />
      </Stack>
    </Box>
  );
}

/**
 * The LLM cost readout, folded into Settings as a low-key section rather than
 * a top-level nav destination — this app is a multi-persona chatroom, and a
 * billing dashboard sitting next to Chat/Personas/Organizations read as out of
 * place. See `UsageSummary` for how the number itself stays the only thing
 * shown at a glance, with everything else behind "show details".
 */
function UsageSettings() {
  const { t } = useTranslation();
  const [usage, setUsage] = useState<DebugUsage | null>(null);
  const [meta, setMeta] = useState<ServerMeta | null>(null);

  useEffect(() => {
    let active = true;
    const load = () => {
      void api.getUsage().then((u) => {
        if (active) setUsage(u);
      });
      void api.probe().then((m) => {
        if (active) setMeta(m);
      });
    };
    load();
    const id = setInterval(load, USAGE_POLL_MS);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, []);

  return (
    <Stack gap={4}>
      <Title order={6}>{t('settings.usageTitle')}</Title>
      <UsageSummary usage={usage} mock={meta?.mock} title={null} />
      {meta && (
        <Text size="xs" c={meta.persistent ? 'teal' : 'dimmed'}>
          {meta.persistent ? t('settings.usagePersisted') : t('settings.usageNotPersisted')}
        </Text>
      )}
    </Stack>
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

/** The editable mirror of {@link LlmSettingsView}, minus the API key (never
 * sent from the server — tracked separately, see {@link LlmProviderForm}). */
interface LlmDraft {
  enabled: boolean;
  baseUrl: string;
  model: string;
  maxTokens: number;
  maxRpm: number;
  maxRetries: number;
  retryBaseMs: number;
  compressAfter: number;
  compressKeep: number;
  compressMaxTokens: number;
  pricingEnabled: boolean;
  inputPerM: number;
  cachedInputPerM: number;
  outputPerM: number;
  currency: string;
}

function draftFromView(v: LlmSettingsView): LlmDraft {
  return {
    enabled: v.enabled,
    baseUrl: v.baseUrl ?? '',
    model: v.model ?? '',
    maxTokens: v.maxTokens,
    maxRpm: v.maxRpm,
    maxRetries: v.maxRetries,
    retryBaseMs: v.retryBaseMs,
    compressAfter: v.compressAfter,
    compressKeep: v.compressKeep,
    compressMaxTokens: v.compressMaxTokens,
    pricingEnabled: v.pricing != null,
    inputPerM: v.pricing?.inputPerM ?? 0,
    cachedInputPerM: v.pricing?.cachedInputPerM ?? 0,
    outputPerM: v.pricing?.outputPerM ?? 0,
    currency: v.pricing?.currency ?? 'USD',
  };
}

/**
 * Builds the `PATCH` body from a draft. `apiKey` is included only when the
 * operator actually touched that field this session — see
 * {@link LlmProviderForm} — never just because the draft has a value, since
 * the field starts blank on every load and blank-but-untouched must mean
 * "unchanged", not "clear the stored key". `pricingEnabled: false` sends the
 * zero-rate sentinel the backend reads as "clear the configured pricing".
 */
function buildPatch(draft: LlmDraft, apiKey?: string): LlmSettingsPatch {
  return {
    enabled: draft.enabled,
    baseUrl: draft.baseUrl,
    model: draft.model,
    maxTokens: draft.maxTokens,
    maxRpm: draft.maxRpm,
    maxRetries: draft.maxRetries,
    retryBaseMs: draft.retryBaseMs,
    compressAfter: draft.compressAfter,
    compressKeep: draft.compressKeep,
    compressMaxTokens: draft.compressMaxTokens,
    pricing: draft.pricingEnabled
      ? {
          inputPerM: draft.inputPerM,
          // Mirrors the original env-var default: an unset (zero) cache rate
          // falls back to the fresh-input rate rather than under-pricing.
          cachedInputPerM: draft.cachedInputPerM || draft.inputPerM,
          outputPerM: draft.outputPerM,
          currency: draft.currency.trim() || 'USD',
        }
      : { inputPerM: 0, cachedInputPerM: 0, outputPerM: 0, currency: '' },
    ...(apiKey !== undefined && { apiKey }),
  };
}

/**
 * Operator config for the real-model provider — endpoint, key, tuning,
 * pricing. Only meaningful against a real backend (configuring a backend's
 * LLM without a backend is a contradiction), so this renders a hint instead
 * of a form when the app is on the in-browser mock.
 */
function LlmProviderSettings() {
  const { t } = useTranslation();
  const backendUrl = useConnection((s) => s.backendUrl);

  return (
    <Stack gap={4}>
      <Title order={6}>{t('settings.llmTitle')}</Title>
      {backendUrl ? (
        <LlmProviderForm backendUrl={backendUrl} />
      ) : (
        <Text size="xs" c="dimmed">
          {t('settings.llmNeedsBackend')}
        </Text>
      )}
    </Stack>
  );
}

function LlmProviderForm({ backendUrl }: { backendUrl: string }) {
  const { t } = useTranslation();
  const readOnly = useReadOnly((s) => s.readOnly);
  const [draft, setDraft] = useState<LlmDraft | null>(null);
  // The API key field: always blank on load (the server never sends the
  // stored key back), and tracked separately from `draft` so "the operator
  // never touched this field" can be told apart from "touched it and left it
  // blank" — see `buildPatch`.
  const [apiKeyDraft, setApiKeyDraft] = useState('');
  const [apiKeyTouched, setApiKeyTouched] = useState(false);
  const [hasApiKey, setHasApiKey] = useState(false);
  const [status, setStatus] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    setDraft(null);
    void getLlmSettings(backendUrl).then(
      (view) => {
        if (!active) return;
        setDraft(draftFromView(view));
        setHasApiKey(view.hasApiKey);
        setApiKeyDraft('');
        setApiKeyTouched(false);
        setStatus('idle');
      },
      (e: unknown) => {
        if (active) setError(e instanceof Error ? e.message : String(e));
      },
    );
    return () => {
      active = false;
    };
  }, [backendUrl]);

  const set = <K extends keyof LlmDraft>(key: K, value: LlmDraft[K]) => {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
    if (status === 'saved') setStatus('idle');
  };

  const save = () => {
    if (!draft) return;
    setStatus('saving');
    setError('');
    void updateLlmSettings(
      backendUrl,
      buildPatch(draft, apiKeyTouched ? apiKeyDraft : undefined),
    ).then(
      (view) => {
        setDraft(draftFromView(view));
        setHasApiKey(view.hasApiKey);
        setApiKeyDraft('');
        setApiKeyTouched(false);
        setStatus('saved');
      },
      (e: unknown) => {
        setError(e instanceof Error ? e.message : String(e));
        setStatus('error');
      },
    );
  };

  if (!draft) {
    return error ? (
      <Alert color="red" variant="light" py={6}>
        <Text size="xs">{error}</Text>
      </Alert>
    ) : (
      <Text size="xs" c="dimmed">
        …
      </Text>
    );
  }

  return (
    <Stack gap="sm">
      <Switch
        checked={draft.enabled}
        onChange={(e) => set('enabled', e.currentTarget.checked)}
        disabled={readOnly}
        label={t('settings.llmEnable')}
        description={t('settings.llmEnableHint')}
      />
      <TextInput
        label={t('settings.llmBaseUrl')}
        description={t('settings.llmBaseUrlHint')}
        placeholder={t('settings.llmBaseUrlPlaceholder')}
        value={draft.baseUrl}
        onChange={(e) => set('baseUrl', e.currentTarget.value)}
        disabled={readOnly}
      />
      <TextInput
        label={t('settings.llmModel')}
        placeholder={t('settings.llmModelPlaceholder')}
        value={draft.model}
        onChange={(e) => set('model', e.currentTarget.value)}
        disabled={readOnly}
      />
      <PasswordInput
        label={t('settings.llmApiKey')}
        description={hasApiKey ? t('settings.llmApiKeyStored') : t('settings.llmApiKeyNotStored')}
        placeholder={t('settings.llmApiKeyPlaceholder')}
        value={apiKeyDraft}
        onChange={(e) => {
          setApiKeyDraft(e.currentTarget.value);
          setApiKeyTouched(true);
          if (status === 'saved') setStatus('idle');
        }}
        disabled={readOnly}
      />

      <Accordion variant="filled" chevronPosition="left">
        <Accordion.Item value="advanced">
          <Accordion.Control py={4} px={0}>
            <Text size="xs" c="dimmed">
              {t('settings.llmAdvanced')}
            </Text>
          </Accordion.Control>
          <Accordion.Panel>
            <Stack gap="sm" pt={4}>
              <Group grow>
                <NumberInput
                  label={t('settings.llmMaxTokens')}
                  min={1}
                  value={draft.maxTokens}
                  onChange={(v) => set('maxTokens', Number(v) || 0)}
                  disabled={readOnly}
                />
                <NumberInput
                  label={t('settings.llmMaxRpm')}
                  description={t('settings.llmMaxRpmHint')}
                  min={0}
                  value={draft.maxRpm}
                  onChange={(v) => set('maxRpm', Number(v) || 0)}
                  disabled={readOnly}
                />
              </Group>
              <Group grow>
                <NumberInput
                  label={t('settings.llmMaxRetries')}
                  min={0}
                  value={draft.maxRetries}
                  onChange={(v) => set('maxRetries', Number(v) || 0)}
                  disabled={readOnly}
                />
                <NumberInput
                  label={t('settings.llmRetryBaseMs')}
                  min={0}
                  value={draft.retryBaseMs}
                  onChange={(v) => set('retryBaseMs', Number(v) || 0)}
                  disabled={readOnly}
                />
              </Group>
              <Group grow>
                <NumberInput
                  label={t('settings.llmCompressAfter')}
                  description={t('settings.llmCompressAfterHint')}
                  min={0}
                  value={draft.compressAfter}
                  onChange={(v) => set('compressAfter', Number(v) || 0)}
                  disabled={readOnly}
                />
                <NumberInput
                  label={t('settings.llmCompressKeep')}
                  min={0}
                  value={draft.compressKeep}
                  onChange={(v) => set('compressKeep', Number(v) || 0)}
                  disabled={readOnly}
                />
              </Group>
              <NumberInput
                label={t('settings.llmCompressMaxTokens')}
                min={1}
                value={draft.compressMaxTokens}
                onChange={(v) => set('compressMaxTokens', Number(v) || 0)}
                disabled={readOnly}
              />

              <Switch
                checked={draft.pricingEnabled}
                onChange={(e) => set('pricingEnabled', e.currentTarget.checked)}
                disabled={readOnly}
                label={t('settings.llmPricingEnable')}
                description={t('settings.llmPricingHint')}
              />
              {draft.pricingEnabled && (
                <Stack gap="sm">
                  <Group grow>
                    <NumberInput
                      label={t('settings.llmPricingInput')}
                      min={0}
                      decimalScale={4}
                      value={draft.inputPerM}
                      onChange={(v) => set('inputPerM', Number(v) || 0)}
                      disabled={readOnly}
                    />
                    <NumberInput
                      label={t('settings.llmPricingCachedInput')}
                      description={t('settings.llmPricingCachedInputHint')}
                      min={0}
                      decimalScale={4}
                      value={draft.cachedInputPerM}
                      onChange={(v) => set('cachedInputPerM', Number(v) || 0)}
                      disabled={readOnly}
                    />
                  </Group>
                  <Group grow>
                    <NumberInput
                      label={t('settings.llmPricingOutput')}
                      min={0}
                      decimalScale={4}
                      value={draft.outputPerM}
                      onChange={(v) => set('outputPerM', Number(v) || 0)}
                      disabled={readOnly}
                    />
                    <TextInput
                      label={t('settings.llmPricingCurrency')}
                      value={draft.currency}
                      onChange={(e) => set('currency', e.currentTarget.value)}
                      disabled={readOnly}
                    />
                  </Group>
                </Stack>
              )}
            </Stack>
          </Accordion.Panel>
        </Accordion.Item>
      </Accordion>

      {error && (
        <Alert color="red" variant="light" py={6}>
          <Text size="xs">{error}</Text>
        </Alert>
      )}
      <Group justify="flex-end" align="center" gap="xs">
        {status === 'saved' && (
          <Text size="xs" c="teal">
            {t('settings.llmSaved')}
          </Text>
        )}
        <Button size="xs" onClick={save} loading={status === 'saving'} disabled={readOnly}>
          {t('settings.llmSave')}
        </Button>
      </Group>
    </Stack>
  );
}
