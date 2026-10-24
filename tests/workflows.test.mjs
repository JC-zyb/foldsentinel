import test from 'node:test';
import assert from 'node:assert/strict';
import { TaskQueue, loadHistory, mergeSourcePaths, serializeHistory } from '../ui/tasks.ts';
import { detectLanguage, messages } from '../ui/i18n.ts';

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
