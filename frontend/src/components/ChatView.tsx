import { Box, Center, Loader, ScrollArea, Stack, Text, UnstyledButton } from '@mantine/core';
import { IconArrowDown } from '@tabler/icons-react';
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '../lib/api';
import { INITIAL_PAGE_CAP } from '../lib/api/types';
import { useChatReadTracking } from '../lib/useChatReadTracking';
import { useIsGuestFallback } from '../store/backendStatus';
import { useConnection } from '../store/connection';
import { useReadOnly } from '../store/readonly';
import { useReadState } from '../store/readState';
import { useWorkspace } from '../store/workspace';
import type { Group, Message, Persona, Turn } from '../types';
import { Composer } from './Composer';
import { MessageItem } from './MessageItem';
import { ReadProgressBar } from './ReadProgressBar';
import { SuggestionChips } from './SuggestionChips';

interface Props {
  group: Group;
  personas: Map<string, Persona>;
}

// A manual "suggest other ideas" always shows the loader for at least this long,
// so a fast backend swap reads as a real refresh rather than an unchanged flicker.
const REGEN_MIN_MS = 1000;
// If no fresh set arrives within this window — the server coalesced the request
// inside its cooldown, so no `suggestions` frame follows — restore what we cleared
// rather than leaving the user on a spinner forever.
const REGEN_TIMEOUT_MS = 8000;

// How many lines the initial open loads, and how many each "load earlier" page
// pulls. The initial page is extended past this when there are more unread lines,
// so the whole unread run is always present; older history is fetched on demand.
const PAGE_SIZE = 40;

// How near either end of the loaded window (px) starts pulling the next page in,
// so scrolling flows into unloaded history instead of stopping at a button.
const AUTO_LOAD_MARGIN = 320;

/** Appends a streamed message, replacing any existing entry with the same id. */
function appendMessage(prev: Message[] | null, message: Message): Message[] {
  if (!prev) return [message];
  if (prev.some((m) => m.id === message.id)) {
    return prev.map((m) => (m.id === message.id ? message : m));
  }
  return [...prev, message];
}

/** Prepends an older page ahead of the loaded lines, dropping any known ids. */
function prependMessages(prev: Message[] | null, older: Message[]): Message[] {
  if (!prev || prev.length === 0) return older;
  const have = new Set(prev.map((m) => m.id));
  const fresh = older.filter((m) => !have.has(m.id));
  return fresh.length > 0 ? [...fresh, ...prev] : prev;
}

/** Appends a newer page after the loaded lines, dropping any known ids. */
function appendMessages(prev: Message[] | null, newer: Message[]): Message[] {
  if (!prev || prev.length === 0) return newer;
  const have = new Set(prev.map((m) => m.id));
  const fresh = newer.filter((m) => !have.has(m.id));
  return fresh.length > 0 ? [...prev, ...fresh] : prev;
}

/**
 * Fetches a window centred on a jump target and the flags that describe where it
 * sits: whether earlier/later pages remain, and whether it reached the true tail
 * (fewer lines after the target than asked for). `null` when the line is gone.
 */
async function fetchJumpWindow(groupId: string, id: string) {
  const win = await api.listMessages(groupId, { anchor: id, before: PAGE_SIZE, after: PAGE_SIZE });
  const at = win.findIndex((m) => m.id === id);
  if (at === -1) return null;
  const after = win.length - at - 1;
  return {
    win,
    hasEarlier: at >= PAGE_SIZE,
    hasLater: after >= PAGE_SIZE,
    atTail: after < PAGE_SIZE,
  };
}

/**
 * Merges buffered read receipts into whichever messages are present. A receipt
 * can arrive before its target message — the turn streams reads as soon as it
 * runs, which races the POST response that inserts the user's line, and the two
 * travel over separate connections — so every receipt is buffered and re-applied
 * on each update, and a late message still picks up the reads it already earned.
 */
function applyReads(messages: Message[] | null, reads: Map<string, Set<string>>): Message[] | null {
  if (!messages) return messages;
  return messages.map((m) => {
    if (m.kind !== 'conversation') return m;
    const seen = reads.get(m.id);
    if (!seen || seen.size === 0) return m;
    const merged = new Set(m.readBy ?? []);
    const before = merged.size;
    for (const id of seen) merged.add(id);
    return merged.size === before ? m : { ...m, readBy: [...merged] };
  });
}

/** The trailing sequence in an `m<ts>-<seq>` id — a process-monotonic counter. */
function seqOf(id: string): number {
  const n = Number(id.slice(id.lastIndexOf('-') + 1));
  return Number.isFinite(n) ? n : 0;
}

/**
 * Orders chat lines oldest-first. Streamed frames and the POST response race
 * across separate connections, so arrival order is unreliable; the server clock
 * (shared with the client) is authoritative. An instant mock brain can stamp
 * several messages in the same millisecond, so the monotonic id sequence breaks
 * ties — keeping the user's line ahead of the replies it triggered.
 */
function compareMessages(a: Message, b: Message): number {
  return a.ts - b.ts || seqOf(a.id) - seqOf(b.id);
}

/** Local midnight for a timestamp, so two messages compare equal iff same calendar day. */
function startOfDay(ts: number): number {
  const d = new Date(ts);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/**
 * A day-separator label for the divider between messages that cross midnight:
 * "Today"/"Yesterday" for the two most recent days, otherwise a full local date
 * (year dropped when it's the current year) so the reader can place each message.
 */
function formatDayLabel(ts: number, locale: string, t: (key: string) => string): string {
  const day = startOfDay(ts);
  const today = startOfDay(Date.now());
  const diffDays = Math.round((today - day) / 86_400_000);
  if (diffDays === 0) return t('chat.today');
  if (diffDays === 1) return t('chat.yesterday');
  const d = new Date(ts);
  return d.toLocaleDateString(locale, {
    year: d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
    month: 'long',
    day: 'numeric',
    weekday: 'long',
  });
}

export function ChatView({ group, personas }: Props) {
  const { t, i18n } = useTranslation();
  const fontSize = useWorkspace((s) => s.settings.chatFontSize ?? 15);
  const readOnly = useReadOnly((s) => s.readOnly);
  // Re-bind history + streams when the active data source changes.
  const backendUrl = useConnection((s) => s.backendUrl);
  const isGuestFallback = useIsGuestFallback();

  // Fall back to any user identity so a group without an explicit self still works.
  const selfId = group.selfPersonaId || [...personas.values()].find((p) => p.kind === 'user')?.id;
  // AI members drive read receipts, replies, and the "locked" state.
  const aiMembers = useMemo(
    () =>
      group.personaIds
        .map((id) => personas.get(id))
        .filter((p): p is Persona => !!p && p.kind === 'ai'),
    [group.personaIds, personas],
  );
  const aiMemberCount = aiMembers.length;
  const locked = aiMemberCount === 0;

  const [messages, setMessages] = useState<Message[] | null>(null);
  // History loads as a contiguous window that can grow either way: the initial
  // tail (extended to cover the whole unread run), earlier pages, later pages, or
  // a window fetched around a jump target. `hasEarlier`/`hasLater` gate the two
  // end loaders.
  const [hasEarlier, setHasEarlier] = useState(false);
  const loadingEarlierRef = useRef(false);
  const [hasLater, setHasLater] = useState(false);
  const loadingLaterRef = useRef(false);
  // Whether the loaded window includes the live newest line. False after a jump
  // into old history: streamed lines are then held out of the detached window (a
  // gap would corrupt it) and the read mark is frozen, until the reader returns to
  // the tail (which reloads it). A ref mirrors it for the stream callback, set up
  // once on mount, to read the current value without re-subscribing.
  const [atTail, setAtTail] = useState(true);
  const atTailRef = useRef(true);
  // True while a jump is fetching the window around an off-screen target — drives
  // a spinner so the jump never looks like it silently did nothing.
  const [jumping, setJumping] = useState(false);
  // A jump target whose window just loaded; the layout effect scrolls to it once
  // the replaced window has rendered.
  const pendingJumpId = useRef<string | null>(null);
  // True when the initial page was capped mid-unread — i.e. more unread lines
  // exist above the loaded window (a large backlog was truncated). It lets the
  // read tracking hold the "new messages" divider back until an earlier page
  // brings the true read boundary into view, rather than drawing it at the top of
  // a window that is entirely unread.
  const [unreadAbove, setUnreadAbove] = useState(false);
  // Pre-prepend scroll metrics, so the layout effect can hold the reader's place
  // when an earlier page grows the content above them.
  const pendingAnchor = useRef<{ height: number; top: number } | null>(null);
  // The agent loop's busy/idle state, driven by the backend's activity signal.
  // The composer stays locked while busy, so a message can't interleave a turn.
  const [busy, setBusy] = useState(false);
  // The current processing round — trigger + per-member progress — streamed from
  // the backend and seeded on connect, independently of the loaded history. It,
  // not the message window, drives the pinned progress bar, so the bar shows even
  // when the trigger line has paged out and for event triggers with no message.
  const [currentTurn, setCurrentTurn] = useState<Turn | null>(null);
  // Server-generated conversation openers; the frontend only fetches & displays.
  const [suggestions, setSuggestions] = useState<string[]>([]);
  // A chip click pushes its text into the composer (never sends). The nonce lets
  // the same suggestion re-fill after it was edited or cleared.
  const [fill, setFill] = useState<{ text: string; nonce: number }>();
  // True while an explicit "suggest other ideas" is in flight: the old chips are
  // cleared and a loader shows until the fresh set arrives (min REGEN_MIN_MS).
  const [regenerating, setRegenerating] = useState(false);
  // The in-flight manual regenerate: when it started, the openers to restore if it
  // yields nothing, and its safety timer. null when no manual refresh is pending.
  const regen = useRef<{ startedAt: number; prev: string[]; timer: number } | null>(null);
  // Buffered read receipts, keyed by message id — decouples receipts from the
  // arrival of their target message so none are lost to the SSE/POST race.
  const reads = useRef<Map<string, Set<string>>>(new Map());

  const ordered = useMemo(
    () => (messages ? [...messages].sort(compareMessages) : messages),
    [messages],
  );

  // Keep the ref the stream callback reads in sync with the atTail state.
  useEffect(() => {
    atTailRef.current = atTail;
  }, [atTail]);

  // Owns the scroll viewport and the user's unread state (divider, jump button,
  // read watermark). Never auto-scrolls on incoming lines — catching up is always
  // an explicit act — which is the "don't yank me down mid-read" behaviour. The
  // read mark only advances while the window is at the tail, so reading old history
  // after a jump never marks live lines seen.
  const {
    viewport,
    atBottom,
    firstUnreadId,
    unreadCount,
    handleScrollPosition,
    followToBottom,
    jumpToMessage,
    resetForReload,
  } = useChatReadTracking(group.id, ordered, selfId, unreadAbove, atTail);

  // Applies a freshly-loaded tail window: the newest lines (extended over the
  // unread run), reconnected to the live stream. Shared by the initial open and
  // the "return to latest" reload.
  const applyTail = useCallback((tail: Message[]) => {
    setMessages(applyReads(tail, reads.current));
    setHasEarlier(tail.length >= PAGE_SIZE);
    setUnreadAbove(tail.length >= INITIAL_PAGE_CAP);
    setHasLater(false);
    setAtTail(true);
  }, []);

  // Reloads the newest window and drops to the bottom — how a detached window (a
  // jump into old history) rejoins the live stream, and how "return to latest"
  // catches up. Reading the mark imperatively keeps it out of the deps.
  const reloadTail = useCallback(() => {
    const since = useReadState.getState().lastRead[group.id];
    return api
      .listMessages(group.id, { before: PAGE_SIZE, since })
      .then((tail) => {
        applyTail(tail);
        followToBottom();
      })
      .catch(() => {});
  }, [group.id, applyTail, followToBottom]);

  // The "return to latest" pill: at the tail it just drops to the bottom; detached
  // in old history it reloads the tail first, reconnecting to the live stream.
  const returnToLatest = useCallback(() => {
    if (atTailRef.current) {
      followToBottom();
      return;
    }
    void reloadTail();
  }, [followToBottom, reloadTail]);

  // Ends an in-flight manual regenerate: cancels its safety timer and drops the
  // loader. Leaves whatever suggestions are currently shown in place.
  const finishRegen = useCallback(() => {
    const r = regen.current;
    if (r) window.clearTimeout(r.timer);
    regen.current = null;
    setRegenerating(false);
  }, []);

  // Applies a freshly-arrived suggestion set (delivered on the SSE frame). When a
  // manual refresh is pending, hold the loader for at least REGEN_MIN_MS so the
  // swap is perceptible; otherwise (open/turn-end regen) apply it immediately.
  const applySuggestions = useCallback(
    (prompts: string[]) => {
      const r = regen.current;
      if (!r) {
        setSuggestions(prompts);
        return;
      }
      const wait = Math.max(0, REGEN_MIN_MS - (Date.now() - r.startedAt));
      window.setTimeout(() => {
        setSuggestions(prompts);
        finishRegen();
      }, wait);
    },
    [finishRegen],
  );

  // The refresh button: clear the current chips, show the loader, and ask the
  // backend for a new set (rate-limited server-side). The fresh set arrives on the
  // `suggestions` frame; a safety timer restores the old chips if none does.
  const handleRegenerate = useCallback(() => {
    if (regen.current) return; // one already pending
    const timer = window.setTimeout(() => {
      const r = regen.current;
      if (r) setSuggestions(r.prev);
      finishRegen();
    }, REGEN_TIMEOUT_MS);
    regen.current = { startedAt: Date.now(), prev: suggestions, timer };
    setRegenerating(true);
    setSuggestions([]);
    void api.regenerateSuggestions(group.id).catch(() => {
      const r = regen.current;
      if (r) setSuggestions(r.prev);
      finishRegen();
    });
  }, [suggestions, group.id, finishRegen]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: backendUrl and isGuestFallback are intentional triggers — a change re-runs this so history and subscriptions rebind to the newly selected data source (connecting to a different backend, or the guest/session state flipping which one `api` actually routes to).
  useEffect(() => {
    let active = true;
    setMessages(null);
    setHasEarlier(false);
    setHasLater(false);
    setUnreadAbove(false);
    loadingEarlierRef.current = false;
    loadingLaterRef.current = false;
    setAtTail(true);
    atTailRef.current = true;
    setJumping(false);
    pendingJumpId.current = null;
    pendingAnchor.current = null;
    setBusy(false);
    setCurrentTurn(null);
    setSuggestions([]);
    finishRegen();
    reads.current = new Map();
    // Re-anchor the view when history reloads for a new data source.
    resetForReload();

    // Load the newest page, extended back far enough to include everything newer
    // than the read mark — so the whole unread run is present and its divider and
    // count stay exact. Earlier lines are fetched on demand as the reader scrolls
    // up. Reading the mark imperatively keeps it out of this effect's deps.
    const since = useReadState.getState().lastRead[group.id];
    void api.listMessages(group.id, { before: PAGE_SIZE, since }).then((initial) => {
      if (!active) return;
      applyTail(initial);
    });

    // Fetch the cached openers on open; a stale set kicks a background regen on
    // the backend, whose result then arrives on the `suggestions` frame below.
    void api.getSuggestions(group.id).then((s) => {
      if (active) setSuggestions(s.prompts);
    });

    const unsubscribeSuggestions = api.subscribeSuggestions(group.id, (s) => {
      applySuggestions(s.prompts);
    });

    const unsubscribe = api.subscribe(group.id, (message) => {
      // Held out while the window is detached in old history — appending here would
      // gap the timeline. The reader picks these up when they return to the tail,
      // which reloads it fresh; buffered receipts still apply on the way back.
      if (!atTailRef.current) return;
      setMessages((prev) => applyReads(appendMessage(prev, message), reads.current));
    });

    const unsubscribeReads = api.subscribeReads(group.id, (receipt) => {
      // Buffer first, then re-apply — a receipt that beats its message still lands.
      const seen = reads.current.get(receipt.messageId) ?? new Set<string>();
      seen.add(receipt.personaId);
      reads.current.set(receipt.messageId, seen);
      setMessages((prev) => applyReads(prev, reads.current));
    });

    // The composer unlocks only when the whole agent loop reports idle.
    const unsubscribeActivity = api.subscribeActivity(group.id, setBusy);

    // The pinned progress bar's source of truth: the backend seeds the current
    // turn on connect and pushes an update on every member's progress.
    const unsubscribeTurn = api.subscribeTurn(group.id, setCurrentTurn);

    return () => {
      active = false;
      unsubscribe();
      unsubscribeReads();
      unsubscribeActivity();
      unsubscribeTurn();
      unsubscribeSuggestions();
      finishRegen();
    };
  }, [group.id, backendUrl, isGuestFallback]);

  // When a turn ends (busy → idle) the conversation has advanced, so the openers
  // are stale. Re-fetch to nudge the backend into regenerating for the new state;
  // the fresh set streams back on the `suggestions` frame.
  const wasBusy = useRef(false);
  useEffect(() => {
    if (wasBusy.current && !busy) {
      void api
        .getSuggestions(group.id)
        .then((s) => setSuggestions(s.prompts))
        .catch(() => {});
    }
    wasBusy.current = busy;
  }, [busy, group.id]);

  const handleSend = async (text: string) => {
    if (locked || busy) return;
    // Lock immediately; the backend's idle activity signal clears it once the
    // whole turn is done. On failure, unlock so the user can retry.
    setBusy(true);
    try {
      const message = await api.sendMessage(group.id, text, selfId);
      if (atTailRef.current) {
        setMessages((prev) => applyReads(appendMessage(prev, message), reads.current));
        // The user just acted, so follow their own line down and mark caught up —
        // the replies it triggers then stream in beneath, without dragging the view.
        followToBottom();
      } else {
        // Sending from old history returns us to the live tail, with the new line.
        await reloadTail();
      }
    } catch {
      setBusy(false);
    }
  };

  // Resumes a turn suspended by a failed agent. We don't lock optimistically:
  // the backend's `activity` signal locks the composer only if there is actually
  // something to resume, so a stale retry (already voided) can't wedge the UI.
  const handleRetry = useCallback(() => {
    if (busy) return;
    void api.retry(group.id).catch(() => {});
  }, [busy, group.id]);

  // The two ends of the loaded window — the anchors for growing it either way.
  const oldestLoadedId = ordered?.[0]?.id;
  const newestLoadedId = ordered?.at(-1)?.id;

  // Fetches the page just before the oldest loaded line and prepends it. Records
  // the scroll metrics first so the layout effect can hold the reader's place; a
  // short page (nothing new past the anchor) means the log's start is reached, so
  // the loader retires. The anchor line comes back in the page and is deduped.
  const handleLoadEarlier = useCallback(() => {
    if (!oldestLoadedId || loadingEarlierRef.current) return;
    loadingEarlierRef.current = true;
    const el = viewport.current;
    const anchorHeight = el?.scrollHeight ?? 0;
    const anchorTop = el?.scrollTop ?? 0;
    void api
      .listMessages(group.id, { anchor: oldestLoadedId, before: PAGE_SIZE })
      .then((older) => {
        const fresh = older.filter((m) => m.id !== oldestLoadedId);
        setHasEarlier(fresh.length >= PAGE_SIZE);
        if (fresh.length > 0) {
          pendingAnchor.current = { height: anchorHeight, top: anchorTop };
          setMessages((prev) => applyReads(prependMessages(prev, fresh), reads.current));
        }
      })
      .catch(() => {})
      .finally(() => {
        loadingEarlierRef.current = false;
      });
  }, [oldestLoadedId, group.id, viewport]);

  // Fetches the page just after the newest loaded line and appends it. A short page
  // means the true tail is reached, so the window rejoins the live stream (`atTail`)
  // and the loader retires. The anchor line comes back in the page and is deduped.
  const handleLoadLater = useCallback(() => {
    if (!newestLoadedId || loadingLaterRef.current) return;
    loadingLaterRef.current = true;
    void api
      .listMessages(group.id, { anchor: newestLoadedId, after: PAGE_SIZE })
      .then((newer) => {
        const fresh = newer.filter((m) => m.id !== newestLoadedId);
        // A full page means more remains below (still detached); a short one means
        // the true tail is reached, rejoining the live stream.
        const more = fresh.length >= PAGE_SIZE;
        setHasLater(more);
        setAtTail(!more);
        if (fresh.length > 0) {
          setMessages((prev) => applyReads(appendMessages(prev, fresh), reads.current));
        }
      })
      .catch(() => {})
      .finally(() => {
        loadingLaterRef.current = false;
      });
  }, [newestLoadedId, group.id]);

  // Continuous scroll: approaching either end pulls the next page in, so history
  // slides in without a click. Each page is gated by its ref (no re-entry) and by
  // `hasEarlier`/`hasLater` (so a settled end stops fetching); after a prepend the
  // layout effect shifts scrollTop off the top edge, so it won't re-fire in a loop.
  const maybeAutoLoad = useCallback(() => {
    const el = viewport.current;
    if (!el) return;
    if (hasEarlier && el.scrollTop < AUTO_LOAD_MARGIN) {
      handleLoadEarlier();
    } else if (hasLater && el.scrollHeight - el.scrollTop - el.clientHeight < AUTO_LOAD_MARGIN) {
      handleLoadLater();
    }
  }, [viewport, hasEarlier, hasLater, handleLoadEarlier, handleLoadLater]);

  // One scroll handler for the viewport: update the read/at-bottom state, then see
  // whether nearing an edge should page more in.
  const handleScroll = useCallback(() => {
    handleScrollPosition();
    maybeAutoLoad();
  }, [handleScrollPosition, maybeAutoLoad]);

  // Jumps to a line — a turn's trigger, an avatar's reply, later a search hit.
  // When it's already loaded, a plain scroll (instant). When it isn't (it paged
  // out, or was never in the window), fetch a window centred on it, replace the
  // view, and scroll to it once rendered — a spinner covers the fetch so the jump
  // never looks like it silently failed. The fetched window may sit in old history
  // (detached from the tail), which the reader leaves via "return to latest".
  const handleJump = useCallback(
    async (id: string) => {
      if (ordered?.some((m) => m.id === id)) {
        jumpToMessage(id);
        return;
      }
      setJumping(true);
      try {
        const w = await fetchJumpWindow(group.id, id);
        if (!w) return; // the line is gone
        setMessages(applyReads(w.win, reads.current));
        setHasEarlier(w.hasEarlier);
        setHasLater(w.hasLater);
        setAtTail(w.atTail);
        setUnreadAbove(false);
        pendingJumpId.current = id;
      } finally {
        setJumping(false);
      }
    },
    [ordered, group.id, jumpToMessage],
  );

  // After a window changes, restore the reader's place. A jump scrolls its target
  // to centre once the replaced window has rendered; an earlier page grew the
  // content above the reader, so shift scrollTop by that delta to hold them put.
  // Ordinary appends set neither ref (a no-op).
  // biome-ignore lint/correctness/useExhaustiveDependencies: messages is the intended trigger — re-run on a window change; viewport.current is a stable DOM ref read imperatively.
  useLayoutEffect(() => {
    const jumpId = pendingJumpId.current;
    if (jumpId) {
      pendingJumpId.current = null;
      pendingAnchor.current = null;
      document
        .getElementById(`msg-${jumpId}`)
        ?.scrollIntoView({ block: 'center', behavior: 'smooth' });
      return;
    }
    const anchor = pendingAnchor.current;
    if (!anchor) return;
    pendingAnchor.current = null;
    const el = viewport.current;
    if (!el) return;
    el.scrollTop = anchor.top + (el.scrollHeight - anchor.height);
  }, [messages]);

  // For each of your messages, which AI ids replied to it (until the next of yours).
  const replyMap = useMemo(() => {
    const map = new Map<string, Set<string>>();
    if (!ordered) return map;
    const aiIdSet = new Set(aiMembers.map((p) => p.id));
    let anchor: string | null = null;
    for (const m of ordered) {
      if (m.kind === 'conversation' && m.personaId === selfId) {
        anchor = m.id;
        map.set(anchor, new Set());
      } else if (m.kind === 'conversation' && anchor && aiIdSet.has(m.personaId)) {
        map.get(anchor)?.add(m.personaId);
      }
    }
    return map;
  }, [ordered, selfId, aiMembers]);

  // Bucket the ordered lines into consecutive same-day runs. Each run renders as
  // its own block with a sticky date header, so the day stays pinned at the top
  // of the viewport the whole time you're reading that day — and is handed off to
  // the next day only once you scroll its block away.
  const dayGroups = useMemo(() => {
    if (!ordered) return null;
    const groups: { day: number; ts: number; items: Message[] }[] = [];
    for (const m of ordered) {
      const day = startOfDay(m.ts);
      const last = groups.at(-1);
      if (last && last.day === day) last.items.push(m);
      else groups.push({ day, ts: m.ts, items: [m] });
    }
    return groups;
  }, [ordered]);
  // The id of the very last line overall — the only one that may offer a retry.
  const lastId = ordered?.at(-1)?.id;

  return (
    <Stack h="100%" gap={0}>
      <Box flex={1} mih={0} pos="relative">
        <ScrollArea h="100%" viewportRef={viewport} p="md" onScrollPositionChange={handleScroll}>
          <MessageList
            dayGroups={dayGroups}
            firstUnreadId={firstUnreadId}
            personas={personas}
            selfId={selfId}
            fontSize={fontSize}
            aiMembers={aiMembers}
            replyMap={replyMap}
            lastId={lastId}
            busy={busy}
            onRetry={handleRetry}
            locale={i18n.language}
            hasEarlier={hasEarlier}
            hasLater={hasLater}
          />
        </ScrollArea>
        {jumping && (
          <Center pos="absolute" inset={0} style={{ pointerEvents: 'none' }}>
            <Loader />
          </Center>
        )}
        <JumpToLatest
          atBottom={atBottom}
          atTail={atTail}
          unreadCount={unreadCount}
          onClick={returnToLatest}
        />
      </Box>
      {currentTurn && currentTurn.members.length > 0 && (
        <ReadProgressBar turn={currentTurn} personas={personas} onJumpToMessage={handleJump} />
      )}
      <Box p="md" style={{ borderTop: '1px solid var(--mantine-color-default-border)' }}>
        {readOnly ? (
          <Text size="sm" c="dimmed" ta="center">
            {t('readonly.chatNotice')}
          </Text>
        ) : (
          <>
            {!locked && !busy && (regenerating || suggestions.length > 0) && (
              <SuggestionChips
                prompts={suggestions}
                loading={regenerating}
                onPick={(text) => setFill({ text, nonce: Date.now() })}
                onRegenerate={handleRegenerate}
              />
            )}
            <Composer
              onSend={handleSend}
              disabled={locked || busy}
              placeholder={locked ? t('chat.locked') : busy ? t('chat.waiting') : undefined}
              fill={fill}
            />
          </>
        )}
      </Box>
    </Stack>
  );
}

interface MessageListProps {
  /** Consecutive same-day runs, or null while history is still loading. */
  dayGroups: { day: number; ts: number; items: Message[] }[] | null;
  /** Id the "new messages" divider is dropped in front of, or null. */
  firstUnreadId: string | null;
  personas: Map<string, Persona>;
  selfId: string | undefined;
  fontSize: number;
  aiMembers: Persona[];
  replyMap: Map<string, Set<string>>;
  lastId: string | undefined;
  busy: boolean;
  onRetry: () => void;
  locale: string;
  /** More history exists above — a top spinner shows and scrolling near it pages in. */
  hasEarlier: boolean;
  /** More history exists below (a detached window) — a bottom spinner pages in. */
  hasLater: boolean;
}

/**
 * The scrollable conversation body: day-grouped lines with the unread divider.
 * Memoized so unrelated app re-renders (notably toggling the mobile navbar, which
 * re-renders the whole shell) don't re-render every message — its props are all
 * stable references, so it bails unless the conversation itself changes.
 */
const MessageList = memo(function MessageList({
  dayGroups,
  firstUnreadId,
  personas,
  selfId,
  fontSize,
  aiMembers,
  replyMap,
  lastId,
  busy,
  onRetry,
  locale,
  hasEarlier,
  hasLater,
}: MessageListProps) {
  const { t } = useTranslation();
  if (dayGroups === null) {
    return (
      <Center h={200}>
        <Loader />
      </Center>
    );
  }
  if (dayGroups.length === 0) {
    return (
      <Center h={200}>
        <Text c="dimmed">{t('chat.empty')}</Text>
      </Center>
    );
  }
  return (
    <Stack gap="md">
      {hasEarlier && (
        <Center py="xs">
          <Loader size="sm" />
        </Center>
      )}
      {dayGroups.map((day) => (
        <Stack key={day.day} gap="md" pos="relative">
          <DayHeader label={formatDayLabel(day.ts, locale, t)} />
          {day.items.map((message) => (
            <div key={message.id} id={`msg-${message.id}`}>
              {message.id === firstUnreadId && <UnreadDivider label={t('chat.unreadDivider')} />}
              <ChatMessage
                message={message}
                persona={personas.get(message.personaId)}
                selfId={selfId}
                fontSize={fontSize}
                aiMembers={aiMembers}
                repliedBy={replyMap.get(message.id)}
                canRetry={message.id === lastId && !busy}
                onRetry={onRetry}
              />
            </div>
          ))}
        </Stack>
      ))}
      {hasLater && (
        <Center py="xs">
          <Loader size="sm" />
        </Center>
      )}
    </Stack>
  );
});

interface ChatMessageProps {
  message: Message;
  persona: Persona | undefined;
  selfId: string | undefined;
  fontSize: number;
  aiMembers: Persona[];
  repliedBy: Set<string> | undefined;
  /** This is the latest line and the loop is idle — a `system` error may retry. */
  canRetry: boolean;
  onRetry: () => void;
}

/** A single chat line: resolves its persona and derives the display-only props. */
function ChatMessage({
  message,
  persona,
  selfId,
  fontSize,
  aiMembers,
  repliedBy,
  canRetry,
  onRetry,
}: ChatMessageProps) {
  if (!persona) return null;
  return (
    <MessageItem
      message={message}
      persona={persona}
      isSelf={message.personaId === selfId}
      fontSize={fontSize}
      aiMembers={aiMembers}
      repliedBy={repliedBy ? [...repliedBy] : undefined}
      onRetry={message.kind === 'system' ? onRetry : undefined}
      canRetry={canRetry}
    />
  );
}

/**
 * The date label for a day's block. It sticks to the top of the scroll viewport
 * so the day you're reading stays visible the whole time, and is pushed off only
 * when its block scrolls away and the next day's header takes over. `pointer-events`
 * is off so the floating pill never swallows clicks on the message beneath it.
 */
function DayHeader({ label }: { label: string }) {
  return (
    <Center pos="sticky" top={4} style={{ zIndex: 2, pointerEvents: 'none' }}>
      <Box
        className="agora-day-pill"
        style={{
          fontSize: 12,
          fontWeight: 600,
          color: 'var(--mantine-color-dimmed)',
          border: '1px solid var(--mantine-color-default-border)',
          borderRadius: 999,
          padding: '2px 12px',
          boxShadow: '0 2px 8px rgba(0, 0, 0, 0.08)',
        }}
      >
        {label}
      </Box>
    </Center>
  );
}

/**
 * The "new messages" marker: a full-width rule with a centered label, dropped in
 * front of the first line the user hasn't seen. Unlike the day header it does not
 * stick — it stays anchored to its place in the log, so scrolling past it reads
 * as crossing into what arrived while you were away.
 */
function UnreadDivider({ label }: { label: string }) {
  return (
    <Box
      id="chat-unread-divider"
      my={8}
      c="red"
      style={{ display: 'flex', alignItems: 'center', gap: 12 }}
    >
      <Box style={{ flex: 1, height: 1, background: 'var(--mantine-color-red-4)' }} />
      <Text size="xs" fw={700} tt="uppercase" style={{ letterSpacing: 0.4 }}>
        {label}
      </Text>
      <Box style={{ flex: 1, height: 1, background: 'var(--mantine-color-red-4)' }} />
    </Box>
  );
}

/**
 * The catch-up affordance: a pill floating at the bottom of the log, shown while
 * the reader is scrolled up (`!atBottom`) or the window is detached in old history
 * (`!atTail`). At the tail it shows the unseen-line count; detached it just offers
 * a way back. Clicking drops to the newest line (reloading the tail if detached)
 * and marks everything read. Owns its own visibility so the parent stays lean.
 */
function JumpToLatest({
  atBottom,
  atTail,
  unreadCount,
  onClick,
}: {
  atBottom: boolean;
  atTail: boolean;
  unreadCount: number;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  if (atBottom && atTail) return null;
  const label =
    atTail && unreadCount > 0
      ? t('chat.newMessages', { count: unreadCount })
      : t('chat.jumpToLatest');
  return (
    <UnstyledButton
      onClick={onClick}
      style={{
        position: 'absolute',
        bottom: 16,
        left: '50%',
        transform: 'translateX(-50%)',
        zIndex: 3,
        display: 'flex',
        alignItems: 'center',
        gap: 6,
        padding: '6px 14px',
        borderRadius: 999,
        color: 'white',
        fontSize: 13,
        fontWeight: 600,
        background:
          'linear-gradient(135deg, var(--mantine-color-indigo-6), var(--mantine-color-cyan-5))',
        boxShadow: '0 6px 18px rgba(0, 0, 0, 0.22)',
      }}
    >
      {label}
      <IconArrowDown size={15} />
    </UnstyledButton>
  );
}
