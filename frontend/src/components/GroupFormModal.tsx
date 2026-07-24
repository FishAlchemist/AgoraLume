import { Button, Group, Modal, MultiSelect, Select, Stack, Text, TextInput } from '@mantine/core';
import { IconPlus } from '@tabler/icons-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useWorkspace } from '../store/workspace';
import type { Group as ChatGroup, Organization, Persona } from '../types';

interface Props {
  opened: boolean;
  onClose: () => void;
  group?: ChatGroup;
}

interface FormState {
  name: string;
  memberIds: string[];
  selfPersonaId: string;
}

const ALL = '__all__';
const UNASSIGNED = '__none__';
const ALL_DEPT = '__all_dept__';
const NO_DEPT = '__no_dept__';

function initialState(group: ChatGroup | undefined, defaultSelfId: string): FormState {
  return {
    name: group?.name ?? '',
    memberIds: group?.personaIds ?? [],
    selfPersonaId: group?.selfPersonaId ?? defaultSelfId,
  };
}

/** Groups persona options by organization so the member picker stays scannable. */
function groupByOrg(list: Persona[], organizations: Organization[], unassignedLabel: string) {
  const byOrg = new Map<string, { value: string; label: string }[]>();
  const unassigned: { value: string; label: string }[] = [];
  for (const p of list) {
    const item = { value: p.id, label: p.name };
    if (p.organizationId) {
      const arr = byOrg.get(p.organizationId) ?? [];
      arr.push(item);
      byOrg.set(p.organizationId, arr);
    } else {
      unassigned.push(item);
    }
  }
  const groups = organizations
    .filter((o) => byOrg.has(o.id))
    .map((o) => ({ group: o.name, items: byOrg.get(o.id) ?? [] }));
  if (unassigned.length > 0) groups.push({ group: unassignedLabel, items: unassigned });
  return groups;
}

export function GroupFormModal({ opened, onClose, group }: Props) {
  const { t } = useTranslation();
  const personas = useWorkspace((s) => s.personas);
  const organizations = useWorkspace((s) => s.organizations);
  const departments = useWorkspace((s) => s.departments);
  const addGroup = useWorkspace((s) => s.addGroup);
  const updateGroup = useWorkspace((s) => s.updateGroup);

  const aiPersonas = personas.filter((p) => p.kind === 'ai');
  const userPersonas = personas.filter((p) => p.kind === 'user');
  const defaultSelfId = userPersonas[0]?.id ?? '';

  const [form, setForm] = useState<FormState>(() => initialState(group, defaultSelfId));
  const [orgFilter, setOrgFilter] = useState<string>(ALL);
  const [deptFilter, setDeptFilter] = useState<string>(ALL_DEPT);
  const [wasOpen, setWasOpen] = useState(false);
  if (opened && !wasOpen) {
    setForm(initialState(group, defaultSelfId));
    setOrgFilter(ALL);
    setDeptFilter(ALL_DEPT);
    setWasOpen(true);
  } else if (!opened && wasOpen) {
    setWasOpen(false);
  }

  const orgFilterActive = orgFilter !== ALL && orgFilter !== UNASSIGNED;
  const selectedOrg = organizations.find((o) => o.id === orgFilter);
  const selectedDept = departments.find((d) => d.id === deptFilter);
  const orgDepartments = orgFilterActive
    ? departments.filter((d) => d.organizationId === orgFilter)
    : [];

  const changeOrgFilter = (value: string) => {
    setOrgFilter(value);
    setDeptFilter(ALL_DEPT);
  };

  const matchesFilter = (p: Persona) => {
    const orgOk =
      orgFilter === ALL
        ? true
        : orgFilter === UNASSIGNED
          ? !p.organizationId
          : p.organizationId === orgFilter;
    const deptOk =
      !orgFilterActive || deptFilter === ALL_DEPT
        ? true
        : deptFilter === NO_DEPT
          ? !p.departmentId
          : p.departmentId === deptFilter;
    return orgOk && deptOk;
  };

  const candidates = aiPersonas.filter(matchesFilter);
  // Keep already-selected members visible/labelled even when filtered out.
  const selected = new Set(form.memberIds);
  const dataPersonas = aiPersonas.filter((p) => matchesFilter(p) || selected.has(p.id));
  const memberData = groupByOrg(dataPersonas, organizations, t('personas.noOrg'));

  // Bulk-add label reflects the tightest active scope (department → organization → filter).
  const scopeName = selectedDept?.name ?? selectedOrg?.name;
  const addable = candidates.filter((p) => !selected.has(p.id)).length;

  const addFiltered = () => {
    setForm((f) => ({
      ...f,
      memberIds: [...new Set([...f.memberIds, ...candidates.map((p) => p.id)])],
    }));
  };

  const canSave = form.name.trim().length > 0;

  const handleSave = () => {
    if (!canSave) return;
    const payload = {
      name: form.name.trim(),
      personaIds: form.memberIds,
      selfPersonaId: form.selfPersonaId || defaultSelfId,
    };
    if (group) updateGroup(group.id, payload);
    else addGroup(payload);
    onClose();
  };

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      size="md"
      title={group ? t('groups.edit') : t('groups.create')}
    >
      <Stack gap="md">
        <TextInput
          label={t('groups.name')}
          value={form.name}
          onChange={(e) => setForm((f) => ({ ...f, name: e.currentTarget.value }))}
          data-autofocus
        />

        <Stack gap="xs">
          <Text size="sm" fw={500}>
            {t('groups.members')}
          </Text>
          <Text size="xs" c="dimmed">
            {t('groups.membersHint')}
          </Text>

          <Group gap="xs" align="flex-end" wrap="wrap">
            <Select
              w={170}
              size="xs"
              label={t('personas.filterByOrg')}
              value={orgFilter}
              onChange={(v) => v && changeOrgFilter(v)}
              allowDeselect={false}
              data={[
                { value: ALL, label: t('personas.filterAll') },
                { value: UNASSIGNED, label: t('personas.noOrg') },
                ...organizations.map((o) => ({ value: o.id, label: o.name })),
              ]}
            />
            {orgFilterActive && orgDepartments.length > 0 && (
              <Select
                w={170}
                size="xs"
                label={t('personas.filterByDept')}
                value={deptFilter}
                onChange={(v) => v && setDeptFilter(v)}
                allowDeselect={false}
                data={[
                  { value: ALL_DEPT, label: t('personas.filterAllDept') },
                  { value: NO_DEPT, label: t('personas.noDepartment') },
                  ...orgDepartments.map((d) => ({ value: d.id, label: d.name })),
                ]}
              />
            )}
            <Button
              size="xs"
              variant="light"
              leftSection={<IconPlus size={14} />}
              disabled={addable === 0}
              onClick={addFiltered}
            >
              {scopeName
                ? t('groups.addNamed', { name: scopeName, count: addable })
                : t('groups.addFiltered', { count: addable })}
            </Button>
          </Group>

          <MultiSelect
            data={memberData}
            value={form.memberIds}
            onChange={(v) => setForm((f) => ({ ...f, memberIds: v }))}
            placeholder={form.memberIds.length === 0 ? t('groups.membersPlaceholder') : undefined}
            searchable
            clearable
            hidePickedOptions
          />
        </Stack>

        <Select
          label={t('groups.identity')}
          description={t('groups.identityHint')}
          data={userPersonas.map((p) => ({ value: p.id, label: p.name }))}
          value={form.selfPersonaId}
          onChange={(v) => v && setForm((f) => ({ ...f, selfPersonaId: v }))}
          allowDeselect={false}
        />

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
