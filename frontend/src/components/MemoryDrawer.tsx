import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Code,
  Drawer,
  Group,
  Loader,
  Stack,
  Text,
  Textarea,
  TextInput,
  Tooltip,
} from '@mantine/core';
import {
  IconAlertTriangle,
  IconBrain,
  IconInfoCircle,
  IconPencil,
  IconTrash,
} from '@tabler/icons-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { workspaceClient } from '../lib/api/workspace';
import { useConnection } from '../store/connection';
import { useReadOnly } from '../store/readonly';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';
import type { Memory } from '../types';

/** First 8 hex chars — enough to tell identity versions apart at a glance. */
const shortHash = (hash: string) => hash.slice(0, 8);

/** A localized, compact "when" for a memory: relative if recent, else a date. */
function formatWhen(ts: number, locale: string): string {
  const diff = Date.now() - ts;
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
  if (diff < hour) return rtf.format(-Math.round(diff / minute), 'minute');
  if (diff < day) return rtf.format(-Math.round(diff / hour), 'hour');
  if (diff < 7 * day) return rtf.format(-Math.round(diff / day), 'day');
  return new Date(ts).toLocaleDateString(locale, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

/**
 * Per-persona memory manager: browse everything a character remembers, grouped
 * by which identity version recorded it, and add or forget individual memories.
 *
 * The grouping is the whole point of the identity hash: a persona's *current*
 * version is shown first as live memory, while anything an earlier version wrote
 * is kept but marked "past" — recall holds it out of character rather than
 * deleting it. Each version can be given a git-tag-style name so the groups read
 * as "溫柔版" / "毒舌版" instead of raw hashes.
 *
 * Memory lives in the backend SSOT (no agent loop means the mock records none),
 * so this reads on demand instead of through the workspace store.
 */
export function MemoryDrawer() {
  const { t, i18n } = useTranslation();
  const personaId = useUi((s) => s.memoryPersonaId);
  const closeMemory = useUi((s) => s.closeMemory);
  const askConfirm = useUi((s) => s.askConfirm);
  const readOnly = useReadOnly((s) => s.readOnly);
  const backendUrl = useConnection((s) => s.backendUrl);
  const persona = useWorkspace((s) => s.personas.find((p) => p.id === personaId));

  const client = backendUrl ? workspaceClient(backendUrl) : null;
  const currentHash = persona?.promptHash;

  const [memories, setMemories] = useState<Memory[]>([]);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(false);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);

  const reload = useCallback(async () => {
    if (!client || !personaId) return;
    setLoading(true);
    setError(false);
    try {
      const [mems, labelList] = await Promise.all([
        client.listMemories(personaId),
        client.listPromptLabels(),
      ]);
      setMemories(mems);
      setLabels(Object.fromEntries(labelList.map((l) => [l.hash, l.label])));
    } catch {
      setError(true);
    } finally {
      setLoading(false);
    }
  }, [client, personaId]);

  // Fresh read every time the drawer opens for a persona; clear on close.
  useEffect(() => {
    if (!personaId) {
      setMemories([]);
      setDraft('');
      return;
    }
    void reload();
  }, [personaId, reload]);

  // Current identity first, then older versions by their most recent memory.
  const groups = useMemo(() => {
    const byHash = new Map<string, Memory[]>();
    for (const m of memories) {
      const list = byHash.get(m.promptHash);
      if (list) list.push(m);
      else byHash.set(m.promptHash, [m]);
    }
    return [...byHash.entries()].sort((a, b) => {
      if (a[0] === currentHash) return -1;
      if (b[0] === currentHash) return 1;
      return (b[1][0]?.createdAt ?? 0) - (a[1][0]?.createdAt ?? 0);
    });
  }, [memories, currentHash]);

  const handleAdd = async () => {
    const content = draft.trim();
    if (!client || !personaId || !content) return;
    setSaving(true);
    try {
      const created = await client.createMemory(personaId, content);
      setMemories((prev) => [created, ...prev]);
      setDraft('');
    } catch {
      void reload();
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = (memory: Memory) => {
    if (!client || !personaId) return;
    askConfirm({
      message: t('memory.confirmDelete'),
      confirmLabel: t('common.delete'),
      danger: true,
      onConfirm: () => {
        setMemories((prev) => prev.filter((m) => m.id !== memory.id));
        client.deleteMemory(personaId, memory.id).catch(() => void reload());
      },
    });
  };

  const handleLabel = async (hash: string, label: string) => {
    if (!client) return;
    const trimmed = label.trim();
    // Optimistic: reflect the rename immediately, reconcile from the response.
    setLabels((prev) => {
      const next = { ...prev };
      if (trimmed) next[hash] = trimmed;
      else delete next[hash];
      return next;
    });
    try {
      await client.setPromptLabel(hash, trimmed);
    } catch {
      void reload();
    }
  };

  const canWrite = Boolean(client) && !readOnly;
  const hasPrompt = Boolean(currentHash);

  return (
    <Drawer
      opened={Boolean(persona)}
      onClose={closeMemory}
      position="right"
      size="md"
      title={
        <Group gap={8}>
          <IconBrain size={20} />
          <Text fw={700}>{persona ? t('memory.titleFor', { name: persona.name }) : ''}</Text>
        </Group>
      }
    >
      {persona && (
        <Stack gap="md">
          <Text size="sm" c="dimmed">
            {t('memory.subtitle')}
          </Text>

          {!client ? (
            <Alert icon={<IconInfoCircle size={16} />} color="gray" variant="light">
              {t('memory.mockNote')}
            </Alert>
          ) : (
            <>
              {canWrite &&
                (hasPrompt ? (
                  <Stack gap={6}>
                    <Textarea
                      value={draft}
                      onChange={(e) => setDraft(e.currentTarget.value)}
                      placeholder={t('memory.addPlaceholder')}
                      autosize
                      minRows={2}
                      maxRows={5}
                      disabled={saving}
                    />
                    <Group justify="space-between" align="center">
                      <Text size="xs" c="dimmed">
                        {t('memory.addHint')}
                      </Text>
                      <Button
                        size="xs"
                        onClick={handleAdd}
                        loading={saving}
                        disabled={!draft.trim()}
                      >
                        {t('common.add')}
                      </Button>
                    </Group>
                  </Stack>
                ) : (
                  <Alert icon={<IconInfoCircle size={16} />} color="gray" variant="light">
                    {t('memory.noPrompt')}
                  </Alert>
                ))}

              <MemoryList
                loading={loading}
                error={error}
                groups={groups}
                labels={labels}
                currentHash={currentHash}
                color={persona.color}
                locale={i18n.language}
                canWrite={canWrite}
                onRelabel={handleLabel}
                onDelete={handleDelete}
              />
            </>
          )}
        </Stack>
      )}
    </Drawer>
  );
}

interface MemoryListProps {
  loading: boolean;
  error: boolean;
  groups: [string, Memory[]][];
  labels: Record<string, string>;
  currentHash: string | undefined;
  color: string;
  locale: string;
  canWrite: boolean;
  onRelabel: (hash: string, value: string) => void;
  onDelete: (memory: Memory) => void;
}

/** The load/error/empty/grouped states of the memory list, one branch each. */
function MemoryList({
  loading,
  error,
  groups,
  labels,
  currentHash,
  color,
  locale,
  canWrite,
  onRelabel,
  onDelete,
}: MemoryListProps) {
  const { t } = useTranslation();
  if (loading) {
    return (
      <Group justify="center" py="lg">
        <Loader size="sm" />
      </Group>
    );
  }
  if (error) {
    return (
      <Alert icon={<IconAlertTriangle size={16} />} color="red" variant="light">
        {t('memory.loadError')}
      </Alert>
    );
  }
  if (groups.length === 0) {
    return (
      <Text c="dimmed" ta="center" py="lg">
        {t('memory.empty')}
      </Text>
    );
  }
  return (
    <Stack gap="lg">
      {groups.map(([hash, items]) => (
        <VersionGroup
          key={hash}
          hash={hash}
          items={items}
          label={labels[hash]}
          isCurrent={hash === currentHash}
          color={color}
          locale={locale}
          canWrite={canWrite}
          onRelabel={(value) => onRelabel(hash, value)}
          onDelete={onDelete}
        />
      ))}
    </Stack>
  );
}

interface VersionGroupProps {
  hash: string;
  items: Memory[];
  label: string | undefined;
  isCurrent: boolean;
  color: string;
  locale: string;
  canWrite: boolean;
  onRelabel: (value: string) => void;
  onDelete: (memory: Memory) => void;
}

/** One identity version: its name/badge header and the memories it recorded. */
function VersionGroup({
  hash,
  items,
  label,
  isCurrent,
  color,
  locale,
  canWrite,
  onRelabel,
  onDelete,
}: VersionGroupProps) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(label ?? '');

  const startEdit = () => {
    setValue(label ?? '');
    setEditing(true);
  };
  const commit = () => {
    setEditing(false);
    if (value.trim() !== (label ?? '')) onRelabel(value);
  };

  return (
    <Stack gap={8}>
      <Group justify="space-between" align="center" wrap="nowrap">
        <Group gap={8} wrap="nowrap" miw={0}>
          {isCurrent ? (
            <Badge color={color} variant="filled" size="sm">
              {t('memory.current')}
            </Badge>
          ) : (
            <Tooltip label={t('memory.pastTooltip')} withArrow multiline w={240}>
              <Badge color="gray" variant="light" size="sm">
                {t('memory.past')}
              </Badge>
            </Tooltip>
          )}
          {editing ? (
            <TextInput
              value={value}
              onChange={(e) => setValue(e.currentTarget.value)}
              onBlur={commit}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commit();
                if (e.key === 'Escape') setEditing(false);
              }}
              placeholder={t('memory.labelPlaceholder')}
              size="xs"
              autoFocus
            />
          ) : label ? (
            <Text fw={600} size="sm" truncate>
              {label}
            </Text>
          ) : (
            <Code>{shortHash(hash)}</Code>
          )}
        </Group>
        {canWrite && !editing && (
          <Tooltip label={t('memory.nameVersion')} withArrow>
            <ActionIcon
              variant="subtle"
              color="gray"
              onClick={startEdit}
              aria-label={t('memory.nameVersion')}
            >
              <IconPencil size={14} />
            </ActionIcon>
          </Tooltip>
        )}
      </Group>

      <Stack gap={6}>
        {items.map((memory) => (
          <Card key={memory.id} withBorder padding="sm" radius="md">
            <Group justify="space-between" align="flex-start" wrap="nowrap" gap={8}>
              <Box miw={0}>
                <Text size="sm" style={{ whiteSpace: 'pre-wrap' }}>
                  {memory.content}
                </Text>
                <Text
                  size="xs"
                  c="dimmed"
                  mt={4}
                  title={new Date(memory.createdAt).toLocaleString(locale)}
                >
                  {formatWhen(memory.createdAt, locale)}
                </Text>
              </Box>
              {canWrite && (
                <ActionIcon
                  variant="subtle"
                  color="red"
                  onClick={() => onDelete(memory)}
                  aria-label={t('common.delete')}
                >
                  <IconTrash size={16} />
                </ActionIcon>
              )}
            </Group>
          </Card>
        ))}
      </Stack>
    </Stack>
  );
}
