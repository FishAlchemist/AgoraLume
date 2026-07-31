import { create } from 'zustand';
import type { PersonaKind } from '../types';

/** A pending confirmation, rendered by the global ConfirmDialog. */
export interface ConfirmRequest {
  /** Optional dialog heading; falls back to a generic title. */
  title?: string;
  message: string;
  /** Label for the confirming action; defaults to a generic "Confirm". */
  confirmLabel?: string;
  /** Tints the confirm button red for destructive actions. */
  danger?: boolean;
  onConfirm: () => void;
}

interface UiState {
  /** Persona whose read-only info card is shown, or null when closed. */
  cardPersonaId: string | null;
  /** Persona whose memory drawer is open, or null when closed. */
  memoryPersonaId: string | null;
  /** Whether the persona editor modal is open. */
  editorOpen: boolean;
  /** Persona being edited; null means "create new". */
  editorPersonaId: string | null;
  /** Which kind to create when editorPersonaId is null. */
  editorKind: PersonaKind;
  /** Pending confirmation dialog, or null when none is open. */
  confirm: ConfirmRequest | null;
  /** Whether the login overlay (see `pages/LoginPage`) is showing. Login is
   * user-triggered, not a gate — the shell stays mounted underneath. */
  loginOpen: boolean;

  openCard: (personaId: string) => void;
  closeCard: () => void;
  openMemory: (personaId: string) => void;
  closeMemory: () => void;
  openEditor: (personaId?: string | null, kind?: PersonaKind) => void;
  closeEditor: () => void;
  askConfirm: (request: ConfirmRequest) => void;
  closeConfirm: () => void;
  openLogin: () => void;
  closeLogin: () => void;
}

/**
 * Transient UI state (not persisted). Lets any avatar open a persona's info
 * card, any surface open the persona editor, and any action raise a unified
 * confirm dialog — all without prop threading or native browser popups.
 */
export const useUi = create<UiState>((set) => ({
  cardPersonaId: null,
  memoryPersonaId: null,
  editorOpen: false,
  editorPersonaId: null,
  editorKind: 'ai',
  confirm: null,
  loginOpen: false,

  openCard: (personaId) => set({ cardPersonaId: personaId }),
  closeCard: () => set({ cardPersonaId: null }),
  openMemory: (personaId) => set({ memoryPersonaId: personaId }),
  closeMemory: () => set({ memoryPersonaId: null }),
  openEditor: (personaId = null, kind = 'ai') =>
    set({ editorOpen: true, editorPersonaId: personaId, editorKind: kind }),
  closeEditor: () => set({ editorOpen: false }),
  askConfirm: (request) => set({ confirm: request }),
  closeConfirm: () => set({ confirm: null }),
  openLogin: () => set({ loginOpen: true }),
  closeLogin: () => set({ loginOpen: false }),
}));
