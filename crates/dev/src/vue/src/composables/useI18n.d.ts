import type { ComputedRef } from "vue";

type TranslationParameters = Record<string, string | number>;

export declare function useI18n(): {
  t: (key: string, parameters?: TranslationParameters) => string;
  setLocale: (locale: string) => void;
  locale: ComputedRef<string>;
};
