import { useEffect } from 'react';
import { type BackendStatus, useBackendStatusStore } from '../store/backendStatus';
import { useConnection } from '../store/connection';
import { useWorkspace } from '../store/workspace';
import { api } from './api';

export type { BackendStatus, Reachability } from '../store/backendStatus';

/**
 * Polls the configured backend's `/meta` for liveness + mode, re-checking
 * whenever the configured backend changes, and writes the result into the
 * shared `useBackendStatusStore` (read outside React too — see
 * `store/backendStatus`'s `isGuestFallback`). In-browser mock mode makes no
 * network calls: it reports `local` / `mock` immediately. A backend that only
 * comes up later flips from `offline` to `online` on the next poll.
 */
export function useBackendStatus(intervalMs = 10_000): BackendStatus {
  const backendUrl = useConnection((s) => s.backendUrl);

  useEffect(() => {
    if (!backendUrl) {
      useBackendStatusStore.setState({ reachable: 'local', mock: true, authRequired: false });
      return;
    }
    useBackendStatusStore.setState({
      reachable: 'checking',
      mock: undefined,
      authRequired: undefined,
    });
    let active = true;
    // Track reachability across polls so we can re-pull the workspace on the
    // rising edge — a backend that only comes up *after* we connected wasn't
    // hydrated by the connection switch, so recover it here.
    let wasOffline = false;

    const check = async () => {
      const meta = await api.probe();
      if (!active) return;
      if (meta) {
        if (wasOffline) void useWorkspace.getState().hydrate();
        wasOffline = false;
        useBackendStatusStore.setState({
          reachable: 'online',
          mock: meta.mock,
          authRequired: meta.authRequired,
        });
      } else {
        wasOffline = true;
        useBackendStatusStore.setState({ reachable: 'offline' });
      }
    };

    void check();
    const id = setInterval(check, intervalMs);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, [backendUrl, intervalMs]);

  return useBackendStatusStore();
}
