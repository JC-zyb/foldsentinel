export type Operation = 'encrypt' | 'decrypt' | 'verify';
export type TaskStatus = 'waiting' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted';
export interface Plan { input: string; output: string | null; operation: Operation }
export interface Task extends Plan { id: string; status: TaskStatus; createdAt: number; endedAt?: number; code?: string; batchId?: string }
export interface Progress { phase: string; completed: number; total: number; current: string }
export interface VaultReport { input_bytes: number; output_bytes: number; directory: boolean; preview?: import('./preview').PackagePreview }
export const HISTORY_KEY = 'foldsentinel.history.v1';
export const MAX_SOURCES = 200;
const statuses: TaskStatus[] = ['waiting', 'running', 'succeeded', 'failed', 'cancelled', 'interrupted'];
const operations: Operation[] = ['encrypt', 'decrypt', 'verify'];

export function sourceKey(path: string): string {
  return path.trim().replaceAll('/', '\\').replace(/^\\\\\?\\UNC\\/i, '\\\\')
    .replace(/^\\\\\?\\/, '').replace(/\\+$/, '').toLowerCase();
}

export function mergeSourcePaths(existing: readonly string[], incoming: readonly string[]): string[] {
  const seen = new Set<string>();
  return [...existing, ...incoming].map(path => path.trim()).filter(path => {
    if (!path) return false;
    const key = sourceKey(path);
    if (seen.has(key)) return false;
    seen.add(key); return true;
  });
}

export function inspectSources(raw: string): { paths: string[]; duplicates: number; overlaps: [string, string][] } {
  const all = raw.split(/\r?\n/).map(path => path.trim()).filter(Boolean);
  const paths = mergeSourcePaths([], all);
  const overlaps: [string, string][] = [];
  // 超限由界面拒绝；不对任意长的粘贴内容进行二次方比较。
  if (paths.length > MAX_SOURCES) return { paths, duplicates: all.length - paths.length, overlaps };
  for (let i = 0; i < paths.length; i++) for (let j = i + 1; j < paths.length; j++) {
    const left = sourceKey(paths[i]); const right = sourceKey(paths[j]);
    if (left.startsWith(right + '\\') || right.startsWith(left + '\\')) overlaps.push([paths[i], paths[j]]);
  }
  return { paths, duplicates: all.length - paths.length, overlaps };
}

export function batchSummary(tasks: readonly Task[]): Record<TaskStatus, number> {
  const summary = { waiting: 0, running: 0, succeeded: 0, failed: 0, cancelled: 0, interrupted: 0 };
  tasks.forEach(task => { summary[task.status]++; });
  return summary;
}

export function loadHistory(raw: string | null): Task[] {
  if (!raw) return [];
  const parsed: unknown = JSON.parse(raw);
  if (!Array.isArray(parsed) || parsed.length > 200) throw new Error('history_invalid');
  const ids = new Set<string>();
  return parsed.map(value => {
    if (!value || typeof value !== 'object') throw new Error('history_invalid');
    const task = value as Task;
    if (typeof task.id !== 'string' || ids.has(task.id) || typeof task.input !== 'string' ||
      !(task.output === null || typeof task.output === 'string') || !operations.includes(task.operation) ||
      !statuses.includes(task.status) || !Number.isFinite(task.createdAt) ||
      (task.endedAt !== undefined && !Number.isFinite(task.endedAt)) ||
      (task.code !== undefined && typeof task.code !== 'string') ||
      (task.batchId !== undefined && (typeof task.batchId !== 'string' || task.batchId.length > 100))) throw new Error('history_invalid');
    ids.add(task.id);
    return { ...descriptor(task), status: ['waiting', 'running'].includes(task.status) ? 'interrupted' : task.status };
  });
}

// 白名单序列化；密码从不属于 Task，也不会通过对象展开进入历史。
function descriptor(task: Task): Task {
  return { id: task.id, input: task.input, output: task.output, operation: task.operation,
    status: task.status, createdAt: task.createdAt, endedAt: task.endedAt, code: task.code, batchId: task.batchId };
}
export function serializeHistory(tasks: Task[]): string { return JSON.stringify(tasks.map(descriptor)); }

export class TaskQueue {
  tasks: Task[];
  running = false;
  private paused = false;
  private secrets = new Map<string, string>();
  constructor(tasks: Task[], private save: (tasks: Task[]) => void, private changed: () => void) { this.tasks = tasks; }
  get pauseRequested(): boolean { return this.paused; }
  add(plans: Plan[], password: string): void {
    if (!password || !plans.length || this.tasks.filter(t => ['waiting', 'running'].includes(t.status)).length + plans.length > 100) throw new Error('queue_limit');
    const batchId = crypto.randomUUID();
    const createdAt = Date.now();
    const added: Task[] = plans.map(plan => ({ ...plan, id: crypto.randomUUID(), batchId, status: 'waiting', createdAt }));
    const pending = this.tasks.filter(t => ['waiting', 'running'].includes(t.status));
    const history = this.tasks.filter(t => !['waiting', 'running'].includes(t.status)).slice(-(200 - pending.length - added.length));
    const next = [...history, ...pending, ...added];
    this.save(next);
    this.tasks = next;
    added.forEach(t => this.secrets.set(t.id, password));
    this.changed();
  }
  cancelWaiting(id: string): void {
    const task = this.tasks.find(t => t.id === id && t.status === 'waiting');
    if (!task) return;
    const endedAt = Date.now();
    const next = this.tasks.map(item => item === task ? { ...item, status: 'cancelled' as const, endedAt } : item);
    this.save(next);
    task.status = 'cancelled'; task.endedAt = endedAt; this.secrets.delete(id);
    this.changed();
  }
  pause(): void { this.paused = true; this.changed(); }
  clearHistory(): void {
    const next = this.tasks.filter(t => ['waiting', 'running'].includes(t.status));
    this.save(next); this.tasks = next; this.changed();
  }
  async start(perform: (task: Task, password: string) => Promise<unknown>, errorCode: (error: unknown) => string): Promise<void> {
    if (this.running) return;
    this.running = true; this.paused = false; this.changed();
    try {
      while (!this.paused) {
        const task = this.tasks.find(t => t.status === 'waiting');
        if (!task) break;
        const password = this.secrets.get(task.id);
        if (!password) { task.status = 'interrupted'; this.save(this.tasks); this.changed(); continue; }
        task.status = 'running';
        // 先持久化运行状态，失败时不会启动无记录的文件操作。
        try { this.save(this.tasks); } catch (error) { task.status = 'waiting'; throw error; }
        this.changed();
        try { await perform(task, password); task.status = 'succeeded'; }
        catch (error) { task.code = errorCode(error); task.status = task.code === 'cancelled' ? 'cancelled' : 'failed'; }
        finally { this.secrets.delete(task.id); task.endedAt = Date.now(); }
        this.save(this.tasks); this.changed();
      }
    } finally { this.running = false; this.changed(); }
  }
}
