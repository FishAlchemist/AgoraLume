import { isGuestFallback } from '../../store/backendStatus';
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
  HistoryWindow,
  MessageHandler,
  PersonaUsage,
  ReadHandler,
  ServerMeta,
  SuggestionsHandler,
  TurnHandler,
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

  private httpFor(url: string): HttpChatApi {
    let http = this.httpByUrl.get(url);
    if (!http) {
      http = new HttpChatApi(url);
      this.httpByUrl.set(url, http);
    }
    return http;
  }

  /**
   * Every call except `probe` — the real backend once there's a usable
   * session, the in-browser demo otherwise (see `isGuestFallback`), so a
   * guest on a login-required deployment gets a working demo instead of a
   * wall of 401s.
   */
  private impl(): ChatApi {
    const url = useConnection.getState().backendUrl;
    if (!url || isGuestFallback()) return this.mock;
    return this.httpFor(url);
  }

  /**
   * `/meta` always goes straight to the real backend when one is configured,
   * bypassing the guest-fallback gate above — it's how the app learns
   * `authRequired` in the first place, and it's unauthenticated by backend
   * design regardless of session state.
   */
  probe(): Promise<ServerMeta | null> {
    const url = useConnection.getState().backendUrl;
    return url ? this.httpFor(url).probe() : this.mock.probe();
  }
  listMessages(groupId: string, opts?: HistoryWindow): Promise<Message[]> {
    return this.impl().listMessages(groupId, opts);
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
  subscribeTurn(groupId: string, handler: TurnHandler): () => void {
    return this.impl().subscribeTurn(groupId, handler);
  }
  getUsage(): Promise<DebugUsage> {
    return this.impl().getUsage();
  }
  getGroupUsage(groupId: string): Promise<DebugUsage> {
    return this.impl().getGroupUsage(groupId);
  }
  getPersonaUsage(groupId: string): Promise<PersonaUsage[]> {
    return this.impl().getPersonaUsage(groupId);
  }
  getGlobalPersonaUsage(): Promise<PersonaUsage[]> {
    return this.impl().getGlobalPersonaUsage();
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
