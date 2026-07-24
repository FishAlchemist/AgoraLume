import { create } from 'zustand';
import { persist } from 'zustand/middleware';

// Build-time default: honour VITE_API_BASE_URL unless the mock is forced. This
// is only the *initial* value — the user can change it at runtime below, and
// the choice is persisted.
const envUrl = import.meta.env.VITE_API_BASE_URL?.trim();
const forceMock = import.meta.env.VITE_USE_MOCK === '1';
const initialBackendUrl = !forceMock && envUrl ? envUrl : null;

interface ConnectionState {
  /** The backend to talk to; `null` means the in-browser mock. */
  backendUrl: string | null;
  /** Sets (or clears, with `null`/empty) the backend URL. Trailing slash trimmed. */
  setBackendUrl: (url: string | null) => void;
}

/**
 * Which data source the app talks to, chosen at runtime and persisted. Kept
 * separate from the workspace store: this is a client/device concern, not part
 * of the (backend-owned) workspace. Changing it re-routes `api` immediately.
 */
export const useConnection = create<ConnectionState>()(
  persist(
    (set) => ({
      backendUrl: initialBackendUrl,
      setBackendUrl: (url) => {
        const trimmed = url?.trim().replace(/\/+$/, '');
        set({ backendUrl: trimmed ? trimmed : null });
      },
    }),
    { name: 'agoralume-connection', version: 1 },
  ),
);
