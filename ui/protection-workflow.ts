// 登记快照与刷新状态分开，读取失败不能抹掉恢复入口。
export class ProtectionRecords<T> {
  snapshot: T | null = null;
  stale = false;
  loading = false;
  error: unknown = null;

  async refresh(load: () => Promise<T>): Promise<void> {
    if (this.loading) throw new Error('busy');
    this.loading = true;
    try {
      const snapshot = await load();
      this.snapshot = snapshot;
      this.stale = false;
      this.error = null;
    } catch (error) {
      this.stale = true;
      this.error = error;
      throw error;
    } finally {
      this.loading = false;
    }
  }
}

export interface ProtectionActionResult {
  operationSucceeded: boolean;
  refreshSucceeded: boolean;
  operationError?: unknown;
  refreshError?: unknown;
}

// 刷新是独立结果，不能在 finally 中用它的异常覆盖原操作异常。
export async function runProtectionAction(action: () => Promise<void>, refresh: () => Promise<void>): Promise<ProtectionActionResult> {
  const result: ProtectionActionResult = { operationSucceeded: false, refreshSucceeded: false };
  try { await action(); result.operationSucceeded = true; }
  catch (error) { result.operationError = error; }
  try { await refresh(); result.refreshSucceeded = true; }
  catch (error) { result.refreshError = error; }
  return result;
}

export interface ProtectionBatchResult<T> { item: T; succeeded: boolean; error?: unknown }

// 每项保留真实结果；某项失败不阻断其余项目，重试范围由失败项决定。
export async function runProtectionBatch<T>(items: readonly T[], action: (item: T) => Promise<void>, completed?: (result: ProtectionBatchResult<T>) => void): Promise<ProtectionBatchResult<T>[]> {
  const results: ProtectionBatchResult<T>[] = [];
  for (const item of items) {
    let result: ProtectionBatchResult<T>;
    try { await action(item); result = { item, succeeded: true }; }
    catch (error) { result = { item, succeeded: false, error }; }
    results.push(result); completed?.(result);
  }
  return results;
}

export function validProtectionNote(note: string): boolean {
  return [...note.trim()].length <= 80 && !/\p{Cc}/u.test(note.trim());
}
