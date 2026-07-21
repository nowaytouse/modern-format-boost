/**
 * i18n国际化系统
 */

import { ref, computed } from "vue";
import zh from "../locales/zh.json";
import en from "../locales/en.json";
import ja from "../locales/ja.json";

const messages = {
  zh: zh,
  zh_CN: zh,
  en: en,
  ja: ja,
};

// 从localStorage读取保存的语言设置，默认中文 zh
const savedLocale =
  typeof localStorage !== "undefined"
    ? localStorage.getItem("pixly_locale") || "zh"
    : "zh";

const currentLocale = ref(savedLocale);

export function useI18n() {
  const t = (key, params = {}) => {
    const keys = key.split(".");
    let value = messages[currentLocale.value];

    for (const k of keys) {
      if (value && typeof value === "object") {
        value = value[k];
      } else {
        return key;
      }
    }

    if (typeof value === "string") {
      // 替换参数 {count} -> 实际值
      return value.replace(/\{(\w+)\}/g, (match, param) => {
        return params[param] !== undefined ? params[param] : match;
      });
    }

    return key;
  };

  const setLocale = (locale) => {
    if (messages[locale]) {
      currentLocale.value = locale;
      // 持久化到localStorage
      if (typeof localStorage !== "undefined") {
        localStorage.setItem("pixly_locale", locale);
      }
    }
  };

  const locale = computed(() => currentLocale.value);

  return {
    t,
    setLocale,
    locale,
  };
}
