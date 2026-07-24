import { Button, Group, Modal, Stack, Text, TextInput } from '@mantine/core';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { entriesToRecord, recordToEntries, type VarEntry } from '../lib/variables';
import { useWorkspace } from '../store/workspace';
import type { Organization } from '../types';
import { ColorSelect } from './ColorSelect';
import { VariablesEditor } from './VariablesEditor';

interface Props {
  opened: boolean;
  onClose: () => void;
  organization?: Organization;
}

interface FormState {
  name: string;
  color: string;
  blurb: string;
  variables: VarEntry[];
}

function initialState(org?: Organization): FormState {
  return {
    name: org?.name ?? '',
    color: org?.color ?? 'indigo',
    blurb: org?.blurb ?? '',
    variables: recordToEntries(org?.variables),
  };
}

export function OrganizationFormModal({ opened, onClose, organization }: Props) {
  const { t } = useTranslation();
  const addOrganization = useWorkspace((s) => s.addOrganization);
  const updateOrganization = useWorkspace((s) => s.updateOrganization);

  const [form, setForm] = useState<FormState>(() => initialState(organization));
  const [wasOpen, setWasOpen] = useState(false);
  if (opened && !wasOpen) {
    setForm(initialState(organization));
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
    if (organization) updateOrganization(organization.id, payload);
    else addOrganization(payload);
    onClose();
  };

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      size="md"
      title={organization ? t('organizations.edit') : t('organizations.create')}
    >
      <Stack gap="md">
        <TextInput
          label={t('common.name')}
          value={form.name}
          onChange={(e) => set('name', e.currentTarget.value)}
          data-autofocus
        />
        <ColorSelect
          label={t('common.color')}
          value={form.color}
          onChange={(v) => set('color', v)}
        />
        <TextInput
          label={t('organizations.blurb')}
          value={form.blurb}
          onChange={(e) => set('blurb', e.currentTarget.value)}
        />
        <Stack gap={4}>
          <Text size="sm" fw={500}>
            {t('organizations.variables')}
          </Text>
          <Text size="xs" c="dimmed">
            {t('organizations.variablesHint')}
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
