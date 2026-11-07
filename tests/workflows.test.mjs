import test from 'node:test';
import assert from 'node:assert/strict';
import { TaskQueue, MAX_SOURCES, batchSummary, canRetryTask, filterHistory, inspectSources, loadHistory, mergeSourcePaths, outputParent, serializeHistory } from '../ui/tasks.ts';
import { DEFAULT_PREFERENCES, loadPreferences, serializePreferences } from '../ui/preferences.ts';
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

test('retry output folders preserve drive roots and handle UNC paths', () => {
  assert.equal(outputParent('C:\\out.fse'), 'C:\\');
  assert.equal(outputParent('C:\\dir\\out.fse'), 'C:\\dir');
  assert.equal(outputParent('\\\\server\\share\\out.fse'), '\\\\server\\share');
  assert.equal(outputParent('\\\\?\\C:\\out.fse'), '\\\\?\\C:\\');
  assert.equal(outputParent('D:/dir/out.fse'), 'D:\\dir');
  assert.equal(outputParent('\\out.fse'), '\\');
  assert.equal(outputParent(null), '');
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

test('preferences validate persisted values and whitelist fields while migrating missing options', () => {
  assert.deepEqual(loadPreferences(null), DEFAULT_PREFERENCES);
  assert.deepEqual(loadPreferences('{}'), DEFAULT_PREFERENCES);
  assert.deepEqual(loadPreferences('{"density":"compact"}'), { ...DEFAULT_PREFERENCES, density: 'compact' });
  const changed = { ...DEFAULT_PREFERENCES, notification: 'failures', historyRetentionDays: 30, historyPaths: false,
    defaultProtectionMode: 'readonly-acl', defaultUnlockMinutes: 15, defaultOutputDirectory: 'D:\\新目录', fontSize: 'large' };
  assert.deepEqual(loadPreferences(serializePreferences(changed)), changed);
  assert.equal(serializePreferences({ ...changed, password: 'not-a-preference', input: 'private-file' }).includes('not-a-preference'), false);
  for (const raw of ['[]', 'null', 'false', '', '{bad', '{"notification":"always"}', '{"historyRetentionDays":1}', '{"historyPaths":"false"}',
    '{"defaultProtectionMode":"invalid"}', '{"defaultUnlockMinutes":0}', '{"defaultOutputDirectory":7}', '{"defaultOutputDirectory":"bad\\npath"}',
    '{"density":"tiny"}', '{"fontSize":"huge"}']) assert.throws(() => loadPreferences(raw), undefined, raw);
});

test('retention only removes expired terminal history and measures completed records from end time', () => {
  const day = 24 * 60 * 60 * 1000, now = 100 * day;
  const record = (id, status, age, endedAt) => ({ id, input: `${id}-source`, output: `${id}-output`, operation: 'encrypt', status, createdAt: now - age * day, endedAt });
  const waiting = record('waiting', 'waiting', 100), running = record('running', 'running', 100);
  const completedRecently = record('recent-end', 'succeeded', 99, now - day);
  const boundary = record('boundary', 'failed', 7);
  const old = ['succeeded', 'failed', 'cancelled', 'interrupted'].map(status => record(`old-${status}`, status, 8));
  const tasks = [waiting, running, completedRecently, boundary, ...old];
  const filtered = filterHistory(tasks, { historyRetentionDays: 7, historyPaths: true }, now);
  assert.deepEqual(filtered, [waiting, running, completedRecently, boundary]);
  assert.equal(filtered[0], waiting, '等待任务保留对象身份与执行路径');
  assert.equal(filtered[1], running, '运行任务保留对象身份，异步结果仍写入同一任务');
  assert.equal(tasks.length, 8, '纯过滤不改变原历史');
  assert.deepEqual(filterHistory(tasks, DEFAULT_PREFERENCES, now), tasks);
  assert.throws(() => filterHistory(tasks, { historyRetentionDays: -1, historyPaths: true }), /preferences_invalid/);
});

test('path privacy removes both paths from terminal records and anonymous history cannot retry', () => {
  const tasks = ['waiting', 'running', 'succeeded', 'failed', 'cancelled', 'interrupted'].map((status, index) => ({
    id: `private-${index}`, input: `private-input-${index}`, output: `private-output-${index}`, operation: 'encrypt', status, createdAt: 1, batchId: 'batch',
  }));
  const policy = { historyRetentionDays: 0, historyPaths: false };
  const filtered = filterHistory(tasks, policy);
  assert.deepEqual(filtered.slice(0, 2), tasks.slice(0, 2), '排队与运行记录仍可用于重启诊断');
  for (const task of filtered.slice(2)) {
    assert.equal(task.input, ''); assert.equal(task.output, null); assert.equal(task.pathsHidden, true);
    assert.equal(canRetryTask(task), false);
  }
  assert.equal(tasks[2].input, 'private-input-2', '不改变调用者的旧快照');
  const saved = serializeHistory(tasks, policy);
  assert.equal(saved.includes('private-input-2'), false); assert.equal(saved.includes('private-output-2'), false);
  const loaded = loadHistory(saved);
  assert.deepEqual(loaded.slice(2), filtered.slice(2));
  assert.equal(canRetryTask(loaded[2]), false);
  assert.equal(canRetryTask({ ...tasks[2], input: '' }), false, '旧空路径记录也不能作为重试来源');
  assert.equal(filterHistory(loaded, DEFAULT_PREFERENCES)[2].pathsHidden, true, '重新开启路径保存不能恢复已经删除的路径');
  assert.throws(() => loadHistory(JSON.stringify([{ ...filtered[2], status: 'waiting' }])), /history_invalid/);
  assert.throws(() => loadHistory(JSON.stringify([{ ...filtered[2], input: 'unexpected-path' }])), /history_invalid/);
  const queue = new TaskQueue([], () => {}, () => {});
  assert.throws(() => queue.add([{ input: '', output: null, operation: 'verify' }], 'secret'), /task_path_unavailable/);
});

test('history settings save before replacing memory and future completions follow current privacy', async () => {
  let policy = { historyRetentionDays: 0, historyPaths: true }, fail = false, saved = '';
  const old = { id: 'old', input: 'private-old-input', output: 'private-old-output', operation: 'encrypt', status: 'succeeded', createdAt: 1 };
  const queue = new TaskQueue([old], tasks => { if (fail) throw new Error('storage_failed'); saved = serializeHistory(tasks); }, () => {}, () => policy);
  fail = true;
  assert.throws(() => queue.anonymizeHistory(), /storage_failed/);
  assert.equal(queue.tasks[0], old); assert.equal(queue.tasks[0].input, 'private-old-input');
  assert.throws(() => queue.pruneHistory(7), /storage_failed/);
  assert.equal(queue.tasks.length, 1);
  fail = false; policy = { historyRetentionDays: 0, historyPaths: false };
  queue.applyHistoryPreferences(policy);
  assert.equal(queue.tasks[0].pathsHidden, true); assert.equal(saved.includes('private-old-input'), false);
  queue.add([{ input: 'private-current-input', output: 'private-current-output', operation: 'encrypt' }], 'secret');
  await queue.start(async task => {
    assert.equal(task.input, 'private-current-input');
    assert.equal(saved.includes('private-current-input'), true, '任务完成前仍保存必要执行描述');
    queue.applyHistoryPreferences(policy);
  }, () => 'internal');
  assert.equal(queue.tasks[1].status, 'succeeded'); assert.equal(queue.tasks[1].pathsHidden, true);
  assert.equal(saved.includes('private-current-input'), false); assert.equal(saved.includes('private-current-output'), false);
  queue.pruneHistory(7);
  assert.equal(queue.tasks.length, 1); assert.equal(queue.tasks[0].status, 'succeeded');
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
