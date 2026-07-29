import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useReadState } from '../store/readState';
import type { Message } from '../types';

/** How near the bottom (px) still counts as "pinned" — absorbs sub-pixel drift. */
const BOTTOM_SLACK = 80;

export interface ChatReadTracking {
  /** Attach to the ScrollArea viewport. */
  viewport: React.RefObject<HTMLDivElement | null>;
  /** Whether the viewport is currently pinned to (near) the bottom. */
  atBottom: boolean;
  /** Id of the first unseen line — anchors the "new messages" divider, or null. */
  firstUnreadId: string | null;
  /** Count of unseen lines below the fold — the jump button's badge. */
  unreadCount: number;
  /** Wire to ScrollArea's onScrollPositionChange. */
  handleScrollPosition: () => void;
  /** Drop to the newest line and mark everything read (jump button / own send). */
  followToBottom: () => void;
  /** Scroll a specific line back into view (your own message). */
  jumpToMessage: (id: string) => void;
  /** Reset placement so the view re-anchors when history reloads for a new source. */
  resetForReload: () => void;
}

/**
 * Owns the chat viewport's scroll position and the user's unread state for a
 * group. Deliberately never auto-scrolls on incoming lines — a burst of fast
 * replies leaves a reader exactly where they are; catching up is always an
 * explicit act (their send, or the jump button). The read watermark only
 * advances once the newest line is actually visible, and it persists per device
 * so stepping away and returning still surfaces what arrived in the meantime.
 */
export function useChatReadTracking(
  groupId: string,
  ordered: Message[] | null,
  selfId: string | undefined,
  /**
   * True when unread lines exist above the loaded window — the initial page was
   * capped mid-backlog. While set, the divider is held back if the oldest loaded
   * line is itself unread, since the real read boundary hasn't paged in yet.
   */
  unreadAbove = false,
  /**
   * Whether the loaded window includes the live newest line. The read mark only
   * advances while this holds — reading old history after a jump (a window
   * detached from the tail) must never mark live lines seen, since the loaded
   * "newest" isn't actually the newest.
   */
  atTail = true,
): ChatReadTracking {
  const lastReadTs = useReadState((s) => s.lastRead[groupId]);
  const markRead = useReadState((s) => s.markRead);
  const [atBottom, setAtBottom] = useState(true);
  const atBottomRef = useRef(true);
  const atTailRef = useRef(atTail);
  atTailRef.current = atTail;
  const viewport = useRef<HTMLDivElement>(null);
  // The ordered list as of the last render, so imperative callbacks (scroll
  // handlers, timers) never close over a stale snapshot.
  const orderedRef = useRef<Message[] | null>(ordered);
  orderedRef.current = ordered;
  // Guards the one-time "place the view on open" effect against re-running as
  // more lines stream in.
  const didPlace = useRef(false);

  const scrollToBottom = useCallback((behavior: ScrollBehavior = 'smooth') => {
    const el = viewport.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior });
  }, []);

  const markReadNow = useCallback(() => {
    // Detached in old history: the loaded "newest" isn't the real newest, so
    // advancing the mark here would wrongly bury still-unseen live lines.
    if (!atTailRef.current) return;
    const newest = orderedRef.current?.at(-1)?.ts;
    if (newest != null) markRead(groupId, newest);
  }, [groupId, markRead]);

  const handleScrollPosition = useCallback(() => {
    const el = viewport.current;
    if (!el) return;
    const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < BOTTOM_SLACK;
    if (bottom !== atBottomRef.current) {
      atBottomRef.current = bottom;
      setAtBottom(bottom);
    }
    if (bottom) markReadNow();
  }, [markReadNow]);

  const followToBottom = useCallback(() => {
    atBottomRef.current = true;
    setAtBottom(true);
    markReadNow();
    requestAnimationFrame(() => scrollToBottom('smooth'));
  }, [markReadNow, scrollToBottom]);

  const jumpToMessage = useCallback((id: string) => {
    document.getElementById(`msg-${id}`)?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }, []);

  const resetForReload = useCallback(() => {
    didPlace.current = false;
    atBottomRef.current = true;
    setAtBottom(true);
  }, []);

  // The first line newer than the watermark and authored by someone else — it
  // anchors the divider and the on-open placement. Frozen while scrolled up
  // (the watermark only advances at the bottom), so the divider stays put.
  const firstUnreadId = useMemo(() => {
    if (!ordered || lastReadTs == null) return null;
    const idx = ordered.findIndex((m) => m.ts > lastReadTs && m.personaId !== selfId);
    if (idx === -1) return null;
    // If that first unread line is the oldest one loaded and more unread sits
    // above (a capped backlog), the real read boundary is out of the window:
    // hold the divider back rather than draw it here — an earlier page brings the
    // boundary into view, where it renders correctly. Once any read line has
    // paged in above the unread run, idx > 0 and the divider shows as usual.
    if (idx === 0 && unreadAbove) return null;
    return ordered[idx].id;
  }, [ordered, lastReadTs, selfId, unreadAbove]);

  const unreadCount = useMemo(() => {
    if (!ordered || lastReadTs == null) return 0;
    return ordered.reduce((n, m) => (m.ts > lastReadTs && m.personaId !== selfId ? n + 1 : n), 0);
  }, [ordered, lastReadTs, selfId]);

  // Places the view once history first loads: land on the divider if unread
  // lines were left behind (and hold there — don't mark read until the reader
  // actually reaches the bottom); otherwise drop to the newest line and catch up.
  useEffect(() => {
    if (didPlace.current || !ordered || ordered.length === 0) return;
    didPlace.current = true;
    const stored = useReadState.getState().lastRead[groupId];
    const newestTs = ordered.at(-1)?.ts ?? 0;
    const hasUnread =
      stored != null && ordered.some((m) => m.ts > stored && m.personaId !== selfId);
    requestAnimationFrame(() => {
      if (hasUnread) {
        atBottomRef.current = false;
        setAtBottom(false);
        document.getElementById('chat-unread-divider')?.scrollIntoView({ block: 'center' });
      } else {
        markRead(groupId, newestTs);
        scrollToBottom('auto');
      }
    });
  }, [ordered, groupId, selfId, markRead, scrollToBottom]);

  // A fresh line never scrolls the view — however fast a burst lands, the reader
  // keeps their place. We only re-measure: if the new line pushed past the fold
  // the jump button surfaces with the unread count; if everything still fits,
  // there's nothing unseen, so mark it read.
  // biome-ignore lint/correctness/useExhaustiveDependencies: ordered is the intended trigger — react to new lines, not to every helper identity.
  useEffect(() => {
    if (!didPlace.current) return;
    requestAnimationFrame(() => {
      const el = viewport.current;
      if (!el) return;
      const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < BOTTOM_SLACK;
      atBottomRef.current = bottom;
      setAtBottom(bottom);
      if (bottom) markReadNow();
    });
  }, [ordered]);

  return {
    viewport,
    atBottom,
    firstUnreadId,
    unreadCount,
    handleScrollPosition,
    followToBottom,
    jumpToMessage,
    resetForReload,
  };
}
