/** The id of the default user identity seeded on first run. */
export const DEFAULT_USER_PERSONA_ID = 'user-me';

/** UI languages the app ships with. */
export type UiLanguage = 'zh-Hant' | 'en';

export type PersonaKind = 'user' | 'ai';

/**
 * A user-created bucket for classifying personas (a "school", "studio",
 * "faction"…). Beyond grouping, an organization can carry shared template
 * variables that every member persona inherits, so prompts can be swapped
 * wholesale by moving a persona between organizations. Organizations may hold
 * departments (a school's classes and clubs) for a second level of grouping.
 */
export interface Organization {
  id: string;
  name: string;
  /** Mantine color name used for accents. */
  color?: string;
  blurb?: string;
  /** Shared template variables inherited by member personas. */
  variables?: Record<string, string>;
}

/**
 * A generic sub-unit inside an organization — a school's class ("2-A") or club,
 * a company's branch or division. The name itself carries the flavour, so there
 * is no fixed "kind". Its variables sit between the parent organization's and
 * the persona's in the inheritance chain.
 */
export interface Department {
  id: string;
  organizationId: string;
  name: string;
  color?: string;
  blurb?: string;
  variables?: Record<string, string>;
}

export interface Persona {
  id: string;
  name: string;
  kind: PersonaKind;
  /** Mantine color name used for accents (bubble tint, mood pill). */
  color: string;
  /** Custom avatar image; takes priority over emoji/initials when set. */
  avatarUrl?: string;
  /** Emoji "face" shown as the avatar when no image is provided. */
  emoji?: string;
  /** CSS gradient used as the avatar background (behind emoji/initials). */
  gradient?: string;
  /** Short description of the modular persona. */
  blurb?: string;
  /** Organization this persona belongs to, for classification and inheritance. */
  organizationId?: string;
  /** Department within the organization; its variables override the org's. */
  departmentId?: string;
  /** The agent system prompt. May contain {{variable}} placeholders. */
  systemPrompt?: string;
  /** Persona-level template variables; override organization variables. */
  variables?: Record<string, string>;
  /**
   * Content hash of the raw `systemPrompt` template — the persona's identity
   * "version". Server-computed and read-only: the backend recomputes it on every
   * write and ignores any value sent by the client. Absent for personas with no
   * prompt (user identities). Full hex SHA-256; show a truncated prefix.
   */
  promptHash?: string;
}

/**
 * A user-assigned, git-tag-style name for a persona identity hash
 * (`Persona.promptHash`). Kept in a side table on the backend so naming a
 * version never mutates the persona itself.
 */
export interface PromptLabel {
  hash: string;
  label: string;
}

/**
 * One persona-scoped memory: a fact a character chose to remember. Stamped with
 * the persona identity hash (`Persona.promptHash`) that was in force when it was
 * written, so a later rewrite of the persona can keep old memories out of
 * character without deleting them. The memory-management UI groups a persona's
 * memories by `promptHash`/label.
 */
export interface Memory {
  id: string;
  personaId: string;
  /** Identity hash in force when written — the scope key that keeps recall in-character. */
  promptHash: string;
  content: string;
  /** Milliseconds since the Unix epoch when the memory was written. */
  createdAt: number;
}

export interface Group {
  id: string;
  name: string;
  /** AI personas that can speak in this group. */
  personaIds: string[];
  /**
   * The user identity that represents "you" in this group. Lets the same user
   * appear under different personas across groups, switchable from the chat.
   */
  selfPersonaId: string;
}

/** User-level preferences persisted on the client. */
export interface Settings {
  /** Language the interface is rendered in. */
  uiLanguage: UiLanguage;
  /**
   * The user's mother tongue, in their own words (e.g. "繁體中文", "English").
   * Injected into agent prompts as {{user_language}} so personas reply in it.
   */
  nativeLanguage: string;
  /** Font size (px) for chat message text. */
  chatFontSize: number;
}

interface BaseMessage {
  id: string;
  groupId: string;
  personaId: string;
  ts: number;
}

/** A normal chat line. */
export interface ConversationMessage extends BaseMessage {
  kind: 'conversation';
  text: string;
  streaming?: boolean;
  /** AI persona ids that have successfully processed (read) this message. */
  readBy?: string[];
}

/** A persona broadcasting its current mood/emotion. */
export interface MoodMessage extends BaseMessage {
  kind: 'mood';
  mood: string;
  note?: string;
}

/**
 * A system notice that an agent's inference failed after exhausting retries.
 * Carries only the HTTP status and its canonical reason (never the provider's
 * raw body). `personaId` is the agent that failed. Rendered as an error line
 * with a retry button.
 */
export interface SystemMessage extends BaseMessage {
  kind: 'system';
  /** HTTP status code, when the failure carried one (e.g. 429). */
  status?: number;
  /** Canonical reason (e.g. "Too Many Requests") or a short generic label. */
  reason: string;
}

export type Message = ConversationMessage | MoodMessage | SystemMessage;

/**
 * How far one AI member has got in the current turn — the buckets the pinned
 * progress bar tints avatars by: still working, done-and-silent, or replied.
 */
export type TurnMemberState = 'pending' | 'read' | 'replied';

/** One AI member's progress within a turn. */
export interface TurnMember {
  personaId: string;
  state: TurnMemberState;
  /**
   * The id of this member's first reply line this turn — the avatar's jump
   * target. Present only once `state` is `replied`.
   */
  replyId?: string;
}

/**
 * What kicked off a turn: a conversation line someone sent (jumpable, with its
 * text to render), or an environment event that carries no message of its own
 * (just a label). The message case is the only one today; the event case is what
 * an upcoming event-trigger feature produces — the bar already handles both.
 */
export type TurnTrigger =
  | { kind: 'message'; messageId: string; personaId: string; text: string }
  | { kind: 'event'; label: string };

/**
 * A processing round: what triggered it and how far each AI member has got.
 * Owned by the backend and streamed independently of message history (a `turn`
 * SSE frame, seeded on connect), so the pinned progress bar reflects the current
 * processing state whether or not the trigger line is in the loaded window — and
 * shows progress for event triggers that have no user message at all. `active`
 * is true while the coordinator is still running the round.
 */
export interface Turn {
  id: string;
  groupId: string;
  trigger: TurnTrigger;
  /** Epoch millis when the turn started. */
  startedAt: number;
  active: boolean;
  /** AI members participating, in the group's member order, each with its progress. */
  members: TurnMember[];
}

/**
 * Conversation-starter suggestions for a group: a few short first-person
 * messages the user could send next when unsure what to say. Generated and
 * cached server-side (the frontend only fetches and displays), and tuned to the
 * time of day so an evening opener isn't shown in the morning. Empty
 * (`generatedAt === 0`) until the first generation for the group completes; a
 * background refresh arrives on the stream's `suggestions` frame.
 */
export interface GroupSuggestions {
  /** Suggested opener lines, in the user's language. Empty until first generated. */
  prompts: string[];
  /** When these were generated (epoch ms); 0 before the first generation. */
  generatedAt: number;
  /** Part of day the openers were tuned for: morning | afternoon | evening | night. */
  timeOfDay: string;
  /** Server bookkeeping (last message id at generation); the UI can ignore it. */
  throughId?: string;
}
