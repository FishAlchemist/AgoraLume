import { Button, Group, Modal, Stack, Text, TextInput } from '@mantine/core';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { entriesToRecord, recordToEntries, type VarEntry } from '../lib/variables';
import { useWorkspace } from '../store/workspace';
import type { Department } from '../types';
import { ColorSelect } from './ColorSelect';
import { VariablesEditor } from './VariablesEditor';

interface Props {
  opened: boolean;
  onClose: () => void;
  /** Parent organization for a new department. */
  organizationId: string;
  department?: Department;
}

interface FormState {
  name: string;
  color: string;
  blurb: string;
  variables: VarEntry[];
}

function initialState(department?: Department): FormState {
  return {
    name: department?.name ?? '',
    color: department?.color ?? 'indigo',
    blurb: department?.blurb ?? '',
    variables: recordToEntries(department?.variables),
  };
}

export function DepartmentFormModal({ opened, onClose, organizationId, department }: Props) {
  const { t } = useTranslation();
  const addDepartment = useWorkspace((s) => s.addDepartment);
  const updateDepartment = useWorkspace((s) => s.updateDepartment);

  const [form, setForm] = useState<FormState>(() => initialState(department));
  const [wasOpen, setWasOpen] = useState(false);
  if (opened && !wasOpen) {
    setForm(initialState(department));
    setWasOpen(true);
  } else if (!opened && wasOpen) {
    setWasOpen(false);
  }

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  const canSave = form.name.trim().length > 0;

  const handleSave = () => {
    if (!canSave) return;
    const payload = {
      name: form.name.trim(),
      color: form.color,
      blurb: form.blurb.trim() || undefined,
      variables: entriesToRecord(form.variables),
    };
    if (department) updateDepartment(department.id, payload);
    else addDepartment({ organizationId, ...payload });
    onClose();
  };

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      size="md"
      title={department ? t('departments.edit') : t('departments.create')}
    >
      <Stack gap="md">
        <TextInput
          label={t('common.name')}
          value={form.name}
          onChange={(e) => set('name', e.currentTarget.value)}
          placeholder={t('departments.namePlaceholder')}
          data-autofocus
        />
        <ColorSelect
          label={t('common.color')}
          value={form.color}
          onChange={(v) => set('color', v)}
        />
        <TextInput
          label={t('departments.blurb')}
          value={form.blurb}
          onChange={(e) => set('blurb', e.currentTarget.value)}
        />
        <Stack gap={4}>
          <Text size="sm" fw={500}>
            {t('departments.variables')}
          </Text>
          <Text size="xs" c="dimmed">
            {t('departments.variablesHint')}
          </Text>
          <VariablesEditor
            entries={form.variables}
            onChange={(v) => set('variables', v)}
            addLabel={t('personas.addVariable')}
          />
        </Stack>
        <Group justify="flex-end" mt="xs">
          <Button variant="default" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button onClick={handleSave} disabled={!canSave}>
            {t('common.save')}
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
