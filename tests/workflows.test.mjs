import test from 'node:test';
import assert from 'node:assert/strict';
import { TaskQueue, MAX_SOURCES, batchSummary, inspectSources, loadHistory, mergeSourcePaths, serializeHistory } from '../ui/tasks.ts';
import { detectLanguage, messages } from '../ui/i18n.ts';
import { previewPage, PREVIEW_PAGE_SIZE } from '../ui/preview.ts';
import { attributeKeys, attributeStateKeys } from '../ui/attributes.ts';

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

test('attribute status distinguishes unavailable reads, unmanaged flags and policy mismatches', () => {
  assert.deepEqual(attributeKeys(null), ['attributeUnavailable']);
  assert.deepEqual(attributeKeys(0x20), ['attributeArchive']);
  assert.deepEqual(attributeKeys(0x17), ['attributeHidden', 'attributeSystem', 'attributeReadonly', 'attributeDirectory']);
  assert.deepEqual(attributeKeys(0), ['attributeNone']);
  for (const key of Object.values(attributeStateKeys)) assert.ok(messages[key]);
  assert.notEqual(attributeStateKeys.changed, attributeStateKeys.unreadable);
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
