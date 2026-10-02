import { useSyncExternalStore } from 'react';

export type AppLanguage = 'en' | 'vi';
export type TranslateParams = Record<string, string | number>;

const PREFERENCES_KEY = 'chatcmd.preferences';
const listeners = new Set<() => void>();

const vi: Record<string, string> = {
  'Allow access in all conversations': 'Cho phép truy cập trong mọi cuộc trò chuyện',
  'When enabled and saved, this registered folder is accessible in all conversations. Existing tool permissions and approval requirements still apply.': 'Sau khi bật tùy chọn này và lưu, thư mục đã đăng ký này có thể được truy cập trong mọi cuộc trò chuyện. Quyền sử dụng công cụ và các yêu cầu phê duyệt hiện có vẫn được áp dụng.',
  "Uncheck and save, or delete this project, to revoke this project's shared access.": 'Bỏ chọn rồi lưu, hoặc xóa dự án này, để thu hồi quyền truy cập dùng chung do dự án này cấp.',
};

function interpolate(value: string, params?: TranslateParams) {
  if (!params) return value;
  return value.replace(/\{([A-Za-z0-9_]+)\}/g, (match, key: string) => params[key] === undefined ? match : String(params[key]));
}

export function resolveAppLanguage(browserLanguage?: string, stored?: unknown): AppLanguage {
  if (stored === 'vi' || stored === 'en') return stored;
  const value = (browserLanguage || '').trim().toLowerCase();
  if (value === 'vi' || value.startsWith('vi-')) return 'vi';
  if (value === 'en' || value.startsWith('en-')) return 'en';
  return 'en';
}

function detectBrowserLanguage(): AppLanguage {
  return resolveAppLanguage(typeof navigator === 'undefined' ? undefined : navigator.language);
}

function storedLanguage(): AppLanguage | undefined {
  if (typeof localStorage === 'undefined') return undefined;
  try {
    const preferences = JSON.parse(localStorage.getItem(PREFERENCES_KEY) ?? '{}') as { language?: unknown };
    return preferences.language === 'vi' || preferences.language === 'en' ? preferences.language : undefined;
  } catch { return undefined; }
}

let language: AppLanguage = storedLanguage() ?? detectBrowserLanguage();

export function getAppLanguage() { return language; }
export function appLocale() { return language === 'vi' ? 'vi-VN' : 'en-US'; }
export function hasStoredLanguagePreference() { return storedLanguage() !== undefined; }

export function tr(source: string, params?: TranslateParams) {
  const translated = language === 'vi' ? vi[source] ?? source : source;
  return interpolate(translated, params);
}

export function formatAppNumber(value: number) { return new Intl.NumberFormat(appLocale()).format(value); }

export function setAppLanguage(next: AppLanguage, persist = true) {
  if (next !== 'en' && next !== 'vi') next = 'en';
  const changed = language !== next;
  language = next;
  if (typeof document !== 'undefined') document.documentElement.lang = next;
  if (persist && typeof localStorage !== 'undefined') {
    try {
      const current = JSON.parse(localStorage.getItem(PREFERENCES_KEY) ?? '{}') as Record<string, unknown>;
      localStorage.setItem(PREFERENCES_KEY, JSON.stringify({ ...current, language: next }));
    } catch { /* storage can be unavailable */ }
  }
  if (changed) listeners.forEach((listener) => listener());
}

export function useAppLanguage() {
  return useSyncExternalStore(
    (listener) => { listeners.add(listener); return () => listeners.delete(listener); },
    () => language,
    () => 'en' as AppLanguage,
  );
}

export function translatedStatus(status: string) { return tr(status.toLowerCase()); }

if (typeof document !== 'undefined') document.documentElement.lang = language;
