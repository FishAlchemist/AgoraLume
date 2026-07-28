import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface ReadState {
  /**
   * Per group, the timestamp (epoch ms) of the newest message the user has
   * actually seen — the "read up to" watermark. Anything newer than this is
   * unread. Persisted per device so stepping away and coming back later still
   * surfaces what arrived while you were gone.
   */
  lastRead: Record<string, number>;
  /** Advances the watermark for a group. Never rewinds — only moves forward. */
  markRead: (groupId: string, ts: number) => void;
}

/**
 * The user's read position in each group's conversation. Like the theme and the
 * backend connection, this is a per-device concern: it lives in localStorage and
 * is never sent to the backend, so what one browser has caught up on never
 * affects anyone else viewing the same instance.
 */
export const useReadState = create<ReadState>()(
  persist(
    (set) => ({
      lastRead: {},
      markRead: (groupId, ts) =>
        set((s) => {
          const current = s.lastRead[groupId] ?? 0;
          if (ts <= current) return s;
          return { lastRead: { ...s.lastRead, [groupId]: ts } };
        }),
    }),
    { name: 'agoralume-read-state', version: 1 },
  ),
);
