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

export type Message = ConversationMessage | MoodMessage;
