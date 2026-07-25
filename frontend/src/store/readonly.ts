import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface ReadOnlyState {
  /** When true, the UI hides every write action — a viewing-only mode. */
  readOnly: boolean;
  setReadOnly: (value: boolean) => void;
}

/**
 * A per-device, view-only mode. Like the backend connection and the theme, this
 * is a client/device concern — persisted locally and never sent to the backend,
 * so switching it changes only this browser and never what other people see.
 *
 * It is a UI affordance, not enforcement: it hides the write controls so the
 * owner can watch a shared instance without touching it. It does not (and can
 * not) stop a determined client from calling the API directly.
 */
export const useReadOnly = create<ReadOnlyState>()(
  persist(
    (set) => ({
      readOnly: false,
      setReadOnly: (value) => set({ readOnly: value }),
    }),
    { name: 'agoralume-readonly', version: 1 },
  ),
);
