import { authFetch } from './authFetch';

/** How long to wait before reconnecting after a dropped or failed connection. */
const RECONNECT_DELAY_MS = 1000;

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * A hand-rolled Server-Sent Events client built on `fetch`, not `EventSource`.
 *
 * `EventSource` has no API to set request headers, so it can't carry an
 * `Authorization: Bearer <token>` — the only alternative would be putting the
 * token in the URL's query string, which some forward proxies log, leaking it
 * outside the app. `fetch` (via {@link authFetch}) can set the header like any
 * other request, so this reads the stream's response body directly and parses
 * the `text/event-stream` framing by hand: `event:`/`data:` lines, blank-line
 * frame boundaries, `:`-prefixed comments (including the backend's keep-alive
 * pings) ignored.
 *
 * Reconnects on any drop (network blip, server restart, a proxy's idle
 * timeout) after a fixed delay, the same as `EventSource`'s built-in retry —
 * `onOpen` fires on the first connect and every reconnect after it, so a
 * caller can tell them apart (see `SharedStreams.open`'s resync-on-reconnect).
 */
export class FetchEventStream {
  private closed = false;
  private controller: AbortController | null = null;

  constructor(
    private readonly url: string,
    private readonly onEvent: (eventName: string, data: string) => void,
    private readonly onOpen: () => void,
  ) {
    void this.run();
  }

  close(): void {
    this.closed = true;
    this.controller?.abort();
  }

  private async run(): Promise<void> {
    while (!this.closed) {
      this.controller = new AbortController();
      try {
        const res = await authFetch(this.url, {
          headers: { Accept: 'text/event-stream' },
          signal: this.controller.signal,
        });
        if (res.status === 401 || res.status === 403) {
          // authFetch already tried a refresh-and-retry internally, so this
          // token isn't coming back without a fresh login — reconnecting
          // would just hammer the backend once a second forever (no session
          // to fix it, unlike a network blip or a server restart). The auth
          // gate takes over once useAuth clears the session.
          this.closed = true;
          return;
        }
        if (!res.ok || !res.body) throw new Error(`stream failed: ${res.status}`);
        this.onOpen();
        await this.readBody(res.body);
      } catch {
        // Network error, abort, or a non-OK status — fall through to the
        // reconnect delay below; an intentional close() exits the loop first.
      }
      if (this.closed) return;
      await delay(RECONNECT_DELAY_MS);
    }
  }

  private async readBody(body: ReadableStream<Uint8Array>): Promise<void> {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    while (true) {
      const { done, value } = await reader.read();
      if (done) return;
      buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, '\n');
      let boundary = buffer.indexOf('\n\n');
      while (boundary !== -1) {
        this.dispatch(buffer.slice(0, boundary));
        buffer = buffer.slice(boundary + 2);
        boundary = buffer.indexOf('\n\n');
      }
    }
  }

  /** Parses one blank-line-delimited SSE frame and fires `onEvent` if it carries data. */
  private dispatch(frame: string): void {
    let eventName = 'message';
    const dataLines: string[] = [];
    for (const line of frame.split('\n')) {
      if (line.startsWith(':')) continue;
      if (line.startsWith('event:')) eventName = line.slice(6).trim();
      else if (line.startsWith('data:')) dataLines.push(line.slice(5).trimStart());
    }
    if (dataLines.length === 0) return;
    this.onEvent(eventName, dataLines.join('\n'));
  }
}
