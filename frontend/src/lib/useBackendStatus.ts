import { useEffect, useState } from 'react';
import { api, usingMock } from './api';

/**
 * Reachability of the data source, kept separate from its *mode*:
 * - `local`    — the in-browser mock (no server; nothing to reach)
 * - `checking` — a backend is configured, first probe in flight
 * - `online`   — the backend answered
 * - `offline`  — the backend is unreachable
 */
export type Reachability = 'local' | 'checking' | 'online' | 'offline';

export interface BackendStatus {
  reachable: Reachability;
  /**
   * Mock = no LLM and no persistence (pure in-memory). True for the in-browser
   * mock and for a backend running in mock mode; independent of reachability.
   * Undefined while `checking`/`offline` (mode unknown).
   */
  mock?: boolean;
}

/**
 * Polls the configured backend's `/meta` for liveness + mode. In-browser mock
 * mode makes no network calls: it reports `local` / `mock` immediately.
 */
export function useBackendStatus(intervalMs = 10_000): BackendStatus {
  const [status, setStatus] = useState<BackendStatus>(
    usingMock ? { reachable: 'local', mock: true } : { reachable: 'checking' },
  );

  useEffect(() => {
    if (usingMock) return; // constant 'local' — no polling.
    let active = true;

    const check = async () => {
      const meta = await api.probe();
      if (!active) return;
      setStatus(meta ? { reachable: 'online', mock: meta.mock } : { reachable: 'offline' });
    };

    void check();
    const id = setInterval(check, intervalMs);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, [intervalMs]);

  return status;
}
