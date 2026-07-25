import {
  Accordion,
  Alert,
  Badge,
  Code,
  Divider,
  Group,
  Paper,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
} from '@mantine/core';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '../lib/api';
import type { AgentTrace, DebugUsage } from '../lib/api/types';
import { useBackendStatus } from '../lib/useBackendStatus';
import { useConnection } from '../store/connection';
import type { Persona } from '../types';

interface Props {
  groupId: string;
  personas: Map<string, Persona>;
}

const fmt = (n: number) => n.toLocaleString();
const pct = (ratio: number) => `${(ratio * 100).toFixed(1)}%`;

/** One labelled figure in the usage summary. */
function Stat({ label, value }: { label: string; value: string }) {
  return (
    <Stack gap={0} miw={64}>
      <Text size="xs" c="dimmed">
        {label}
      </Text>
      <Text fw={600} ff="monospace">
        {value}
      </Text>
    </Stack>
  );
}

/**
 * A collapsible debug panel for a group: the backend's cumulative LLM usage
 * (requests, tokens, cache-hit ratio, estimated cost) plus the recent prompts
 * each character received and what it decided. Live via the group's `debug` SSE
 * frames. Harmless against the mock (shows zeros and a note).
 */
export function DebugPanel({ groupId, personas }: Props) {
  const { t } = useTranslation();
  const backendUrl = useConnection((s) => s.backendUrl);
  const status = useBackendStatus();
  const [usage, setUsage] = useState<DebugUsage | null>(null);
  // Traces carry no server id, so tag each with a monotonic client id for a
  // stable React key and accordion value.
  const [entries, setEntries] = useState<{ id: number; trace: AgentTrace }[]>([]);
  const nextId = useRef(0);
  // Refetch the totals on each new trace, coalescing bursts into one request.
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: backendUrl re-binds the panel when the data source changes.
  useEffect(() => {
    let active = true;
    setUsage(null);
    setEntries([]);
    nextId.current = 0;

    const loadUsage = () => {
      void api.getUsage().then((u) => {
        if (active) setUsage(u);
      });
    };
    loadUsage();
    void api.listTraces(groupId).then((initial) => {
      if (!active) return;
      setEntries(initial.map((trace) => ({ id: nextId.current++, trace })));
    });

    const unsubscribe = api.subscribeDebug(groupId, (trace) => {
      const id = nextId.current++;
      setEntries((prev) => [...prev, { id, trace }]);
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
      refreshTimer.current = setTimeout(loadUsage, 200);
    });

    return () => {
      active = false;
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
      unsubscribe();
    };
  }, [groupId, backendUrl]);

  const cost = usage?.estimatedCost;

  return (
    <Paper withBorder radius="md" p="sm" m="md" mb={0}>
      <Stack gap="xs">
        <Group justify="space-between" align="center">
          <Text fw={700} size="sm">
            {t('debug.usage')}
          </Text>
          {usage && usage.promptTokens > 0 && (
            <Tooltip label={t('debug.cacheHit')}>
              <Badge variant="light" color="teal">
                {t('debug.cacheHit')} {pct(usage.cacheHitRatio)}
              </Badge>
            </Tooltip>
          )}
        </Group>

        <Group gap="lg" wrap="wrap">
          <Stat label={t('debug.requests')} value={fmt(usage?.requests ?? 0)} />
          <Stat label={t('debug.inputTokens')} value={fmt(usage?.promptTokens ?? 0)} />
          <Stat label={t('debug.outputTokens')} value={fmt(usage?.completionTokens ?? 0)} />
          <Stat label={t('debug.totalTokens')} value={fmt(usage?.totalTokens ?? 0)} />
          <Stat label={t('debug.cached')} value={fmt(usage?.cachedPromptTokens ?? 0)} />
        </Group>

        {cost ? (
          <Tooltip
            multiline
            w={240}
            label={`${t('debug.costHint')}\n${cost.currency} · in ${cost.input.toFixed(4)} · cached ${cost.cachedInput.toFixed(4)} · out ${cost.output.toFixed(4)}`}
          >
            <Text size="sm">
              {t('debug.cost')}:{' '}
              <Text span fw={700} ff="monospace">
                {cost.total.toFixed(4)} {cost.currency}
              </Text>{' '}
              <Text span size="xs" c="dimmed">
                ({t('debug.costHint')})
              </Text>
            </Text>
          </Tooltip>
        ) : (
          <Text size="xs" c="dimmed">
            {t('debug.noCost')}
          </Text>
        )}

        {status.mock && (
          <Alert color="yellow" variant="light" py={6}>
            <Text size="xs">{t('debug.mockNote')}</Text>
          </Alert>
        )}

        <Divider />

        <Group gap={6} align="baseline">
          <Text fw={700} size="sm">
            {t('debug.traces')}
          </Text>
          <Text size="xs" c="dimmed">
            {t('debug.tracesHint')}
          </Text>
        </Group>

        {entries.length === 0 ? (
          <Text size="xs" c="dimmed">
            {t('debug.empty')}
          </Text>
        ) : (
          <ScrollArea.Autosize mah={320}>
            <Accordion variant="separated" chevronPosition="left">
              {[...entries].reverse().map(({ id, trace }) => (
                <TraceItem key={id} value={String(id)} trace={trace} personas={personas} />
              ))}
            </Accordion>
          </ScrollArea.Autosize>
        )}
      </Stack>
    </Paper>
  );
}

/** One trace row: who, what action, token cost — expanding to the full prompt. */
function TraceItem({
  value,
  trace,
  personas,
}: {
  value: string;
  trace: AgentTrace;
  personas: Map<string, Persona>;
}) {
  const { t } = useTranslation();
  const name = personas.get(trace.personaId)?.name ?? trace.personaName;
  const spoke = trace.message ?? (trace.mood ? '' : undefined);

  return (
    <Accordion.Item value={value}>
      <Accordion.Control>
        <Group gap="xs" wrap="nowrap">
          <Text fw={600} size="sm" truncate>
            {name}
          </Text>
          <Badge size="xs" variant="light">
            {trace.action}
          </Badge>
          {trace.usage && (
            <Text size="xs" c="dimmed" ff="monospace">
              {trace.usage.promptTokens}→{trace.usage.completionTokens}
            </Text>
          )}
        </Group>
      </Accordion.Control>
      <Accordion.Panel>
        <Stack gap={6}>
          <Text size="xs" fw={600} c="dimmed">
            {t('debug.system')}
          </Text>
          <Code block fz="xs">
            {trace.system.trim()}
          </Code>
          <Text size="xs" fw={600} c="dimmed">
            {t('debug.context')}
          </Text>
          <Code block fz="xs">
            {trace.conversation.trim()}
          </Code>
          <Text size="xs" fw={600} c="dimmed">
            {t('debug.decision')}
          </Text>
          <Text size="sm">
            {trace.mood && (
              <Text span mr={6}>
                {trace.mood}
              </Text>
            )}
            {spoke !== undefined && spoke !== '' ? (
              <Text span>{spoke}</Text>
            ) : (
              !trace.mood && (
                <Text span c="dimmed" fs="italic">
                  {t('debug.noReply')}
                </Text>
              )
            )}
          </Text>
        </Stack>
      </Accordion.Panel>
    </Accordion.Item>
  );
}
