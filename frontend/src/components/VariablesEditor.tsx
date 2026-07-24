import { ActionIcon, Button, Group, Stack, TextInput } from '@mantine/core';
import { IconPlus, IconTrash } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';
import { blankEntry, type VarEntry } from '../lib/variables';

interface Props {
  entries: VarEntry[];
  onChange: (next: VarEntry[]) => void;
  addLabel: string;
}

/** Editable list of key/value template variables. Order is preserved. */
export function VariablesEditor({ entries, onChange, addLabel }: Props) {
  const { t } = useTranslation();

  const update = (id: string, patch: Partial<VarEntry>) =>
    onChange(entries.map((e) => (e.id === id ? { ...e, ...patch } : e)));

  return (
    <Stack gap="xs">
      {entries.map((entry) => (
        <Group key={entry.id} gap="xs" wrap="nowrap" align="flex-start">
          <TextInput
            flex={1}
            placeholder={t('common.key')}
            value={entry.key}
            onChange={(e) => update(entry.id, { key: e.currentTarget.value })}
          />
          <TextInput
            flex={1.6}
            placeholder={t('common.value')}
            value={entry.value}
            onChange={(e) => update(entry.id, { value: e.currentTarget.value })}
          />
          <ActionIcon
            variant="subtle"
            color="red"
            size="lg"
            aria-label={t('common.remove')}
            onClick={() => onChange(entries.filter((e) => e.id !== entry.id))}
          >
            <IconTrash size={16} />
          </ActionIcon>
        </Group>
      ))}
      <Group>
        <Button
          size="xs"
          variant="light"
          leftSection={<IconPlus size={14} />}
          onClick={() => onChange([...entries, blankEntry()])}
        >
          {addLabel}
        </Button>
      </Group>
    </Stack>
  );
}
