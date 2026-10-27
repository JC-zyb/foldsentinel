import type { ProtectionMode } from './attributes.ts';

export type ManagedMode = Extract<ProtectionMode, 'locked' | 'hidden-locked' | 'readonly-acl'>;
export interface ServiceRoot {
  id: string; name: string; note: string; mode: ManagedMode; password_required: boolean;
  monitoring: boolean; unlocked_until: number | null; restoring: boolean; last_error: string | null; entries: number;
}
export interface ServiceTransport { host: 'service'; healthy: boolean; last_tick_error: string | null }
export interface ServiceStatus { registered: number; roots: ServiceRoot[] }
export type ManagedCommand =
  | { action: 'status' | 'list' }
  | { action: 'enroll'; path: string; mode: ManagedMode; note: string; password: string | null }
  | { action: 'unlock'; id: string; password: string; duration_seconds: number }
  | { action: 'relock'; id: string }
  | { action: 'remove'; id: string; password: string | null }
  | { action: 'set_password'; id: string; password: string; new_password: string | null }
  | { action: 'set_continuous'; id: string; password: string | null; enabled: boolean };

function failure(code = 'internal'): never { throw { code }; }
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const errorCode = (value: unknown): value is string | null => value === null || (typeof value === 'string' && /^[a-z_]{1,64}$/.test(value));

export function managedResponse(response: unknown): { transport: ServiceTransport; data: unknown } {
  if (!object(response) || response.version !== 1 || !object(response.transport)) failure();
  const transport = response.transport;
  if (transport.host !== 'service' || typeof transport.healthy !== 'boolean' || !errorCode(transport.last_tick_error)) failure('service_unavailable');
  // IPC成功只代表收到响应，服务业务错误必须单独拒绝。
  if (!errorCode(response.error)) failure();
  if (response.error) failure(response.error);
  if (response.data === null || response.data === undefined) failure();
  return { transport: { host: 'service', healthy: transport.healthy, last_tick_error: transport.last_tick_error }, data: response.data };
}

export function managedStatus(data: unknown): ServiceStatus {
  if (!object(data) || !Number.isSafeInteger(data.registered) || (data.registered as number) < 0 || !Array.isArray(data.roots) || data.registered !== data.roots.length) failure();
  const ids = new Set<string>();
  const roots = data.roots.map((value: unknown): ServiceRoot => {
    if (!object(value) || typeof value.id !== 'string' || !value.id || value.id.length > 128 || ids.has(value.id)
      || typeof value.name !== 'string' || typeof value.note !== 'string'
      || !['locked', 'hidden-locked', 'readonly-acl'].includes(value.mode as string)
      || typeof value.password_required !== 'boolean' || typeof value.monitoring !== 'boolean' || typeof value.restoring !== 'boolean'
      || !errorCode(value.last_error) || !Number.isSafeInteger(value.entries) || (value.entries as number) < 0
      || !(value.unlocked_until === null || (Number.isSafeInteger(value.unlocked_until) && (value.unlocked_until as number) >= 0))) failure();
    ids.add(value.id);
    // 只复制展示白名单，未知后台字段不会进入界面或前端状态。
    return { id: value.id, name: value.name, note: value.note, mode: value.mode as ManagedMode,
      password_required: value.password_required, monitoring: value.monitoring, unlocked_until: value.unlocked_until as number | null,
      restoring: value.restoring, last_error: value.last_error, entries: value.entries as number };
  });
  return { registered: data.registered as number, roots };
}

export function validManagedPassword(password: string): boolean { return [...password].length >= 8 && new TextEncoder().encode(password).length <= 1024; }

export function clearManagedCommand(command: ManagedCommand): void {
  if ('password' in command) command.password = '';
  if ('new_password' in command) command.new_password = null;
}

// 一次调用结束后释放请求对象中的密码引用；不把请求存入草稿或诊断。
export async function requestManaged(command: ManagedCommand, request: (command: ManagedCommand) => Promise<unknown>): Promise<{ transport: ServiceTransport; data: unknown }> {
  try { return managedResponse(await request(command)); }
  finally { clearManagedCommand(command); }
}
