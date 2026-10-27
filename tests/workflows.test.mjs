import test from 'node:test';
import assert from 'node:assert/strict';
import { TaskQueue, MAX_SOURCES, batchSummary, inspectSources, loadHistory, mergeSourcePaths, serializeHistory } from '../ui/tasks.ts';
import { detectLanguage, messages } from '../ui/i18n.ts';
import { previewPage, PREVIEW_PAGE_SIZE } from '../ui/preview.ts';
import { ProtectionRecords, runProtectionAction, runProtectionBatch, validProtectionNote } from '../ui/protection-workflow.ts';
import { managedResponse, managedStatus, requestManaged, validManagedPassword } from '../ui/managed-workflow.ts';

test('separate selections accumulate files and folders without duplicate Windows paths', () => {
  const file = String.raw`D:\work\alpha.txt`;
  const folder = String.raw`D:\work\资料`;
  const second = String.raw`D:\work\beta.bin`;
  const firstDrop = mergeSourcePaths([], [file]);
  const secondDrop = mergeSourcePaths(firstDrop, [folder]);
  assert.deepEqual(secondDrop, [file, folder]);
  assert.deepEqual(mergeSourcePaths(secondDrop, ['d:/WORK/ALPHA.txt', `${folder}\\`, second, '']), [file, folder, second]);
  assert.deepEqual(mergeSourcePaths([`\\\\?\\${file}`], [file]), [`\\\\?\\${file}`]);
  assert.deepEqual(mergeSourcePaths([String.raw`\\server\share\资料`], [String.raw`\\?\UNC\server\share\资料`]), [String.raw`\\server\share\资料`]);
  assert.deepEqual(mergeSourcePaths(secondDrop, []), secondDrop);
  assert.deepEqual(firstDrop, [file]);
});

test('queue is serial, continues after failures, forgets passwords and reloads interrupted work', async () => {
  let saved = '';
  const queue = new TaskQueue([], tasks => { saved = serializeHistory(tasks); }, () => {});
  queue.add([{ input: 'a', output: 'a.fse', operation: 'encrypt' }, { input: 'b', output: 'b.fse', operation: 'encrypt' }], 'never-persist-this-password');
  assert.equal(saved.includes('never-persist'), false);
  assert.equal(loadHistory(saved).every(task => task.status === 'interrupted'), true);
  let active = 0;
  const order = [];
  await queue.start(async (task, password) => {
    assert.equal(password, 'never-persist-this-password');
    assert.equal(++active, 1);
    order.push(task.input);
    await new Promise(resolve => setTimeout(resolve, 5));
    active--;
    if (task.input === 'a') throw { code: 'authentication' };
  }, error => error.code);
  assert.deepEqual(order, ['a', 'b']);
  assert.deepEqual(queue.tasks.map(t => t.status), ['failed', 'succeeded']);
  const restored = new TaskQueue(loadHistory(saved), () => {}, () => {});
  let ran = false;
  await restored.start(async () => { ran = true; }, () => 'internal');
  assert.equal(ran, false);
  assert.equal(saved.includes('password'), false);
});

test('draft identifies duplicates and directory boundaries without changing input order', () => {
  const draft = inspectSources('D:\\Work\\资料\nD:/WORK/资料/a.txt\nD:\\Work\\资料2\n d:/work/资料/ \n');
  assert.deepEqual(draft.paths, [String.raw`D:\Work\资料`, 'D:/WORK/资料/a.txt', String.raw`D:\Work\资料2`]);
  assert.equal(draft.duplicates, 1);
  assert.deepEqual(draft.overlaps, [[String.raw`D:\Work\资料`, 'D:/WORK/资料/a.txt']]);
  assert.equal(inspectSources('D:\\\nD:\\a').overlaps.length, 1);
  assert.equal(inspectSources(String.raw`\\server\share` + '\n' + String.raw`\\?\UNC\server\share\child`).overlaps.length, 1);
  const many = Array.from({ length: MAX_SOURCES + 1 }, (_, i) => `D:/dir/${i}`);
  assert.deepEqual(inspectSources(many.join('\n')).paths, many);
  assert.deepEqual(inspectSources(many.join('\n')).overlaps, []);
});

test('batches summarize outcomes and old v0.3 history remains readable', async () => {
  let saved = '';
  const queue = new TaskQueue([], tasks => { saved = serializeHistory(tasks); }, () => {});
  queue.add(['ok', 'bad', 'cancel'].map(input => ({ input, output: null, operation: 'verify' })), 'secret');
  const firstBatch = queue.tasks[0].batchId;
  assert.ok(firstBatch);
  assert.equal(queue.tasks.every(task => task.batchId === firstBatch), true);
  queue.cancelWaiting(queue.tasks[2].id);
  await queue.start(async task => { if (task.input === 'bad') throw { code: 'authentication' }; }, error => error.code);
  assert.deepEqual(batchSummary(queue.tasks), { waiting: 0, running: 0, succeeded: 1, failed: 1, cancelled: 1, interrupted: 0 });
  queue.add([{ input: 'later', output: null, operation: 'verify' }], 'different-secret');
  assert.notEqual(queue.tasks[3].batchId, firstBatch);
  assert.equal(loadHistory(saved)[0].batchId, firstBatch);
  const legacy = queue.tasks.map(({ batchId, ...task }) => task);
  assert.equal(loadHistory(JSON.stringify(legacy)).length, 4);
  assert.equal(loadHistory(JSON.stringify(legacy))[3].status, 'interrupted');
  assert.equal(serializeHistory(loadHistory(saved)).includes('secret'), false);
  assert.throws(() => loadHistory(JSON.stringify([{ ...legacy[0], batchId: {} }])), /history_invalid/);
});

test('waiting cancellation and pause do not run further tasks', async () => {
  const queue = new TaskQueue([], () => {}, () => {});
  queue.add(['a', 'b', 'c'].map(input => ({ input, output: null, operation: 'verify' })), 'secret');
  queue.cancelWaiting(queue.tasks[2].id);
  await queue.start(async () => { queue.pause(); }, () => 'internal');
  assert.deepEqual(queue.tasks.map(t => t.status), ['succeeded', 'waiting', 'cancelled']);
  await queue.start(async () => { throw { code: 'cancelled' }; }, e => e.code);
  assert.deepEqual(queue.tasks.map(t => t.status), ['succeeded', 'cancelled', 'cancelled']);
  queue.clearHistory();
  assert.equal(queue.tasks.length, 0);
});

test('persistence errors prevent execution and history ignores unexpected secret fields', async () => {
  let fail = false;
  const queue = new TaskQueue([], () => { if (fail) throw new Error('storage_failed'); }, () => {});
  queue.add([{ input: 'a', output: null, operation: 'verify' }], 'secret');
  fail = true;
  let ran = false;
  await assert.rejects(queue.start(async () => { ran = true; }, () => 'internal'), /storage_failed/);
  assert.equal(ran, false);
  assert.equal(queue.tasks[0].status, 'waiting');
  assert.equal(queue.running, false);
  assert.throws(() => queue.cancelWaiting(queue.tasks[0].id), /storage_failed/);
  assert.equal(queue.tasks[0].status, 'waiting');
  const raw = JSON.stringify([{ ...queue.tasks[0], password: 'hidden-secret', key: 'hidden-key' }]);
  assert.equal(serializeHistory(loadHistory(raw)).includes('hidden-'), false);
  assert.throws(() => loadHistory('{bad'));
});

test('language selection and interpolation contracts match in both catalogs', () => {
  assert.equal(detectLanguage(null, ['zh-CN']), 'zh-CN');
  assert.equal(detectLanguage(null, ['en-US', 'zh-CN']), 'en');
  assert.equal(detectLanguage('en', ['zh-CN']), 'en');
  assert.equal(detectLanguage('zh-CN', ['en-US']), 'zh-CN');
  for (const [key, [zh, en]] of Object.entries(messages)) {
    assert.ok(zh && en, key);
    const placeholders = value => [...value.matchAll(/\{([^}]+)\}/g)].map(m => m[1]).sort();
    assert.deepEqual(placeholders(zh), placeholders(en), key);
  }
});

test('preview paging is bounded, clamps indices and handles empty folders', () => {
  const entries = Array.from({ length: PREVIEW_PAGE_SIZE + 3 }, (_, i) => ({ path: `资料/${i}.txt`, directory: false, size: i }));
  const preview = { entries, total_entries: 1003, file_count: 1003, directory_count: 0, content_bytes: 0, format_version: 2, truncated: true };
  assert.equal(previewPage(preview, 0).entries.length, PREVIEW_PAGE_SIZE);
  assert.deepEqual(previewPage(preview, 100), { page: 1, pages: 2, entries: entries.slice(PREVIEW_PAGE_SIZE) });
  assert.equal(previewPage(preview, -1).page, 0);
  assert.equal(previewPage(preview, NaN).page, 0);
  assert.equal(previewPage(preview, 0.9).page, 0);
  assert.deepEqual(previewPage({ ...preview, entries: [] }, 10), { page: 0, pages: 1, entries: [] });
});

test('v0.4 task descriptors survive upgrade while preview contents are never persisted', () => {
  const prior = ['succeeded', 'failed', 'cancelled', 'waiting', 'running'].map((status, index) => ({
    id: `v04-${index}`, input: `D:\\资料\\${index}.fse`, output: null, operation: 'verify', status,
    createdAt: 1234 + index, endedAt: index < 3 ? 5678 + index : undefined, batchId: 'v04-existing-batch',
    code: status === 'failed' ? 'authentication' : undefined, preview: { entries: [{ path: 'sensitive-name.txt' }] }
  }));
  const loaded = loadHistory(JSON.stringify(prior));
  assert.deepEqual(loaded.map(task => task.status), ['succeeded', 'failed', 'cancelled', 'interrupted', 'interrupted']);
  loaded.forEach((task, i) => { assert.equal(task.batchId, prior[i].batchId); assert.equal(task.input, prior[i].input); assert.equal(task.createdAt, prior[i].createdAt); });
  assert.equal(loaded[1].code, 'authentication');
  assert.equal(serializeHistory(loaded).includes('sensitive-name'), false);
  assert.equal(serializeHistory(loaded).includes('preview'), false);
});

test('protection refresh failure keeps recoverable records and retry clears stale state', async () => {
  const records = new ProtectionRecords();
  const saved = { roots: [{ id: 'saved-item', name: '资料' }], total: 1 };
  await records.refresh(async () => saved);
  const failure = { code: 'permission_denied' };
  await assert.rejects(records.refresh(async () => { throw failure; }), error => error === failure);
  assert.equal(records.snapshot, saved, '读取失败不能清空恢复入口');
  assert.equal(records.stale, true);
  assert.equal(records.error, failure);
  assert.equal(records.loading, false);
  await records.refresh(async () => ({ roots: [], total: 0 }));
  assert.deepEqual(records.snapshot, { roots: [], total: 0 }, '成功取得空记录与读取失败不同');
  assert.equal(records.stale, false);
  assert.equal(records.error, null);
});

test('protection refresh preserves displayed snapshot while loading and rejects overlapping reads', async () => {
  const records = new ProtectionRecords();
  const saved = { roots: ['first'] };
  await records.refresh(async () => saved);
  let finish;
  const next = records.refresh(() => new Promise(resolve => { finish = resolve; }));
  assert.equal(records.loading, true);
  assert.equal(records.snapshot, saved);
  let secondReadStarted = false;
  await assert.rejects(records.refresh(async () => { secondReadStarted = true; return {}; }), /busy/);
  assert.equal(secondReadStarted, false, '并发刷新不得抢先覆盖较新快照');
  finish({ roots: ['next'] });
  await next;
  assert.deepEqual(records.snapshot, { roots: ['next'] });
  assert.equal(records.loading, false);
});

test('protection action and index refresh retain separate outcomes on every failure combination', async () => {
  const operationError = { code: 'protection_conflict' };
  const refreshError = { code: 'permission_denied' };
  for (const operationFails of [false, true]) {
    for (const refreshFails of [false, true]) {
      const calls = [];
      const result = await runProtectionAction(async () => {
        calls.push('operation'); if (operationFails) throw operationError;
      }, async () => {
        calls.push('refresh'); if (refreshFails) throw refreshError;
      });
      assert.deepEqual(calls, ['operation', 'refresh'], '操作失败后也读取实际登记结果');
      assert.equal(result.operationSucceeded, !operationFails);
      assert.equal(result.refreshSucceeded, !refreshFails);
      assert.equal(result.operationError, operationFails ? operationError : undefined);
      assert.equal(result.refreshError, refreshFails ? refreshError : undefined);
    }
  }
});

test('protection batches run serially, preserve each failure and retry only failed items', async () => {
  const denied = { code: 'permission_denied' };
  const source = [{ name: '资料', note: '工作' }, { name: '资料', note: '归档' }, { name: '照片', note: '' }];
  let running = 0; let maximum = 0; const completed = [];
  const results = await runProtectionBatch(source, async item => {
    running++; maximum = Math.max(maximum, running);
    await new Promise(resolve => setTimeout(resolve, 1)); running--;
    if (item.note === '归档') throw denied;
  }, result => completed.push(result));
  assert.equal(maximum, 1, '权限变更必须串行处理');
  assert.deepEqual(results.map(result => result.succeeded), [true, false, true]);
  assert.equal(results[1].error, denied);
  assert.deepEqual(completed, results);
  const retried = [];
  const retry = await runProtectionBatch(results.filter(result => !result.succeeded).map(result => result.item), async item => { retried.push(item); });
  assert.deepEqual(retried, [source[1]], '已成功项目不得因重试重复执行');
  assert.equal(retry[0].succeeded, true);
  assert.deepEqual(await runProtectionBatch([], async () => assert.fail('空批次不能执行')), []);
});

test('protection notes use Unicode scalar limits and reject embedded control characters', () => {
  assert.equal(validProtectionNote('  工作资料  '), true);
  assert.equal(validProtectionNote('😀'.repeat(80)), true);
  assert.equal(validProtectionNote('😀'.repeat(81)), false);
  assert.equal(validProtectionNote('工作\n资料'), false);
  assert.equal(validProtectionNote('工作\u0000资料'), false);
  assert.equal(validProtectionNote(''), true);
});

test('managed service response rejects business errors, console hosts and malformed snapshots', () => {
  const transport = { host: 'service', healthy: true, last_tick_error: null };
  assert.throws(() => managedResponse({ version: 1, transport, data: {}, error: 'authentication' }), error => error.code === 'authentication');
  assert.throws(() => managedResponse({ version: 1, transport: { ...transport, host: 'console' }, data: {}, error: null }), error => error.code === 'service_unavailable');
  assert.throws(() => managedResponse({ version: 1, transport, data: null, error: null }), error => error.code === 'internal');
  assert.throws(() => managedResponse({ version: 9, transport, data: {}, error: null }));
  const root = { id: 'opaque', name: '资料', note: '工作', mode: 'hidden-locked', password_required: true, monitoring: true, unlocked_until: null, restoring: false, last_error: null, entries: 2, path: 'private-path', sid: 'private-sid', original_acl: 'private-acl' };
  const snapshot = managedStatus({ registered: 1, roots: [root] });
  assert.deepEqual(Object.keys(snapshot.roots[0]).sort(), ['id', 'name', 'note', 'mode', 'password_required', 'monitoring', 'unlocked_until', 'restoring', 'last_error', 'entries'].sort());
  assert.equal(JSON.stringify(snapshot).includes('private-'), false, '未知服务字段不进入UI状态');
  assert.throws(() => managedStatus({ registered: 2, roots: [root] }));
  assert.throws(() => managedStatus({ registered: 2, roots: [root, root] }));
  assert.throws(() => managedStatus({ registered: 1, roots: [{ ...root, unlocked_until: 'tomorrow' }] }));
});

test('managed requests release password references after transport and business failures', async () => {
  for (const transportFailure of [false, true]) {
    const command = { action: 'set_password', id: 'opaque', password: 'current-password-only', new_password: 'new-password-only' };
    await assert.rejects(requestManaged(command, async received => {
      assert.equal(received, command); assert.equal(received.password, 'current-password-only');
      if (transportFailure) throw { code: 'service_unavailable' };
      return { version: 1, transport: { host: 'service', healthy: true, last_tick_error: null }, data: null, error: 'authentication' };
    }), error => error.code === (transportFailure ? 'service_unavailable' : 'authentication'));
    assert.equal(command.password, ''); assert.equal(command.new_password, null);
    assert.equal(JSON.stringify(command).includes('password-only'), false);
  }
});

test('managed access password accepts Unicode characters within UTF-8 byte bounds', () => {
  assert.equal(validManagedPassword('1234567'), false);
  assert.equal(validManagedPassword('12345678'), true);
  assert.equal(validManagedPassword('😀'.repeat(8)), true);
  assert.equal(validManagedPassword('😀'.repeat(256)), true);
  assert.equal(validManagedPassword('😀'.repeat(257)), false);
});
