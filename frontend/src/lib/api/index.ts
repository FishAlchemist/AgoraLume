import { useConnection } from '../../store/connection';
import type { GroupSuggestions, Message } from '../../types';
import { HttpChatApi } from './http';
import { MockChatApi } from './mock';
import type {
  ActivityHandler,
  AgentTrace,
  ChatApi,
  DebugHandler,
  DebugUsage,
  MessageHandler,
  ReadHandler,
  ServerMeta,
  SuggestionsHandler,
} from './types';

/**
 * A stable `ChatApi` that routes every call to the currently configured data
 * source — the in-browser mock, or an HTTP backend — read live from the
 * connection store. Because the object identity never changes, components can
 * `import { api }` once; switching backends (or connecting to one that comes up
 * later) takes effect on the next call, with no reload.
 */
class RoutingChatApi implements ChatApi {
  private readonly mock = new MockChatApi();
  // One client per URL, reused so repeated switches don't leak connections.
  private readonly httpByUrl = new Map<string, HttpChatApi>();

  private impl(): ChatApi {
    const url = useConnection.getState().backendUrl;
    if (!url) return this.mock;
    let http = this.httpByUrl.get(url);
    if (!http) {
      http = new HttpChatApi(url);
      this.httpByUrl.set(url, http);
    }
    return http;
  }

  probe(): Promise<ServerMeta | null> {
    return this.impl().probe();
  }
  listMessages(groupId: string): Promise<Message[]> {
    return this.impl().listMessages(groupId);
  }
  sendMessage(groupId: string, text: string, personaId?: string): Promise<Message> {
    return this.impl().sendMessage(groupId, text, personaId);
  }
  retry(groupId: string): Promise<void> {
    return this.impl().retry(groupId);
  }
  subscribe(groupId: string, handler: MessageHandler): () => void {
    return this.impl().subscribe(groupId, handler);
  }
  subscribeReads(groupId: string, handler: ReadHandler): () => void {
    return this.impl().subscribeReads(groupId, handler);
  }
  subscribeActivity(groupId: string, handler: ActivityHandler): () => void {
    return this.impl().subscribeActivity(groupId, handler);
  }
  getUsage(): Promise<DebugUsage> {
    return this.impl().getUsage();
  }
  listTraces(groupId: string): Promise<AgentTrace[]> {
    return this.impl().listTraces(groupId);
  }
  subscribeDebug(groupId: string, handler: DebugHandler): () => void {
    return this.impl().subscribeDebug(groupId, handler);
  }
  getSuggestions(groupId: string): Promise<GroupSuggestions> {
    return this.impl().getSuggestions(groupId);
  }
  regenerateSuggestions(groupId: string): Promise<void> {
    return this.impl().regenerateSuggestions(groupId);
  }
  subscribeSuggestions(groupId: string, handler: SuggestionsHandler): () => void {
    return this.impl().subscribeSuggestions(groupId, handler);
  }
}

/** The single ChatApi instance the whole UI depends on. */
export const api: ChatApi = new RoutingChatApi();

export type { ChatApi } from './types';
