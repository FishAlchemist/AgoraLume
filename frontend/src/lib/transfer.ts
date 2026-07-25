import type { Department, Group, Organization, Persona } from '../types';

/** Serialised backup of personas plus the organizations/departments they need. */
export interface PersonaBundle {
  type: 'agoralume-personas';
  version: 1;
  exportedAt: string;
  organizations: Organization[];
  departments: Department[];
  personas: Persona[];
}

/**
 * Serialised backup of one or more groups, self-contained: it carries the AI
 * personas each group references and those personas' organizations/departments,
 * so an import restores a working group. User identities are never exported (a
 * group's "you" is remapped to a local identity on import).
 */
export interface GroupBundle {
  type: 'agoralume-groups';
  version: 1;
  exportedAt: string;
  organizations: Organization[];
  departments: Department[];
  personas: Persona[];
  groups: Group[];
}

const BUNDLE_TYPE = 'agoralume-personas';
const GROUP_BUNDLE_TYPE = 'agoralume-groups';

/**
 * Builds a self-contained bundle for the given personas, pulling in only the
 * organizations and departments they reference so the export restores cleanly.
 * The local user persona is never exported.
 */
export function buildBundle(
  personas: Persona[],
  organizations: Organization[],
  departments: Department[],
): PersonaBundle {
  const exportable = personas.filter((p) => p.kind !== 'user');
  const orgIds = new Set(exportable.map((p) => p.organizationId).filter(Boolean));
  const deptIds = new Set(exportable.map((p) => p.departmentId).filter(Boolean));
  // A referenced department also pulls in its parent organization.
  const usedDepartments = departments.filter((d) => deptIds.has(d.id));
  for (const d of usedDepartments) orgIds.add(d.organizationId);

  return {
    type: BUNDLE_TYPE,
    version: 1,
    exportedAt: new Date().toISOString(),
    organizations: organizations.filter((o) => orgIds.has(o.id)),
    departments: usedDepartments,
    personas: exportable,
  };
}

/**
 * Builds a self-contained backup of the given groups: each group's AI personas
 * (user identities excluded), then those personas' organizations/departments.
 * Group membership is trimmed to the AI personas actually carried.
 */
export function buildGroupBundle(
  groups: Group[],
  personas: Persona[],
  organizations: Organization[],
  departments: Department[],
): GroupBundle {
  const personaById = new Map(personas.map((p) => [p.id, p]));
  const aiIds = new Set<string>();
  for (const group of groups) {
    for (const pid of group.personaIds) {
      const persona = personaById.get(pid);
      if (persona && persona.kind !== 'user') aiIds.add(pid);
    }
  }
  const exportablePersonas = personas.filter((p) => aiIds.has(p.id));
  const orgIds = new Set(exportablePersonas.map((p) => p.organizationId).filter(Boolean));
  const deptIds = new Set(exportablePersonas.map((p) => p.departmentId).filter(Boolean));
  const usedDepartments = departments.filter((d) => deptIds.has(d.id));
  for (const d of usedDepartments) orgIds.add(d.organizationId);

  return {
    type: GROUP_BUNDLE_TYPE,
    version: 1,
    exportedAt: new Date().toISOString(),
    organizations: organizations.filter((o) => orgIds.has(o.id)),
    departments: usedDepartments,
    personas: exportablePersonas,
    // Trim membership to the AI personas the bundle carries; the self identity
    // is dropped here and remapped to a local one on import.
    groups: groups.map((g) => ({ ...g, personaIds: g.personaIds.filter((pid) => aiIds.has(pid)) })),
  };
}

/** Triggers a browser download of the bundle as a JSON file. */
export function downloadBundle(bundle: PersonaBundle | GroupBundle, filename: string): void {
  const blob = new Blob([JSON.stringify(bundle, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}

/** Parses and validates bundle JSON. Throws on anything unexpected. */
export function parseBundle(text: string): PersonaBundle {
  const data: unknown = JSON.parse(text);
  if (
    typeof data !== 'object' ||
    data === null ||
    (data as { type?: unknown }).type !== BUNDLE_TYPE
  ) {
    throw new Error('invalid-bundle');
  }
  const b = data as Partial<PersonaBundle>;
  return {
    type: BUNDLE_TYPE,
    version: 1,
    exportedAt: typeof b.exportedAt === 'string' ? b.exportedAt : new Date().toISOString(),
    organizations: Array.isArray(b.organizations) ? b.organizations : [],
    departments: Array.isArray(b.departments) ? b.departments : [],
    personas: Array.isArray(b.personas) ? b.personas : [],
  };
}

/** Parses and validates group-bundle JSON. Throws on anything unexpected. */
export function parseGroupBundle(text: string): GroupBundle {
  const data: unknown = JSON.parse(text);
  if (
    typeof data !== 'object' ||
    data === null ||
    (data as { type?: unknown }).type !== GROUP_BUNDLE_TYPE
  ) {
    throw new Error('invalid-bundle');
  }
  const b = data as Partial<GroupBundle>;
  return {
    type: GROUP_BUNDLE_TYPE,
    version: 1,
    exportedAt: typeof b.exportedAt === 'string' ? b.exportedAt : new Date().toISOString(),
    organizations: Array.isArray(b.organizations) ? b.organizations : [],
    departments: Array.isArray(b.departments) ? b.departments : [],
    personas: Array.isArray(b.personas) ? b.personas : [],
    groups: Array.isArray(b.groups) ? b.groups : [],
  };
}

/** A filesystem-safe slug for export filenames. */
export function slugify(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, '-')
    .replace(/^-+|-+$/g, '');
  return slug || 'personas';
}
