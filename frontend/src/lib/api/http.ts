import type { Message } from '../../types';
import type {
  ActivityHandler,
  ChatApi,
  MessageHandler,
  ReadHandler,
  ReadReceipt,
  ServerMeta,
} from './types';

/**
 * Talks to an AgoraLume backend over HTTP. The base URL is injected via
 * VITE_API_BASE_URL, so the frontend stays fully decoupled: point it at any
 * compatible backend, or leave it unset to fall back to the in-memory mock.
 *
 * The endpoints below follow the backend's OpenAPI contract (`openapi.yml` at
 * the repo root); the generated request/response types live in `./schema.d.ts`
 * (regenerate both with `pnpm gen:api`).
 */
export class HttpChatApi implements ChatApi {
  constructor(private readonly baseUrl: string) {}

  private async getJson<T>(path: string): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`, {
      headers: { Accept: 'application/json' },
    });
    if (!res.ok) throw new Error(`GET ${path} failed: ${res.status}`);
    return (await res.json()) as T;
  }

  async probe(): Promise<ServerMeta | null> {
    try {
      const res = await fetch(`${this.baseUrl}/meta`, { headers: { Accept: 'application/json' } });
      if (!res.ok) return null;
      return (await res.json()) as ServerMeta;
    } catch {
      // Network error / server down.
      return null;
    }
  }

  listMessages(groupId: string): Promise<Message[]> {
    return this.getJson<Message[]>(`/groups/${groupId}/messages`);
  }

  async sendMessage(groupId: string, text: string, personaId?: string): Promise<Message> {
    const res = await fetch(`${this.baseUrl}/groups/${groupId}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text, personaId }),
    });
    if (!res.ok) throw new Error(`sendMessage failed: ${res.status}`);
    return (await res.json()) as Message;
  }

  subscribe(groupId: string, handler: MessageHandler): () => void {
    const source = new EventSource(`${this.baseUrl}/groups/${groupId}/stream`);
    const onMessage = (event: MessageEvent<string>) => {
      try {
        handler(JSON.parse(event.data) as Message);
      } catch {
        // Ignore malformed events.
      }
    };
    source.addEventListener('message', onMessage);
    return () => source.close();
  }

  subscribeReads(groupId: string, handler: ReadHandler): () => void {
    const source = new EventSource(`${this.baseUrl}/groups/${groupId}/stream`);
    const onRead = (event: MessageEvent<string>) => {
      try {
        handler(JSON.parse(event.data) as ReadReceipt);
      } catch {
        // Ignore malformed events.
      }
    };
    // Read receipts arrive as a named "read" SSE event on the same stream.
    source.addEventListener('read', onRead);
    return () => source.close();
  }

  subscribeActivity(groupId: string, handler: ActivityHandler): () => void {
    const source = new EventSource(`${this.baseUrl}/groups/${groupId}/stream`);
    const onActivity = (event: MessageEvent<string>) => {
      try {
        handler((JSON.parse(event.data) as { active: boolean }).active);
      } catch {
        // Ignore malformed events.
      }
    };
    // Turn activity arrives as a named "activity" SSE event on the same stream.
    source.addEventListener('activity', onActivity);
    return () => source.close();
  }
}
