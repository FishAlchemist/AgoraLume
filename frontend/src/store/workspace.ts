import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { PersonaBundle } from '../lib/transfer';
import type { Department, Group, Organization, Persona, Settings } from '../types';
import { DEFAULT_USER_PERSONA_ID } from '../types';

const uid = () =>
  crypto.randomUUID?.() ?? `id-${Date.now()}-${Math.random().toString(36).slice(2)}`;

interface WorkspaceState {
  organizations: Organization[];
  departments: Department[];
  personas: Persona[];
  groups: Group[];
  settings: Settings;

  addOrganization: (input: Omit<Organization, 'id'>) => Organization;
  updateOrganization: (id: string, patch: Partial<Omit<Organization, 'id'>>) => void;
  deleteOrganization: (id: string) => void;

  addDepartment: (input: Omit<Department, 'id'>) => Department;
  updateDepartment: (id: string, patch: Partial<Omit<Department, 'id'>>) => void;
  deleteDepartment: (id: string) => void;

  addPersona: (input: Omit<Persona, 'id'>) => Persona;
  updatePersona: (id: string, patch: Partial<Omit<Persona, 'id'>>) => void;
  deletePersona: (id: string) => void;

  addGroup: (input: Omit<Group, 'id'>) => Group;
  updateGroup: (id: string, patch: Partial<Omit<Group, 'id'>>) => void;
  deleteGroup: (id: string) => void;

  updateSettings: (patch: Partial<Settings>) => void;

  /** Merges a backup bundle in with fresh ids; returns personas imported. */
  importBundle: (bundle: PersonaBundle) => number;
}

function seedOrganizations(): Organization[] {
  return [
    {
      id: 'aurora',
      name: 'Aurora Academy',
      color: 'indigo',
      blurb: 'A school whose classes and clubs share one bright near-future setting.',
      variables: { world: 'a bright near-future Tokyo' },
    },
  ];
}

function seedDepartments(): Department[] {
  return [
    {
      id: 'class-2a',
      organizationId: 'aurora',
      name: 'Class 2-A',
      color: 'violet',
      variables: { setting: 'a lively second-year classroom' },
    },
    {
      id: 'broadcast',
      organizationId: 'aurora',
      name: 'Broadcast Club',
      color: 'cyan',
      variables: { setting: 'the after-school broadcast room' },
    },
  ];
}

function seedPersonas(): Persona[] {
  return [
    {
      id: DEFAULT_USER_PERSONA_ID,
      name: 'You',
      kind: 'user',
      color: 'gray',
      emoji: '🧑',
      gradient: 'linear-gradient(135deg, #4dabf7, #4263eb)',
      blurb: 'Your own voice.',
    },
    {
      id: 'alter-ego',
      name: 'Masked',
      kind: 'user',
      color: 'dark',
      emoji: '🎭',
      gradient: 'linear-gradient(135deg, #495057, #212529)',
      blurb: 'An anonymous alter-ego.',
    },
    {
      id: 'aria',
      name: 'Aria',
      kind: 'ai',
      color: 'violet',
      emoji: '🌟',
      gradient: 'linear-gradient(135deg, #b197fc, #4dabf7)',
      blurb: 'Warm, curious host persona.',
      organizationId: 'aurora',
      departmentId: 'class-2a',
      systemPrompt:
        'You are {{persona_name}} in {{department_name}}, a warm and curious host in {{setting}}. Always reply in {{user_language}}.',
    },
    {
      id: 'nox',
      name: 'Nox',
      kind: 'ai',
      color: 'cyan',
      emoji: '🌙',
      gradient: 'linear-gradient(135deg, #3bc9db, #4263eb)',
      blurb: 'Dry, analytical strategist.',
      organizationId: 'aurora',
      departmentId: 'broadcast',
      systemPrompt:
        'You are {{persona_name}} of {{department_name}}, a dry, analytical strategist in {{setting}}. Always reply in {{user_language}}.',
    },
    {
      id: 'sol',
      name: 'Sol',
      kind: 'ai',
      color: 'orange',
      emoji: '☀️',
      gradient: 'linear-gradient(135deg, #ffd43b, #ff922b)',
      blurb: 'Upbeat, energetic cheerleader.',
      organizationId: 'aurora',
      departmentId: 'class-2a',
      systemPrompt:
        'You are {{persona_name}} in {{department_name}}, an upbeat, energetic cheerleader in {{setting}}. Always reply in {{user_language}}.',
    },
  ];
}

function seedGroups(): Group[] {
  return [
    {
      id: 'lounge',
      name: 'The Lounge',
      personaIds: ['aria', 'nox', 'sol'],
      selfPersonaId: DEFAULT_USER_PERSONA_ID,
    },
    {
      id: 'lab',
      name: 'Persona Lab',
      personaIds: ['aria', 'nox'],
      selfPersonaId: DEFAULT_USER_PERSONA_ID,
    },
  ];
}

const defaultSettings: Settings = {
  uiLanguage: 'zh-Hant',
  nativeLanguage: '繁體中文',
  chatFontSize: 15,
};

export const useWorkspace = create<WorkspaceState>()(
  persist(
    (set) => ({
      organizations: seedOrganizations(),
      departments: seedDepartments(),
      personas: seedPersonas(),
      groups: seedGroups(),
      settings: defaultSettings,

      addOrganization: (input) => {
        const org: Organization = { ...input, id: uid() };
        set((s) => ({ organizations: [...s.organizations, org] }));
        return org;
      },
      updateOrganization: (id, patch) =>
        set((s) => ({
          organizations: s.organizations.map((o) => (o.id === id ? { ...o, ...patch } : o)),
        })),
      deleteOrganization: (id) =>
        set((s) => {
          const removedDeptIds = new Set(
            s.departments.filter((d) => d.organizationId === id).map((d) => d.id),
          );
          return {
            organizations: s.organizations.filter((o) => o.id !== id),
            departments: s.departments.filter((d) => d.organizationId !== id),
            // Members keep existing but lose the now-dangling org/department links.
            personas: s.personas.map((p) =>
              p.organizationId === id || (p.departmentId && removedDeptIds.has(p.departmentId))
                ? { ...p, organizationId: undefined, departmentId: undefined }
                : p,
            ),
          };
        }),

      addDepartment: (input) => {
        const department: Department = { ...input, id: uid() };
        set((s) => ({ departments: [...s.departments, department] }));
        return department;
      },
      updateDepartment: (id, patch) =>
        set((s) => ({
          departments: s.departments.map((d) => (d.id === id ? { ...d, ...patch } : d)),
        })),
      deleteDepartment: (id) =>
        set((s) => ({
          departments: s.departments.filter((d) => d.id !== id),
          personas: s.personas.map((p) =>
            p.departmentId === id ? { ...p, departmentId: undefined } : p,
          ),
        })),

      addPersona: (input) => {
        const persona: Persona = { ...input, id: uid() };
        set((s) => ({ personas: [...s.personas, persona] }));
        return persona;
      },
      updatePersona: (id, patch) =>
        set((s) => ({
          personas: s.personas.map((p) => (p.id === id ? { ...p, ...patch } : p)),
        })),
      deletePersona: (id) =>
        set((s) => {
          const target = s.personas.find((p) => p.id === id);
          // Keep at least one user identity around — groups always need a "you".
          if (target?.kind === 'user' && s.personas.filter((p) => p.kind === 'user').length <= 1) {
            return {};
          }
          const fallbackSelf = s.personas.find((p) => p.kind === 'user' && p.id !== id)?.id;
          return {
            personas: s.personas.filter((p) => p.id !== id),
            groups: s.groups.map((g) => ({
              ...g,
              personaIds: g.personaIds.filter((pid) => pid !== id),
              selfPersonaId:
                g.selfPersonaId === id && fallbackSelf ? fallbackSelf : g.selfPersonaId,
            })),
          };
        }),

      addGroup: (input) => {
        const group: Group = { ...input, id: uid() };
        set((s) => ({ groups: [...s.groups, group] }));
        return group;
      },
      updateGroup: (id, patch) =>
        set((s) => ({
          groups: s.groups.map((g) => (g.id === id ? { ...g, ...patch } : g)),
        })),
      deleteGroup: (id) => set((s) => ({ groups: s.groups.filter((g) => g.id !== id) })),

      updateSettings: (patch) => set((s) => ({ settings: { ...s.settings, ...patch } })),

      importBundle: (bundle) => {
        const orgIdMap = new Map<string, string>();
        const deptIdMap = new Map<string, string>();

        const newOrgs = bundle.organizations.map((o) => {
          const id = uid();
          orgIdMap.set(o.id, id);
          return { ...o, id };
        });
        const newDepartments = bundle.departments.map((d) => {
          const id = uid();
          deptIdMap.set(d.id, id);
          return { ...d, id, organizationId: orgIdMap.get(d.organizationId) ?? d.organizationId };
        });
        const newPersonas = bundle.personas
          .filter((p) => p.kind !== 'user')
          .map((p) => ({
            ...p,
            id: uid(),
            organizationId: p.organizationId ? orgIdMap.get(p.organizationId) : undefined,
            departmentId: p.departmentId ? deptIdMap.get(p.departmentId) : undefined,
          }));

        set((s) => ({
          organizations: [...s.organizations, ...newOrgs],
          departments: [...s.departments, ...newDepartments],
          personas: [...s.personas, ...newPersonas],
        }));
        return newPersonas.length;
      },
    }),
    {
      name: 'agoralume-workspace',
      // Alpha: the schema still moves freely, so we don't migrate old state —
      // incompatible persisted data is simply discarded on load.
      version: 1,
      partialize: (s) => ({
        organizations: s.organizations,
        departments: s.departments,
        personas: s.personas,
        groups: s.groups,
        settings: s.settings,
      }),
    },
  ),
);
