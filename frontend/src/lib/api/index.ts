import { HttpChatApi } from './http';
import { MockChatApi } from './mock';
import type { ChatApi } from './types';

const baseUrl = import.meta.env.VITE_API_BASE_URL?.trim();
const forceMock = import.meta.env.VITE_USE_MOCK === '1';

/** True when the app is running against the in-memory mock. */
export const usingMock = forceMock || !baseUrl;

/** The single ChatApi instance the whole UI depends on. */
export const api: ChatApi = !usingMock && baseUrl ? new HttpChatApi(baseUrl) : new MockChatApi();

export type { ChatApi } from './types';
