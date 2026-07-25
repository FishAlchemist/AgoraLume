import type { Persona } from '../types';

/**
 * Length caps for a persona's name and blurb. Kept short so they can't bloat the
 * agent prompt (names appear in the roster, blurbs in the `<directory>`) or the
 * UI. Applied as input `maxLength` in the persona and profile editors.
 */
export const MAX_PERSONA_NAME_LEN = 40;
export const MAX_PERSONA_BLURB_LEN = 200;

/**
 * Whether another persona (any except `exceptId`) already uses `name`, matched
 * case-insensitively and trimmed. Names are globally unique — the lookup the
 * agent does by name relies on it, and the backend enforces the same with a 409.
 */
export function isNameTaken(personas: Persona[], name: string, exceptId?: string): boolean {
  const n = name.trim().toLowerCase();
  if (!n) return false;
  return personas.some((p) => p.id !== exceptId && p.name.trim().toLowerCase() === n);
}
