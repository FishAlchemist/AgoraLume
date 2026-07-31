import {
  Accordion,
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
import { workspaceClient } from '../lib/api/workspace';
import { useBackendStatus } from '../lib/useBackendStatus';
import { useConnection } from '../store/connection';
import type { Persona } from '../types';
import { CopyIconButton } from './CopyIconButton';
import { UsageSummary } from './UsageSummary';

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
  // Persona identity-hash → git-tag-style name, so a trace shows "溫柔版" rather
  // than a raw hash. Fetched once per backend bind; labels change rarely.
  const [labels, setLabels] = useState<Record<string, string>>({});
  // Traces carry no server id, so tag each with a monotonic client id for a
  // stable React key and accordion value.
  const [entries, setEntries] = useState<{ id: number; trace: AgentTrace }[]>([]);
  const nextId = useRef(0);
  // Refetch the totals on each new trace, coalescing bursts into one request.
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    let active = true;
    setUsage(null);
    setLabels({});
    setEntries([]);
    nextId.current = 0;

    const loadUsage = () => {
      void api.getGroupUsage(groupId).then((u) => {
        if (active) setUsage(u);
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

  return (
    <Paper withBorder radius="md" p="sm" m="md" mb={0}>
      <Stack gap="xs">
        <UsageSummary usage={usage} mock={status.mock} title={t('debug.groupUsage')} />

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
  labels,
}: {
  value: string;
  trace: AgentTrace;
  personas: Map<string, Persona>;
  labels: Record<string, string>;
}) {
  const { t } = useTranslation();
  const persona = personas.get(trace.personaId);
  const name = persona?.name ?? trace.personaName;
  const hash = persona?.promptHash;
  const versionLabel = hash ? labels[hash] : undefined;
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
              label={t('debug.context')}
              text={trace.conversation.trim()}
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
