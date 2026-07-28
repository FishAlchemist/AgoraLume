import {
  Button,
  Code,
  Divider,
  Group,
  Modal,
  Paper,
  ScrollArea,
  Select,
  Stack,
  Text,
  Textarea,
  TextInput,
} from '@mantine/core';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isNameTaken, MAX_PERSONA_BLURB_LEN, MAX_PERSONA_NAME_LEN } from '../lib/persona';
import { BUILTIN_VARIABLE_NAMES, resolveSystemPrompt } from '../lib/prompt';
import { entriesToRecord, recordToEntries, type VarEntry } from '../lib/variables';
import { useWorkspace } from '../store/workspace';
import type { Department, Persona, PersonaKind } from '../types';
import { AvatarUpload } from './AvatarUpload';
import { ColorSelect } from './ColorSelect';
import { CopyIconButton } from './CopyIconButton';
import { VariablesEditor } from './VariablesEditor';

interface Props {
  opened: boolean;
  onClose: () => void;
  /** Persona being edited; omit to create a new one. */
  persona?: Persona;
  /** Kind to use when creating (ignored when editing an existing persona). */
  defaultKind?: PersonaKind;
}

interface FormState {
  name: string;
  kind: PersonaKind;
  organizationId: string;
  departmentId: string;
  color: string;
  emoji: string;
  avatarUrl: string;
  blurb: string;
  systemPrompt: string;
  variables: VarEntry[];
}

const NO_ORG = '__none__';
const NO_DEPT = '__none__';

/** Whether `deptId` still belongs to `orgId`, so a department selection survives
 * an organization change. */
function keepsDepartment(departments: Department[], deptId: string, orgId: string): boolean {
  return departments.some((d) => d.id === deptId && d.organizationId === orgId);
}

/** Runs `onOpen` once each time `opened` transitions false → true — the "re-seed
 * the form when the modal opens" idiom, as a state adjustment during render. */
function useOnOpen(opened: boolean, onOpen: () => void) {
  const [wasOpen, setWasOpen] = useState(false);
  if (opened && !wasOpen) {
    setWasOpen(true);
    onOpen();
  } else if (!opened && wasOpen) {
    setWasOpen(false);
  }
}

function initialState(persona: Persona | undefined, defaultKind: PersonaKind): FormState {
  return {
    name: persona?.name ?? '',
    kind: persona?.kind ?? defaultKind,
    organizationId: persona?.organizationId ?? NO_ORG,
    departmentId: persona?.departmentId ?? NO_DEPT,
    color: persona?.color ?? 'indigo',
    emoji: persona?.emoji ?? '',
    avatarUrl: persona?.avatarUrl ?? '',
    blurb: persona?.blurb ?? '',
    systemPrompt: persona?.systemPrompt ?? '',
    variables: recordToEntries(persona?.variables),
  };
}

export function PersonaFormModal({ opened, onClose, persona, defaultKind = 'ai' }: Props) {
  const { t } = useTranslation();
  const organizations = useWorkspace((s) => s.organizations);
  const departments = useWorkspace((s) => s.departments);
  const settings = useWorkspace((s) => s.settings);
  const personas = useWorkspace((s) => s.personas);
  const addPersona = useWorkspace((s) => s.addPersona);
  const updatePersona = useWorkspace((s) => s.updatePersona);

  // Re-seed the form each time the modal transitions into the open state.
  const [form, setForm] = useState<FormState>(() => initialState(persona, defaultKind));
  useOnOpen(opened, () => setForm(initialState(persona, defaultKind)));

  const isUser = form.kind === 'user';

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  const org = organizations.find((o) => o.id === form.organizationId);
  const department = departments.find((d) => d.id === form.departmentId);
  const orgDepartments = departments.filter((d) => d.organizationId === form.organizationId);

  const changeOrganization = (value: string) =>
    setForm((f) => ({
      ...f,
      organizationId: value,
      departmentId: keepsDepartment(departments, f.departmentId, value) ? f.departmentId : NO_DEPT,
    }));

  const preview = useMemo(() => {
    const draft: Persona = {
      id: persona?.id ?? 'draft',
      name: form.name || 'Persona',
      kind: form.kind,
      color: form.color,
      organizationId: org?.id,
      departmentId: department?.id,
      systemPrompt: form.systemPrompt,
      variables: entriesToRecord(form.variables),
    };
    return resolveSystemPrompt(draft, org, department, settings);
  }, [form, org, department, settings, persona?.id]);

  // Names are globally unique (the backend enforces it with a 409, and agents
  // address members by name), so flag a collision before saving.
  const nameErr = isNameTaken(personas, form.name, persona?.id)
    ? t('personas.nameTaken')
    : undefined;
  const canSave = form.name.trim().length > 0 && !nameErr;

  const handleSave = () => {
    if (!canSave) return;
    const payload = {
      name: form.name.trim(),
      kind: form.kind,
      color: form.color,
      organizationId: form.organizationId === NO_ORG ? undefined : form.organizationId,
      departmentId: form.departmentId === NO_DEPT ? undefined : form.departmentId,
      emoji: form.emoji.trim() || undefined,
      avatarUrl: form.avatarUrl || undefined,
      blurb: form.blurb.trim() || undefined,
      systemPrompt: form.systemPrompt.trim() || undefined,
      variables: entriesToRecord(form.variables),
    };
    if (persona) updatePersona(persona.id, payload);
    else addPersona(payload);
    onClose();
  };

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      size="lg"
      title={
        persona
          ? t(isUser ? 'personas.editIdentity' : 'personas.edit')
          : t(isUser ? 'personas.createIdentity' : 'personas.create')
      }
      scrollAreaComponent={ScrollArea.Autosize}
    >
      <Stack gap="md">
        <TextInput
          label={t('personas.displayName')}
          value={form.name}
          onChange={(e) => set('name', e.currentTarget.value)}
          error={nameErr}
          maxLength={MAX_PERSONA_NAME_LEN}
          data-autofocus
        />

        <AvatarUpload
          value={form.avatarUrl || undefined}
          onChange={(v) => set('avatarUrl', v ?? '')}
          preview={{
            name: form.name,
            color: form.color,
            emoji: form.emoji || undefined,
            gradient: persona?.gradient,
          }}
        />

        {!isUser && (
          <Group grow align="flex-start">
            <Select
              label={t('personas.organization')}
              value={form.organizationId}
              onChange={(v) => v && changeOrganization(v)}
              allowDeselect={false}
              data={[
                { value: NO_ORG, label: t('personas.noOrg') },
                ...organizations.map((o) => ({ value: o.id, label: o.name })),
              ]}
            />
            <Select
              label={t('personas.department')}
              value={form.departmentId}
              onChange={(v) => v && set('departmentId', v)}
              allowDeselect={false}
              disabled={form.organizationId === NO_ORG}
              data={[
                { value: NO_DEPT, label: t('personas.noDepartment') },
                ...orgDepartments.map((d) => ({ value: d.id, label: d.name })),
              ]}
            />
          </Group>
        )}

        <Group grow align="flex-start">
          <ColorSelect
            label={t('common.color')}
            value={form.color}
            onChange={(v) => set('color', v)}
          />
          <TextInput
            label={t('personas.emoji')}
            value={form.emoji}
            onChange={(e) => set('emoji', e.currentTarget.value)}
            maxLength={4}
          />
        </Group>

        <TextInput
          label={t('personas.blurb')}
          value={form.blurb}
          onChange={(e) => set('blurb', e.currentTarget.value)}
          maxLength={MAX_PERSONA_BLURB_LEN}
        />

        {!isUser && (
          <>
            <Divider />

            <Textarea
              label={t('personas.systemPrompt')}
              description={t('personas.systemPromptHint', { token: '{{variable}}' })}
              value={form.systemPrompt}
              onChange={(e) => set('systemPrompt', e.currentTarget.value)}
              autosize
              minRows={3}
              maxRows={10}
            />

            <Stack gap={4}>
              <Text size="sm" fw={500}>
                {t('personas.variables')}
              </Text>
              <Text size="xs" c="dimmed">
                {t('personas.variablesHint')}
              </Text>
              <VariablesEditor
                entries={form.variables}
                onChange={(v) => set('variables', v)}
                addLabel={t('personas.addVariable')}
              />
            </Stack>

            <Stack gap={4}>
              <Text size="xs" c="dimmed">
                {t('personas.builtins')}:{' '}
                {BUILTIN_VARIABLE_NAMES.map((n, i) => (
                  <span key={n}>
                    {i > 0 ? ' ' : ''}
                    <Code>{`{{${n}}}`}</Code>
                  </span>
                ))}
              </Text>
              <Group justify="space-between" align="center" wrap="nowrap">
                <Text size="sm" fw={500}>
                  {t('personas.preview')}
                </Text>
                {preview && <CopyIconButton value={preview} />}
              </Group>
              <Paper withBorder p="sm" radius="md" bg="var(--mantine-color-body)">
                <Text
                  size="sm"
                  style={{ whiteSpace: 'pre-wrap' }}
                  c={preview ? undefined : 'dimmed'}
                >
                  {preview || '—'}
                </Text>
              </Paper>
            </Stack>
          </>
        )}

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
