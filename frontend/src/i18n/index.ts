import i18n from 'i18next';
import LanguageDetector from 'i18next-browser-languagedetector';
import { initReactI18next } from 'react-i18next';
import type { UiLanguage } from '../types';
import { en } from './locales/en';
import { zhHant } from './locales/zh-Hant';

export const UI_LANGUAGES: { value: UiLanguage; label: string }[] = [
  { value: 'zh-Hant', label: '繁體中文' },
  { value: 'en', label: 'English' },
];

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: en },
      'zh-Hant': { translation: zhHant },
    },
    fallbackLng: 'en',
    supportedLngs: ['en', 'zh-Hant'],
    interpolation: {
      // React already escapes, and our literal {{token}} values are trusted.
      escapeValue: false,
    },
    detection: {
      order: ['localStorage', 'navigator'],
      caches: [],
    },
    react: {
      // The workspace store drives the language; never suspend on load.
      useSuspense: false,
    },
  });

export default i18n;
