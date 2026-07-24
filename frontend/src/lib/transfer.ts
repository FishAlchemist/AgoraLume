import type { Department, Organization, Persona } from '../types';

/** Serialised backup of personas plus the organizations/departments they need. */
export interface PersonaBundle {
  type: 'agoralume-personas';
  version: 1;
  exportedAt: string;
  organizations: Organization[];
  departments: Department[];
  personas: Persona[];
}

const BUNDLE_TYPE = 'agoralume-personas';

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

/** Triggers a browser download of the bundle as a JSON file. */
export function downloadBundle(bundle: PersonaBundle, filename: string): void {
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

/** A filesystem-safe slug for export filenames. */
export function slugify(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, '-')
    .replace(/^-+|-+$/g, '');
  return slug || 'personas';
}
