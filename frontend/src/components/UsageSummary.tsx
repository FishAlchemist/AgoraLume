import { Accordion, Alert, Badge, Code, Group, Stack, Text, Tooltip } from '@mantine/core';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { DebugUsage, ModelUsage } from '../lib/api/types';

const fmt = (n: number) => n.toLocaleString();
const pct = (ratio: number) => `${(ratio * 100).toFixed(1)}%`;

/** One labelled figure in the detail grid. */
function Stat({ label, value }: { label: string; value: string }) {
  return (
    <Stack gap={0} miw={64}>
      <Text size="xs" c="dimmed">
        {label}
      </Text>
      <Text size="sm" fw={600} ff="monospace">
        {value}
      </Text>
    </Stack>
  );
}

/** One model's row in the by-model breakdown. */
function ModelRow({ entry }: { entry: ModelUsage }) {
  const { t } = useTranslation();
  const cost = entry.estimatedCost;
  const label = entry.model === 'unknown' ? t('debug.unknownModel') : entry.model;
  return (
    <Group gap="xs" wrap="nowrap" justify="space-between">
      <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
        <Code fz="xs">{label}</Code>
        <Text size="xs" c="dimmed" ff="monospace" truncate>
          {fmt(entry.requests)}× · {fmt(entry.promptTokens)}→{fmt(entry.completionTokens)}
        </Text>
      </Group>
      <Text size="xs" c="dimmed" ff="monospace">
        {cost ? `${cost.total.toFixed(4)} ${cost.currency}` : '—'}
      </Text>
    </Group>
  );
}

interface Props {
  /** The cumulative usage snapshot from `GET /debug/usage`, or `null` while loading. */
  usage: DebugUsage | null;
  /** Shows a note that no LLM calls (and so no real cost) are happening. */
  mock?: boolean;
  /**
   * Overrides the small caps label above the headline number; defaults to
   * `debug.usage` ("Total usage"). Pass `null` to omit the label entirely —
   * for a caller (e.g. the Settings page) that already has its own heading
   * directly above this component.
   */
  title?: ReactNode | null;
}

/**
 * The LLM cost readout, shared by the per-group debug panel and the Settings
 * page. Deliberately top-heavy: the one number that matters (the estimated
 * running total) is the only thing shown at a glance; everything else —
 * request/token counts, cache-hit ratio, the per-model breakdown — sits behind
 * a collapsed "show details" toggle so it doesn't compete for attention in an
 * app that's fundamentally about the chat, not the billing.
 */
export function UsageSummary({ usage, mock, title }: Props) {
  const { t } = useTranslation();
  const cost = usage?.estimatedCost;
  const models = usage?.models ?? [];

  return (
    <Stack gap={4}>
      <Group justify="space-between" align="flex-end" wrap="nowrap">
        <Stack gap={0}>
          {title !== null && (
            <Text size="xs" c="dimmed" tt="uppercase" fw={700}>
              {title ?? t('debug.usage')}
            </Text>
          )}
          {cost ? (
            <Text fw={800} ff="monospace" style={{ fontSize: 'var(--mantine-font-size-xl)' }}>
              {cost.total.toFixed(4)}{' '}
              <Text span size="sm" fw={600} c="dimmed">
                {cost.currency}
              </Text>
            </Text>
          ) : (
            <Text size="sm" c="dimmed">
              {t('debug.noCost')}
            </Text>
          )}
        </Stack>
        {usage && usage.promptTokens > 0 && (
          <Tooltip label={t('debug.cacheHit')}>
            <Badge variant="light" color="teal">
              {t('debug.cacheHit')} {pct(usage.cacheHitRatio)}
            </Badge>
          </Tooltip>
        )}
      </Group>
      <Text size="xs" c="dimmed">
        {t('debug.costHint')}
      </Text>

      <Accordion variant="filled" chevronPosition="left">
        <Accordion.Item value="details">
          <Accordion.Control py={4} px={0}>
            <Text size="xs" c="dimmed">
              {t('debug.showDetails')}
            </Text>
          </Accordion.Control>
          <Accordion.Panel>
            <Stack gap="sm" pt={4}>
              <Group gap="lg" wrap="wrap">
                <Stat label={t('debug.requests')} value={fmt(usage?.requests ?? 0)} />
                <Stat label={t('debug.inputTokens')} value={fmt(usage?.promptTokens ?? 0)} />
                <Stat label={t('debug.outputTokens')} value={fmt(usage?.completionTokens ?? 0)} />
                <Stat label={t('debug.totalTokens')} value={fmt(usage?.totalTokens ?? 0)} />
                <Stat label={t('debug.cached')} value={fmt(usage?.cachedPromptTokens ?? 0)} />
              </Group>

              {models.length > 0 && (
                <Stack gap={4}>
                  <Text size="xs" fw={600} c="dimmed">
                    {t('debug.byModel')}
                  </Text>
                  {models.map((entry) => (
                    <ModelRow key={entry.model} entry={entry} />
                  ))}
                </Stack>
              )}

              {mock && (
                <Alert color="yellow" variant="light" py={6}>
                  <Text size="xs">{t('debug.mockNote')}</Text>
                </Alert>
              )}
            </Stack>
          </Accordion.Panel>
        </Accordion.Item>
      </Accordion>
    </Stack>
  );
}
