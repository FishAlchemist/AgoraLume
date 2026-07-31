import type {
  Department,
  Group,
  Memory,
  Organization,
  Persona,
  PromptLabel,
  Settings,
} from '../../types';
import { versionedBase } from './version';

/** A full read of the backend-owned workspace (the SSOT). */
export interface WorkspaceSnapshot {
  organizations: Organization[];
  departments: Department[];
  personas: Persona[];
  groups: Group[];
  settings: Settings;
}

/**
 * REST client for the backend-owned workspace: organizations, departments,
 * personas, groups, and settings. The workspace store uses this whenever it's
 * pointed at a backend; offline (mock) mode edits the store locally instead.
 *
 * Follows the backend OpenAPI contract (`openapi.yml` at the repo root): POST
 * creates (the body carries a client-generated id the server honours, so the
 * client can insert optimistically without a round-trip), PATCH merges a
 * partial body, DELETE removes — cascades (e.g. deleting an organization drops
 * its departments) happen server-side, so callers re-read after a delete.
 */
export class HttpWorkspaceApi {
  private readonly baseUrl: string;

  constructor(rawBaseUrl: string) {
    this.baseUrl = versionedBase(rawBaseUrl);
  }

  private async get<T>(path: string): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`, { headers: { Accept: 'application/json' } });
    if (!res.ok) throw new Error(`GET ${path} failed: ${res.status}`);
    return (await res.json()) as T;
  }

  private async send(method: string, path: string, body?: unknown): Promise<void> {
    const res = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`${method} ${path} failed: ${res.status}`);
  }

  /** Like `send`, but for endpoints that return the created/updated record. */
  private async sendJson<T>(method: string, path: string, body: unknown): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`${method} ${path} failed: ${res.status}`);
    return (await res.json()) as T;
  }

  /** Reads the whole workspace in one shot (used to hydrate the store). */
  async fetchAll(): Promise<WorkspaceSnapshot> {
    const [organizations, departments, personas, groups, settings] = await Promise.all([
      this.get<Organization[]>('/organizations'),
      this.get<Department[]>('/departments'),
      this.get<Persona[]>('/personas'),
      this.get<Group[]>('/groups'),
      this.get<Settings>('/settings'),
    ]);
    return { organizations, departments, personas, groups, settings };
  }

  createOrganization(org: Organization): Promise<void> {
    return this.send('POST', '/organizations', org);
  }
  updateOrganization(id: string, patch: Partial<Organization>): Promise<void> {
    return this.send('PATCH', `/organizations/${id}`, patch);
  }
  deleteOrganization(id: string): Promise<void> {
    return this.send('DELETE', `/organizations/${id}`);
  }

  createDepartment(dept: Department): Promise<void> {
    return this.send('POST', '/departments', dept);
  }
  updateDepartment(id: string, patch: Partial<Department>): Promise<void> {
    return this.send('PATCH', `/departments/${id}`, patch);
  }
  deleteDepartment(id: string): Promise<void> {
    return this.send('DELETE', `/departments/${id}`);
  }

  createPersona(persona: Persona): Promise<void> {
    return this.send('POST', '/personas', persona);
  }
  updatePersona(id: string, patch: Partial<Persona>): Promise<void> {
    return this.send('PATCH', `/personas/${id}`, patch);
  }
  deletePersona(id: string): Promise<void> {
    return this.send('DELETE', `/personas/${id}`);
  }

  createGroup(group: Group): Promise<void> {
    return this.send('POST', '/groups', group);
  }
  updateGroup(id: string, patch: Partial<Group>): Promise<void> {
    return this.send('PATCH', `/groups/${id}`, patch);
  }
  deleteGroup(id: string): Promise<void> {
    return this.send('DELETE', `/groups/${id}`);
  }

  updateSettings(patch: Partial<Settings>): Promise<void> {
    return this.send('PATCH', '/settings', patch);
  }

  // Persona memory. Backend-only: the mock has no agent loop to record it, and
  // memories live in the SSOT rather than the client-cached workspace, so the
  // management UI reads them on demand instead of holding them in the store.
  listMemories(personaId: string): Promise<Memory[]> {
    return this.get<Memory[]>(`/personas/${personaId}/memories`);
  }
  createMemory(personaId: string, content: string): Promise<Memory> {
    return this.sendJson<Memory>('POST', `/personas/${personaId}/memories`, { content });
  }
  deleteMemory(personaId: string, memoryId: string): Promise<void> {
    return this.send('DELETE', `/personas/${personaId}/memories/${memoryId}`);
  }

  // Prompt-version labels: git-tag-style names for persona identity hashes,
  // shown by the memory UI to tell one character version from another.
  listPromptLabels(): Promise<PromptLabel[]> {
    return this.get<PromptLabel[]>('/prompt-labels');
  }
  setPromptLabel(hash: string, label: string): Promise<PromptLabel> {
    return this.sendJson<PromptLabel>('PUT', `/prompt-labels/${hash}`, { label });
  }
}

// One client per URL, reused across switches so we don't leak or re-create them.
const clients = new Map<string, HttpWorkspaceApi>();

/** The workspace client for a backend URL (cached), mirroring the chat facade. */
export function workspaceClient(url: string): HttpWorkspaceApi {
  let client = clients.get(url);
  if (!client) {
    client = new HttpWorkspaceApi(url);
    clients.set(url, client);
  }
  return client;
}
