import {
  Accordion,
  ActionIcon,
  Badge,
  Code,
  Divider,
  Group,
  Pagination,
  Paper,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
} from '@mantine/core';
import { IconDownload } from '@tabler/icons-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '../lib/api';
import type { AgentTrace, DebugUsage, PersonaUsage } from '../lib/api/types';
import { workspaceClient } from '../lib/api/workspace';
import { useBackendStatus } from '../lib/useBackendStatus';
import { useConnection } from '../store/connection';
import type { Persona } from '../types';
import { CopyIconButton } from './CopyIconButton';
import { UsageSummary } from './UsageSummary';

const fmt = (n: number) => n.toLocaleString();

/** Traces per page — rendering all of them at once is what made opening a busy
 * group's panel janky (each row's prompt/context text is not cheap to mount). */
const TRACES_PER_PAGE = 5;

/** "850ms" below 1s, else "1.2s" — coarse enough to spot a slow call at a glance. */
function formatDuration(ms: number | null | undefined): string | null {
  if (ms == null) return null;
  return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
}

/** Date + clock time for the trace title — traces can span multiple days, so a
 * bare "14:23:05" isn't enough to tell today's from yesterday's. Full local
 * date-time lives in the hover title regardless. */
function formatTraceTime(ts: number, locale: string): string {
  return new Date(ts).toLocaleString(locale, {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

interface Props {
  groupId: string;
  personas: Map<string, Persona>;
}

/** First 8 hex chars — enough to tell identity versions apart at a glance. */
const shortHash = (hash: string) => hash.slice(0, 8);

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
  const [personaUsage, setPersonaUsage] = useState<PersonaUsage[]>([]);
  // Persona identity-hash → git-tag-style name, so a trace shows "溫柔版" rather
  // than a raw hash. Fetched once per backend bind; labels change rarely.
  const [labels, setLabels] = useState<Record<string, string>>({});
  // Traces carry no server id, so tag each with a monotonic client id for a
  // stable React key and accordion value.
  const [entries, setEntries] = useState<{ id: number; trace: AgentTrace }[]>([]);
  const [tracePage, setTracePage] = useState(1);
  const nextId = useRef(0);
  // Refetch the totals on each new trace, coalescing bursts into one request.
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    let active = true;
    setUsage(null);
    setPersonaUsage([]);
    setLabels({});
    setEntries([]);
    setTracePage(1);
    nextId.current = 0;

    const loadUsage = () => {
      void api.getGroupUsage(groupId).then((u) => {
        if (active) setUsage(u);
      });
      void api.getPersonaUsage(groupId).then((list) => {
        if (active) setPersonaUsage(list);
      });
    };
    loadUsage();
    void api.listTraces(groupId).then((initial) => {
      if (!active) return;
      setEntries(initial.map((trace) => ({ id: nextId.current++, trace })));
    });
    // Prompt labels live on the workspace API (not the chat routing client); the
    // mock has none, so only fetch against a real backend.
    if (backendUrl) {
      void workspaceClient(backendUrl)
        .listPromptLabels()
        .then((list) => {
          if (active) setLabels(Object.fromEntries(list.map((l) => [l.hash, l.label])));
        })
        .catch(() => {});
    }

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

  // Downloads the currently loaded traces (capped server-side, newest window
  // only) as a JSON file — a snapshot for offline inspection or sharing, not a
  // full-history export.
  const exportTraces = () => {
    const blob = new Blob(
      [
        JSON.stringify(
          entries.map((e) => e.trace),
          null,
          2,
        ),
      ],
      { type: 'application/json' },
    );
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `agoralume-debug-${groupId}-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  // Only members with at least one recorded inference — a group that hasn't
  // run a turn yet shouldn't show every member at a flat zero.
  const activePersonaUsage = personaUsage.filter((entry) => entry.usage.requests > 0);

  // Newest first, sliced to one page — a group can run 20+ turns, and mounting
  // every trace's prompt/context text at once is what made opening the panel
  // janky. Always paginated (even a single page) so the control's position
  // doesn't jump around as history grows.
  const reversed = [...entries].reverse();
  const totalPages = Math.max(1, Math.ceil(reversed.length / TRACES_PER_PAGE));
  const currentPage = Math.min(tracePage, totalPages);
  const pageEntries = reversed.slice(
    (currentPage - 1) * TRACES_PER_PAGE,
    currentPage * TRACES_PER_PAGE,
  );

  return (
    <Paper withBorder radius="md" p="sm" m="md" mb={0}>
      <Stack gap="xs">
        <UsageSummary usage={usage} mock={status.mock} title={t('debug.groupUsage')} />

        {activePersonaUsage.length > 0 && (
          <Accordion variant="separated" chevronPosition="left">
            {/* Collapsed by default — a group can have dozens of characters, so
                folding this away keeps the always-visible billing summary short. */}
            <Accordion.Item value="by-persona">
              <Accordion.Control>
                <Text size="xs" fw={600} c="dimmed">
                  {t('debug.byPersona')} ({activePersonaUsage.length})
                </Text>
              </Accordion.Control>
              <Accordion.Panel>
                <ScrollArea.Autosize mah={200}>
                  <Stack gap={4}>
                    {activePersonaUsage.map((entry) => (
                      <PersonaUsageRow key={entry.personaId} entry={entry} personas={personas} />
                    ))}
                  </Stack>
                </ScrollArea.Autosize>
              </Accordion.Panel>
            </Accordion.Item>
          </Accordion>
        )}

        <Divider />

        <Group gap={6} align="baseline" justify="space-between" wrap="nowrap">
          <Group gap={6} align="baseline">
            <Text fw={700} size="sm">
              {t('debug.traces')}
            </Text>
            <Text size="xs" c="dimmed">
              {t('debug.tracesHint')}
            </Text>
          </Group>
          {entries.length > 0 && (
            <Tooltip label={t('debug.exportTitle')} withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                onClick={exportTraces}
                aria-label={t('debug.export')}
              >
                <IconDownload size={16} />
              </ActionIcon>
            </Tooltip>
          )}
        </Group>

        {entries.length === 0 ? (
          <Text size="xs" c="dimmed">
            {t('debug.empty')}
          </Text>
        ) : (
          <>
            <ScrollArea.Autosize mah={320}>
              <Accordion variant="separated" chevronPosition="left">
                {pageEntries.map(({ id, trace }) => (
                  <TraceItem
                    key={id}
                    value={String(id)}
                    trace={trace}
                    personas={personas}
                    labels={labels}
                  />
                ))}
              </Accordion>
            </ScrollArea.Autosize>
            <Group justify="center">
              <Pagination
                size="xs"
                value={currentPage}
                onChange={setTracePage}
                total={totalPages}
              />
            </Group>
          </>
        )}
      </Stack>
    </Paper>
  );
}

/** One character's row in the by-persona usage breakdown. */
function PersonaUsageRow({
  entry,
  personas,
}: {
  entry: PersonaUsage;
  personas: Map<string, Persona>;
}) {
  const persona = personas.get(entry.personaId);
  const name = persona?.name ?? entry.personaId;
  const cost = entry.usage.estimatedCost;
  return (
    <Group gap="xs" wrap="nowrap" justify="space-between">
      <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
        <Text size="xs" fw={600} truncate>
          {name}
        </Text>
        <Text size="xs" c="dimmed" ff="monospace" truncate>
          {fmt(entry.usage.requests)}× · {fmt(entry.usage.promptTokens)}→
          {fmt(entry.usage.completionTokens)}
        </Text>
      </Group>
      <Text size="xs" c="dimmed" ff="monospace">
        {cost ? `${cost.total.toFixed(4)} ${cost.currency}` : '—'}
      </Text>
    </Group>
  );
}

/** One trace row: who, what action, token cost — expanding to the full prompt. */
function TraceItem({
  value,
  trace,
  personas,
  labels,
}: {
  value: string;
  trace: AgentTrace;
  personas: Map<string, Persona>;
  labels: Record<string, string>;
}) {
  const { t, i18n } = useTranslation();
  const persona = personas.get(trace.personaId);
  const name = persona?.name ?? trace.personaName;
  const hash = persona?.promptHash;
  const versionLabel = hash ? labels[hash] : undefined;
  const spoke = trace.message ?? (trace.mood ? '' : undefined);
  const duration = formatDuration(trace.durationMs);
  const time = formatTraceTime(trace.ts, i18n.language);
  const fullTime = new Date(trace.ts).toLocaleString(i18n.language);
  const contextText = trace.conversation.trim();

  return (
    <Accordion.Item value={value}>
      <Accordion.Control>
        <Stack gap={2}>
          <Group gap="xs" wrap="nowrap">
            <Text fw={600} size="sm" truncate>
              {name}
            </Text>
            <Badge size="xs" variant="light">
              {trace.action}
            </Badge>
          </Group>
          <Group gap={8} wrap="wrap">
            <Text size="xs" c="dimmed" title={fullTime}>
              {time}
            </Text>
            {duration && (
              <Text size="xs" c="dimmed" ff="monospace">
                {duration}
              </Text>
            )}
            {trace.usage && (
              <Text size="xs" c="dimmed" ff="monospace">
                {t('debug.inputTokens')} {fmt(trace.usage.promptTokens)} → {t('debug.outputTokens')}{' '}
                {fmt(trace.usage.completionTokens)}
              </Text>
            )}
          </Group>
        </Stack>
      </Accordion.Control>
      <Accordion.Panel>
        <Stack gap={6}>
          {/* The system prompt is near-identical across a character's traces in a
              turn, so we don't re-render it each time: show its identity version
              (named label, else short hash) and offer the full text on demand via
              copy. Context genuinely varies per trace, so it stays collapsible. */}
          <Group gap="xs" wrap="nowrap" align="center">
            <Text size="xs" fw={600} c="dimmed">
              {t('debug.system')}
            </Text>
            {hash ? (
              <Tooltip label={hash} withArrow>
                {versionLabel ? (
                  <Badge size="xs" variant="light">
                    {versionLabel}
                  </Badge>
                ) : (
                  <Code fz="xs">{shortHash(hash)}</Code>
                )}
              </Tooltip>
            ) : (
              <Text size="xs" c="dimmed">
                {t('debug.systemUnknown')}
              </Text>
            )}
            <CopyIconButton value={trace.system.trim()} />
          </Group>
          <Accordion variant="contained" chevronPosition="left">
            <CollapsibleText
              value="context"
              label={`${t('debug.context')} · ${t('debug.charCount', { n: fmt(contextText.length) })}`}
              text={contextText}
            />
          </Accordion>
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

/**
 * The collapsible context section. Default collapsed; when open, the text is
 * height-bounded so a long transcript can't blow out the panel, with a copy
 * button so the full text is one click away.
 */
function CollapsibleText({ value, label, text }: { value: string; label: string; text: string }) {
  return (
    <Accordion.Item value={value}>
      <Accordion.Control>
        <Text size="xs" fw={600} c="dimmed">
          {label}
        </Text>
      </Accordion.Control>
      <Accordion.Panel>
        <Group align="flex-start" gap="xs" wrap="nowrap">
          <ScrollArea.Autosize mah={200} style={{ flex: 1, minWidth: 0 }}>
            <Code block fz="xs">
              {text}
            </Code>
          </ScrollArea.Autosize>
          <CopyIconButton value={text} />
        </Group>
      </Accordion.Panel>
    </Accordion.Item>
  );
}
