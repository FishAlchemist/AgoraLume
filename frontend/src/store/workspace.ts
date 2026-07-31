import { create } from 'zustand';
import { type WorkspaceSnapshot, workspaceClient } from '../lib/api/workspace';
import type { GroupBundle, PersonaBundle } from '../lib/transfer';
import type { Department, Group, Organization, Persona, Settings } from '../types';
import { DEFAULT_USER_PERSONA_ID } from '../types';
import { useAuth } from './auth';
import { isGuestFallback, useBackendStatusStore } from './backendStatus';
import { useConnection } from './connection';

const uid = () =>
  crypto.randomUUID?.() ?? `id-${Date.now()}-${Math.random().toString(36).slice(2)}`;

/** The workspace data — the shape mirrored by the backend SSOT. */
interface WorkspaceData {
  organizations: Organization[];
  departments: Department[];
  personas: Persona[];
  groups: Group[];
  settings: Settings;
}

interface WorkspaceState extends WorkspaceData {
  addOrganization: (input: Omit<Organization, 'id'>) => void;
  updateOrganization: (id: string, patch: Partial<Omit<Organization, 'id'>>) => void;
  deleteOrganization: (id: string) => void;

  addDepartment: (input: Omit<Department, 'id'>) => void;
  updateDepartment: (id: string, patch: Partial<Omit<Department, 'id'>>) => void;
  deleteDepartment: (id: string) => void;

  addPersona: (input: Omit<Persona, 'id'>) => void;
  updatePersona: (id: string, patch: Partial<Omit<Persona, 'id'>>) => void;
  deletePersona: (id: string) => void;

  addGroup: (input: Omit<Group, 'id'>) => void;
  updateGroup: (id: string, patch: Partial<Omit<Group, 'id'>>) => void;
  deleteGroup: (id: string) => void;

  updateSettings: (patch: Partial<Settings>) => void;

  /** Merges a persona backup bundle in with fresh ids; returns personas imported. */
  importBundle: (bundle: PersonaBundle) => number;

  /** Merges a group backup bundle in with fresh ids; returns groups imported. */
  importGroupBundle: (bundle: GroupBundle) => number;

  /** Re-reads the whole workspace from the connected backend (SSOT). */
  hydrate: () => Promise<void>;
}

// --- Seed (mirrors backend/src/workspace.rs) --------------------------------

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
      name: 'Alex',
      kind: 'user',
      color: 'gray',
      emoji: '🧑',
      gradient: 'linear-gradient(135deg, #4dabf7, #4263eb)',
      blurb: 'Your own voice.',
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

function seedData(): WorkspaceData {
  return {
    organizations: seedOrganizations(),
    departments: seedDepartments(),
    personas: seedPersonas(),
    groups: seedGroups(),
    settings: defaultSettings,
  };
}

// --- Mock persistence -------------------------------------------------------
//
// Only the offline (mock) workspace is persisted to localStorage — it's the
// editable copy the app owns when no backend is present. When connected to a
// backend the workspace is a cache of the SSOT and is *not* written here, so
// switching back to mock restores the user's own offline data untouched.

const MOCK_KEY = 'agoralume-workspace';
// v3: collapsed to a single user identity (dropped multi-persona "identities").
const MOCK_VERSION = 3;

function loadMock(): WorkspaceData {
  try {
    const raw = localStorage.getItem(MOCK_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as { version?: number } & Partial<WorkspaceData>;
      // Alpha: the schema still moves freely, so incompatible data is discarded.
      if (parsed.version === MOCK_VERSION) {
        return {
          organizations: parsed.organizations ?? [],
          departments: parsed.departments ?? [],
          personas: parsed.personas ?? [],
          groups: parsed.groups ?? [],
          settings: parsed.settings ?? defaultSettings,
        };
      }
    }
  } catch {
    // Corrupt or unavailable storage — fall through to a fresh seed.
  }
  return seedData();
}

function saveMock(data: WorkspaceData): void {
  try {
    localStorage.setItem(MOCK_KEY, JSON.stringify({ version: MOCK_VERSION, ...data }));
  } catch {
    // Storage full or unavailable — nothing we can do; keep running in memory.
  }
}

// --- Backend routing --------------------------------------------------------

/**
 * The workspace client for the active backend, or `null` when the app should
 * show placeholder data instead — no backend configured, or one is but
 * there's no session for it yet (see `isGuestFallback`).
 */
function backend() {
  const url = useConnection.getState().backendUrl;
  return url && !isGuestFallback() ? workspaceClient(url) : null;
}

/**
 * What to show instead of the real SSOT when `backend()` is null. A
 * configured-but-unauthenticated backend gets the shared seed — the same
 * content a fresh install of *that* backend actually has (see `seedData`),
 * not some unrelated sandbox — since a guest can't see the real account's
 * data anyway and showing them a stand-in for their own past local edits
 * would be a non sequitur. Only the true no-backend case (nothing to ever log
 * into) gets the persisted offline sandbox.
 */
function offlineData(): WorkspaceData {
  return useConnection.getState().backendUrl ? seedData() : loadMock();
}

export const useWorkspace = create<WorkspaceState>()((set, get) => {
  // If a mutation's optimistic guess ever disagrees with the backend, snap the
  // whole store back to the SSOT so the two can't quietly drift apart.
  const resync = () => {
    void get().hydrate();
  };

  return {
    // A configured backend (whether about to hydrate, or a guest still
    // waiting on a session) starts from the shared seed — identical to the
    // backend's own, so there's no visible flash; no backend at all loads the
    // persisted offline sandbox. See `offlineData`.
    ...(backend() ? seedData() : offlineData()),

    addOrganization: (input) => {
      const org: Organization = { ...input, id: uid() };
      set((s) => ({ organizations: [...s.organizations, org] }));
      backend()?.createOrganization(org).catch(resync);
    },
    updateOrganization: (id, patch) => {
      set((s) => ({
        organizations: s.organizations.map((o) => (o.id === id ? { ...o, ...patch } : o)),
      }));
      backend()?.updateOrganization(id, patch).catch(resync);
    },
    deleteOrganization: (id) => {
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
      });
      backend()?.deleteOrganization(id).catch(resync);
    },

    addDepartment: (input) => {
      const department: Department = { ...input, id: uid() };
      set((s) => ({ departments: [...s.departments, department] }));
      backend()?.createDepartment(department).catch(resync);
    },
    updateDepartment: (id, patch) => {
      set((s) => ({
        departments: s.departments.map((d) => (d.id === id ? { ...d, ...patch } : d)),
      }));
      backend()?.updateDepartment(id, patch).catch(resync);
    },
    deleteDepartment: (id) => {
      set((s) => ({
        departments: s.departments.filter((d) => d.id !== id),
        personas: s.personas.map((p) =>
          p.departmentId === id ? { ...p, departmentId: undefined } : p,
        ),
      }));
      backend()?.deleteDepartment(id).catch(resync);
    },

    addPersona: (input) => {
      const persona: Persona = { ...input, id: uid() };
      set((s) => ({ personas: [...s.personas, persona] }));
      backend()?.createPersona(persona).catch(resync);
    },
    updatePersona: (id, patch) => {
      set((s) => ({
        personas: s.personas.map((p) => (p.id === id ? { ...p, ...patch } : p)),
      }));
      backend()?.updatePersona(id, patch).catch(resync);
    },
    deletePersona: (id) => {
      const state = get();
      const target = state.personas.find((p) => p.id === id);
      // Keep at least one user identity around — groups always need a "you".
      // The backend enforces the same guard (409), so we don't call it here.
      if (target?.kind === 'user' && state.personas.filter((p) => p.kind === 'user').length <= 1) {
        return;
      }
      const fallbackSelf = state.personas.find((p) => p.kind === 'user' && p.id !== id)?.id;
      set((s) => ({
        personas: s.personas.filter((p) => p.id !== id),
        groups: s.groups.map((g) => ({
          ...g,
          personaIds: g.personaIds.filter((pid) => pid !== id),
          selfPersonaId: g.selfPersonaId === id && fallbackSelf ? fallbackSelf : g.selfPersonaId,
        })),
      }));
      backend()?.deletePersona(id).catch(resync);
    },

    addGroup: (input) => {
      const group: Group = { ...input, id: uid() };
      set((s) => ({ groups: [...s.groups, group] }));
      backend()?.createGroup(group).catch(resync);
    },
    updateGroup: (id, patch) => {
      set((s) => ({
        groups: s.groups.map((g) => (g.id === id ? { ...g, ...patch } : g)),
      }));
      backend()?.updateGroup(id, patch).catch(resync);
    },
    deleteGroup: (id) => {
      set((s) => ({ groups: s.groups.filter((g) => g.id !== id) }));
      backend()?.deleteGroup(id).catch(resync);
    },

    updateSettings: (patch) => {
      set((s) => ({ settings: { ...s.settings, ...patch } }));
      backend()?.updateSettings(patch).catch(resync);
    },

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

      const client = backend();
      if (client) {
        // Ids are client-generated and honoured by the server, so cross-links
        // stay consistent regardless of insert order.
        const posts = [
          ...newOrgs.map((o) => client.createOrganization(o)),
          ...newDepartments.map((d) => client.createDepartment(d)),
          ...newPersonas.map((p) => client.createPersona(p)),
        ];
        Promise.all(posts).catch(resync);
      }
      return newPersonas.length;
    },

    importGroupBundle: (bundle) => {
      const state = get();
      const orgIdMap = new Map<string, string>();
      const deptIdMap = new Map<string, string>();
      const personaIdMap = new Map<string, string>();

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
        .map((p) => {
          const id = uid();
          personaIdMap.set(p.id, id);
          return {
            ...p,
            id,
            organizationId: p.organizationId ? orgIdMap.get(p.organizationId) : undefined,
            departmentId: p.departmentId ? deptIdMap.get(p.departmentId) : undefined,
          };
        });
      // The imported group's "you" is remapped to an existing local identity —
      // user identities are never carried in a bundle.
      const fallbackSelf =
        state.personas.find((p) => p.kind === 'user')?.id ?? DEFAULT_USER_PERSONA_ID;
      const newGroups = bundle.groups.map((g) => ({
        ...g,
        id: uid(),
        personaIds: g.personaIds
          .map((pid) => personaIdMap.get(pid))
          .filter((id): id is string => Boolean(id)),
        selfPersonaId: fallbackSelf,
      }));

      set((s) => ({
        organizations: [...s.organizations, ...newOrgs],
        departments: [...s.departments, ...newDepartments],
        personas: [...s.personas, ...newPersonas],
        groups: [...s.groups, ...newGroups],
      }));

      const client = backend();
      if (client) {
        // Client-generated ids are honoured by the server, so cross-links stay
        // consistent regardless of insert order.
        const posts = [
          ...newOrgs.map((o) => client.createOrganization(o)),
          ...newDepartments.map((d) => client.createDepartment(d)),
          ...newPersonas.map((p) => client.createPersona(p)),
          ...newGroups.map((g) => client.createGroup(g)),
        ];
        Promise.all(posts).catch(resync);
      }
      return newGroups.length;
    },

    hydrate: async () => {
      const url = useConnection.getState().backendUrl;
      if (!url || isGuestFallback()) return;
      try {
        const snap: WorkspaceSnapshot = await workspaceClient(url).fetchAll();
        // Ignore a stale response if the source changed while we were fetching.
        if (useConnection.getState().backendUrl !== url) return;
        set(snap);
      } catch {
        // Backend not up yet (or unreachable). useBackendStatus polls liveness
        // and re-hydrates when it comes online, so we don't loop here.
      }
    },
  };
});

// Persist the offline sandbox, and only that sandbox: a guest's session on
// a real backend shows the shared seed (see `offlineData`) but never saves
// it here — that key is the no-backend-at-all dev sandbox specifically, not
// a place for ephemeral guest edits to end up.
useWorkspace.subscribe((state) => {
  if (!useConnection.getState().backendUrl) {
    saveMock({
      organizations: state.organizations,
      departments: state.departments,
      personas: state.personas,
      groups: state.groups,
      settings: state.settings,
    });
  }
});

// React to anything that changes which data source is live — connecting to a
// different backend, logging in or out, or learning whether login is even
// required — by pulling the SSOT once it becomes reachable and restoring the
// offline copy once it stops being. `backend()` already makes exactly this
// call; only act when it actually flips, so an unrelated store update (e.g.
// editing a setting) doesn't re-trigger a hydrate.
let wasBackendRoute = !!backend();

function syncWorkspaceToDataSource() {
  const isBackendRoute = !!backend();
  if (isBackendRoute === wasBackendRoute) return;
  wasBackendRoute = isBackendRoute;
  if (isBackendRoute) {
    void useWorkspace.getState().hydrate();
  } else {
    useWorkspace.setState(offlineData());
  }
}

useConnection.subscribe(syncWorkspaceToDataSource);
useAuth.subscribe(syncWorkspaceToDataSource);
useBackendStatusStore.subscribe(syncWorkspaceToDataSource);

// On startup already routed to a backend (e.g. a returning session with a
// still-valid token), pull the SSOT once.
if (wasBackendRoute) {
  void useWorkspace.getState().hydrate();
}
