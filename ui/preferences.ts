import type { ProtectionMode } from './attributes.ts';

export interface Preferences {
  notification: 'off' | 'failures' | 'all';
  historyRetentionDays: 0 | 7 | 30 | 90;
  historyPaths: boolean;
  defaultProtectionMode: ProtectionMode;
  defaultUnlockMinutes: 1 | 5 | 15 | 30 | 60;
  defaultOutputDirectory: string;
  density: 'standard' | 'compact';
  fontSize: 'standard' | 'large';
}
export type HistoryPreferences = Pick<Preferences, 'historyRetentionDays' | 'historyPaths'>;
export const PREFERENCES_KEY = 'foldsentinel.preferences.v1';
export const DEFAULT_PREFERENCES: Readonly<Preferences> = Object.freeze({
  notification: 'off', historyRetentionDays: 0, historyPaths: true,
  defaultProtectionMode: 'hidden-locked', defaultUnlockMinutes: 5,
  defaultOutputDirectory: '', density: 'standard', fontSize: 'standard',
});

// 只接受当前设置字段；旧版本缺少新增字段时使用默认值，错误值明确报告。
export function loadPreferences(raw: string | null): Preferences {
  if (raw === null) return { ...DEFAULT_PREFERENCES };
  const value: unknown = JSON.parse(raw);
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('preferences_invalid');
  const record = value as Record<string, unknown>;
  const read = <T>(key: keyof Preferences, valid: (value: unknown) => boolean): T => {
    if (!(key in record)) return DEFAULT_PREFERENCES[key] as T;
    if (!valid(record[key])) throw new Error('preferences_invalid');
    return record[key] as T;
  };
  return {
    notification: read('notification', value => ['off', 'failures', 'all'].includes(value as string)),
    historyRetentionDays: read('historyRetentionDays', value => [0, 7, 30, 90].includes(value as number)),
    historyPaths: read('historyPaths', value => typeof value === 'boolean'),
    defaultProtectionMode: read('defaultProtectionMode', value => ['none', 'hidden', 'locked', 'readonly', 'readonly-acl', 'hidden-locked'].includes(value as string)),
    defaultUnlockMinutes: read('defaultUnlockMinutes', value => [1, 5, 15, 30, 60].includes(value as number)),
    defaultOutputDirectory: read('defaultOutputDirectory', value => typeof value === 'string' && value.length <= 32767 && !/\p{Cc}/u.test(value)),
    density: read('density', value => ['standard', 'compact'].includes(value as string)),
    fontSize: read('fontSize', value => ['standard', 'large'].includes(value as string)),
  };
}

export function serializePreferences(preferences: Preferences): string {
  // 用相同校验器读取，避免直接对象展开将未来的密码、路径草稿等写入设置。
  return JSON.stringify(loadPreferences(JSON.stringify(preferences)));
}
