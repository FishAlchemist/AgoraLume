import type { Department, Organization, Persona, Settings } from '../types';

/** Placeholder names the app always provides, regardless of user variables. */
export const BUILTIN_VARIABLE_NAMES = [
  'user_language',
  'persona_name',
  'org_name',
  'department_name',
] as const;

/**
 * Computes the effective variable map for a persona. Precedence, lowest to
 * highest: built-ins → organization → department → persona. So a persona can
 * override its department, which overrides its organization, which overrides
 * the built-ins.
 */
export function resolveVariables(
  persona: Persona,
  organization: Organization | undefined,
  department: Department | undefined,
  settings: Settings,
): Record<string, string> {
  return {
    user_language: settings.nativeLanguage,
    persona_name: persona.name,
    org_name: organization?.name ?? '',
    department_name: department?.name ?? '',
    ...(organization?.variables ?? {}),
    ...(department?.variables ?? {}),
    ...(persona.variables ?? {}),
  };
}

const PLACEHOLDER = /\{\{\s*([\w.-]+)\s*\}\}/g;

/**
 * Substitutes {{name}} placeholders in a template using the given variables.
 * Unknown placeholders are left untouched so authors can spot typos.
 */
export function applyTemplate(template: string, variables: Record<string, string>): string {
  return template.replace(PLACEHOLDER, (whole, name: string) =>
    name in variables ? variables[name] : whole,
  );
}

/** Resolves a persona's system prompt with all variables applied. */
export function resolveSystemPrompt(
  persona: Persona,
  organization: Organization | undefined,
  department: Department | undefined,
  settings: Settings,
): string {
  return applyTemplate(
    persona.systemPrompt ?? '',
    resolveVariables(persona, organization, department, settings),
  );
}
