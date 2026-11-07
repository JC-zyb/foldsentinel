import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 通过生产设置页操作真实 localStorage；夹具只提供等待任务和只读恢复元数据，不执行原生文件操作。
const { values } = parseArgs({ options: {
  playwright: { type: 'string' }, browser: { type: 'string' }, report: { type: 'string' },
  url: { type: 'string', default: 'http://127.0.0.1:1420/' }
} });
assert.ok(values.playwright && values.browser && values.report, '需要 --playwright、--browser 和 --report');
const url = new URL(values.url);
assert.ok(url.protocol === 'http:' && url.hostname === '127.0.0.1', '仅测试本机开发服务');
const reportPath = resolve(values.report), screenshotRoot = join(dirname(reportPath), 'screenshots');
await mkdir(dirname(reportPath), { recursive: true });
const reportFile = await open(reportPath, 'wx');
await mkdir(screenshotRoot);
const version = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')).version;
const { chromium } = await import(pathToFileURL(resolve(values.playwright)).href);
const browser = await chromium.launch({ executablePath: resolve(values.browser), headless: true, args: ['--no-proxy-server'] });
const checks = [], errors = [], blockedRequests = [];
const preferencesKey = 'foldsentinel.preferences.v1', historyKey = 'foldsentinel.history.v1';
const groups = ['notifications', 'history', 'diagnostics', 'defaults', 'window', 'display'];
let passed = false;

async function setup(context, seed = {}) {
  await context.addInitScript(({ seed, preferencesKey, historyKey }) => {
    if (sessionStorage.getItem('settings-validation-seeded')) return;
    sessionStorage.setItem('settings-validation-seeded', 'true');
    if (seed.preferences) localStorage.setItem(preferencesKey, JSON.stringify(seed.preferences));
    if (seed.history) localStorage.setItem(historyKey, JSON.stringify(seed.history));
    localStorage.setItem('settings-validation-unrelated-record', 'retained');
  }, { seed, preferencesKey, historyKey });
  await context.route('**/*', async route => {
    const target = new URL(route.request().url());
    if (target.origin !== url.origin) { blockedRequests.push(route.request().url()); await route.abort(); return; }
    if (target.pathname === '/ui/main.ts') {
      const response = await route.fetch();
      // 仅增加观察器和内存元数据入口，保留所有生产设置处理器；从不调用 queue.start 或原生命令。
      await route.fulfill({ response, body: await response.text() + `
window.settingsValidation = {
  tasks: () => queue.tasks.map(task => ({ ...task })),
  failNextHistoryWrite: () => {
    const set = Storage.prototype.setItem;
    Storage.prototype.setItem = function(key, value) {
      if (this === localStorage && key === HISTORY_KEY) {
        Storage.prototype.setItem = set;
        throw new DOMException('Explicit history-write failure fixture', 'QuotaExceededError');
      }
      return set.call(this, key, value);
    };
  },
  addWaiting: () => queue.add([{ input: 'D:\\\\settings-fixture\\\\queued-needed.txt', output: 'D:\\\\settings-fixture\\\\queued-needed.fse', operation: 'encrypt' }], 'test-only-memory-password'),
  addWaitingWithFeedback: () => {
    try { window.settingsValidation.addWaiting(); return true; }
    catch (error) { showError(error); return false; }
  },
  seedRecoveryMetadata: () => {
    recordState.snapshot = { roots: [{ id: 'settings-protection-metadata', name: '保护恢复展示记录', mode: 'locked', note: '', entry_count: 1 }], total: 1, overall_total: 1, offset: 0 };
    jobs = [{ id: 'settings-recovery-metadata', status: 'cleanable', record: { id: 'settings-recovery-metadata', input: 'D:\\\\settings-fixture\\\\input', output: 'D:\\\\settings-fixture\\\\output.fse', staging: 'D:\\\\settings-fixture\\\\staging', decrypt: false }, code: null }];
    renderRecords(); renderJobs();
  },
  recoveryMetadata: () => JSON.stringify({ protection: recordState.snapshot, recovery: jobs })
  ,servicePresentation: () => {
    managedUi.available = true; managedUi.stale = false;
    managedUi.snapshot = { registered: 1, roots: [{ id: 'settings-service-metadata', name: '默认时长展示记录', note: '', mode: 'locked', password_required: true, monitoring: true, unlocked_until: null, restoring: false, last_error: null, entries: 1 }] };
    managedUi.render();
  }
};
` });
      return;
    }
    await route.continue();
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url.href, { waitUntil: 'domcontentloaded' });
  await page.locator('#nav-settings').click();
  await page.locator('#settings-density').waitFor();
  return page;
}

async function stored(page, key) { return page.evaluate(key => JSON.parse(localStorage.getItem(key)), key); }
async function tasks(page) { return page.evaluate(() => window.settingsValidation.tasks()); }
async function expectPreference(page, key, value) {
  await page.waitForFunction(({ preferencesKey, key, value }) => JSON.parse(localStorage.getItem(preferencesKey))?.[key] === value, { preferencesKey, key, value });
}
async function dimensions(page, metadata) {
  await page.waitForFunction(() => document.getAnimations().every(animation => !animation.pending && animation.playState !== 'running'));
  const result = await page.locator('#settings-panel').evaluate(panel => ({
    viewport: innerWidth, page: document.documentElement.scrollWidth,
    outside: [...panel.querySelectorAll('label, p, button, select, input, summary, h2, strong')].filter(element => {
      if (element.closest('[hidden]') || element.classList.contains('sr-only')) return false;
      const box = element.getBoundingClientRect();
      return box.width > 0 && box.height > 0 && (box.left < -1 || box.right > innerWidth + 1 || (!(element instanceof HTMLInputElement) && element.scrollWidth > element.clientWidth + 2));
    }).map(element => element.id || element.className || element.tagName)
  }));
  assert.ok(result.page <= result.viewport + 1, JSON.stringify({ ...metadata, result }));
  assert.deepEqual(result.outside, [], JSON.stringify({ ...metadata, result }));
  return result;
}

try {
  const now = Date.now(), day = 24 * 60 * 60 * 1000;
  const history = [
    { id: 'settings-old', input: 'D:\\settings-fixture\\old.txt', output: 'D:\\settings-fixture\\old.fse', operation: 'encrypt', status: 'succeeded', createdAt: now - 60 * day, endedAt: now - 40 * day },
    { id: 'settings-recent', input: 'D:\\settings-fixture\\recent.txt', output: 'D:\\settings-fixture\\recent.fse', operation: 'encrypt', status: 'failed', code: 'authentication', createdAt: now - day, endedAt: now - day }
  ];
  const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const page = await setup(context, { history });
    for (const group of groups) assert.equal(await page.locator(`#settings-${group}-group`).count(), 1, group);
    for (const id of ['language', 'theme', 'autostart', 'check-updates', 'settings-app-version']) assert.equal(await page.locator(`#${id}`).count(), 1, `保留原有设置 ${id}`);
    for (const id of ['autostart', 'check-updates', 'settings-close-to-tray', 'settings-start-minimized', 'settings-choose-output', 'settings-export-diagnostic']) assert.equal(await page.locator(`#${id}`).isDisabled(), true, `浏览器禁用桌面入口 ${id}`);
    await page.locator('.settings-data-locations').evaluate(details => { details.open = true; });
    for (const kind of ['app_data', 'recovery', 'protection']) assert.equal(await page.locator(`#settings-open-${kind}`).isDisabled(), true);
    checks.push({ operation: 'groups-and-desktop-boundary', groups: 6, legacy_settings_preserved: true, native_controls_disabled_in_browser: true });

    await page.locator('#settings-notification').selectOption('failures');
    await expectPreference(page, 'notification', 'failures');
    await page.locator('#settings-density').selectOption('compact');
    await expectPreference(page, 'density', 'compact');
    await page.locator('#settings-font-size').selectOption('large');
    await expectPreference(page, 'fontSize', 'large');
    await page.locator('#settings-default-protection').selectOption('readonly-acl');
    await expectPreference(page, 'defaultProtectionMode', 'readonly-acl');
    await page.locator('#settings-default-unlock').selectOption('15');
    await expectPreference(page, 'defaultUnlockMinutes', 15);
    await page.locator('#language').selectOption('en');
    await page.locator('#theme').selectOption('dark');
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.locator('#nav-settings').click();
    for (const [id, value] of [['settings-notification', 'failures'], ['settings-density', 'compact'], ['settings-font-size', 'large'], ['settings-default-protection', 'readonly-acl'], ['settings-default-unlock', '15'], ['language', 'en'], ['theme', 'dark']]) assert.equal(await page.locator(`#${id}`).inputValue(), value, `重启保留 ${id}`);
    assert.equal(await page.locator('html').getAttribute('data-density'), 'compact');
    assert.equal(await page.locator('html').getAttribute('data-font-size'), 'large');
    checks.push({ operation: 'real-preference-persistence', notification: true, defaults: true, appearance: true, language_and_theme: true, reload: true });

    await page.evaluate(() => { window.settingsValidation.addWaiting(); window.settingsValidation.seedRecoveryMetadata(); });
    const waiting = (await tasks(page)).find(task => task.status === 'waiting');
    const recoveryBefore = await page.evaluate(() => window.settingsValidation.recoveryMetadata());
    assert.ok(waiting);
    await page.locator('#settings-history-retention').selectOption('7');
    assert.equal(await page.locator('#settings-confirm-history').isVisible(), true);
    assert.equal((await tasks(page)).length, 3, '确认前不清理');
    await page.locator('#settings-cancel-history').click();
    assert.equal(await page.locator('#settings-history-retention').inputValue(), '0');
    assert.equal((await tasks(page)).length, 3);
    await page.locator('#settings-history-retention').selectOption('7');
    await page.evaluate(() => window.settingsValidation.failNextHistoryWrite());
    await page.locator('#settings-confirm-history').click();
    await page.waitForFunction(() => document.querySelector('#settings-history-feedback').getAttribute('role') === 'alert');
    assert.equal((await stored(page, preferencesKey)).historyRetentionDays, 0, '历史写入失败后回滚已写入的新偏好');
    assert.equal((await tasks(page)).length, 3, '历史写入失败后保留内存记录');
    assert.equal((await stored(page, historyKey)).length, 3, '历史写入失败后保留旧磁盘记录');
    assert.equal(await page.locator('#settings-confirm-history').isVisible(), true, '错误明确可见并保留确认入口以重试');
    await page.locator('#settings-cancel-history').click();
    checks.push({ operation: 'history-persistence-failure', explicit_error: true, preference_rolled_back: true, memory_and_old_storage_kept: true, retry_available: true });
    await page.locator('#settings-history-retention').selectOption('7');
    await page.locator('#settings-confirm-history').click();
    await expectPreference(page, 'historyRetentionDays', 7);
    assert.deepEqual((await tasks(page)).map(task => task.id), ['settings-recent', waiting.id]);
    assert.deepEqual((await tasks(page)).find(task => task.id === waiting.id), waiting, '保留等待任务及执行描述');
    assert.equal(await page.evaluate(() => window.settingsValidation.recoveryMetadata()), recoveryBefore);
    checks.push({ operation: 'history-retention-confirmation', cancellation_keeps_records: true, expired_history_removed: true, pending_record_unchanged: true, recovery_metadata_unchanged: true });

    await page.locator('#settings-history-paths').click();
    assert.equal((await tasks(page)).find(task => task.id === 'settings-recent').input, history[1].input, '确认前保留路径');
    await page.locator('#settings-cancel-history').click();
    assert.equal(await page.locator('#settings-history-paths').isChecked(), true);
    await page.locator('#settings-history-paths').click();
    await page.locator('#settings-confirm-history').click();
    await expectPreference(page, 'historyPaths', false);
    const anonymous = (await tasks(page)).find(task => task.id === 'settings-recent');
    assert.equal(anonymous.input, ''); assert.equal(anonymous.output, null); assert.equal(anonymous.pathsHidden, true);
    assert.deepEqual((await tasks(page)).find(task => task.id === waiting.id), waiting);
    const saved = await stored(page, historyKey);
    assert.equal(JSON.stringify(saved).includes('recent.txt'), false);
    assert.equal(JSON.stringify(saved).includes('recent.fse'), false);
    await page.locator('#nav-vault').click();
    await page.locator('#history-panel').evaluate(details => { details.open = true; });
    assert.equal(await page.locator('#history-list .task-item').count(), 1);
    assert.equal(await page.locator('#history-list [data-task-action="retry"]').count(), 0, '匿名历史没有直接重试入口');
    checks.push({ operation: 'history-path-privacy', confirmation_required: true, cancellation_keeps_paths: true, input_and_output_removed_from_storage: true, no_retry: true, pending_path_retained: true });

    await page.locator('#nav-settings').click();
    await page.locator('#settings-clear-history').click();
    assert.equal((await tasks(page)).length, 2, '清空前须确认');
    await page.locator('#settings-cancel-history').click();
    assert.equal((await tasks(page)).length, 2);
    await page.locator('#settings-clear-history').click();
    await page.locator('#settings-confirm-history').click();
    await page.waitForFunction(() => window.settingsValidation.tasks().length === 1);
    assert.deepEqual(await tasks(page), [waiting]);
    assert.deepEqual(await stored(page, historyKey), [waiting]);
    assert.equal(await page.evaluate(() => window.settingsValidation.recoveryMetadata()), recoveryBefore, '清空操作历史不改变保护与恢复展示元数据');
    assert.equal(await page.evaluate(() => localStorage.getItem('settings-validation-unrelated-record')), 'retained');
    assert.equal(await page.locator('#settings-clear-history').isDisabled(), true);
    checks.push({ operation: 'clear-history-confirmation', cancellation_keeps_records: true, pending_kept: true, recovery_metadata_kept: true, unrelated_storage_kept: true });

    await page.locator('#nav-attributes').click();
    await page.locator('#open-attribute-form').click();
    assert.equal(await page.locator('#attribute-mode').inputValue(), 'readonly-acl', '新的本地保护表单采用默认方式');
    await page.locator('#attribute-mode').selectOption('locked');
    await page.locator('#nav-settings').click();
    await page.locator('#settings-default-protection').selectOption('hidden');
    await expectPreference(page, 'defaultProtectionMode', 'hidden');
    await page.locator('#nav-attributes').click();
    assert.equal(await page.locator('#attribute-mode').inputValue(), 'locked', '已填写的表单不被默认值覆盖');
    await page.locator('#cancel-attribute-add').click();
    await page.locator('#open-attribute-form').click();
    assert.equal(await page.locator('#attribute-mode').inputValue(), 'hidden', '取消后新表单采用新的默认方式');
    checks.push({ operation: 'new-protection-default', new_form_uses_preference: true, filled_form_preserved: true });
    await page.locator('#nav-settings').click();
    await page.locator('#settings-default-unlock').selectOption('30');
    await expectPreference(page, 'defaultUnlockMinutes', 30);
    await page.locator('#nav-attributes').click();
    await page.locator('#protection-service-tab').click();
    await page.evaluate(() => window.settingsValidation.servicePresentation());
    await page.locator('[data-managed-action="unlock"]').click();
    assert.equal(await page.locator('#managed-duration').inputValue(), '1800', '新临时解锁表单使用设置中的30分钟默认值');
    checks.push({ operation: 'new-unlock-default', live_preference_getter: true, default_minutes: 30, native_commands_not_submitted: true });
  } finally { await context.close(); }

  const invalidPreferencesContext = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const invalidPreferences = { historyRetentionDays: 7, historyPaths: 'invalid-value', notification: 'all' };
    const page = await setup(invalidPreferencesContext, { history, preferences: invalidPreferences });
    const rawHistory = JSON.stringify(history), rawPreferences = JSON.stringify(invalidPreferences);
    assert.equal(await page.evaluate(key => localStorage.getItem(key), historyKey), rawHistory, '偏好损坏时不按默认策略改写历史');
    assert.equal(await page.evaluate(key => localStorage.getItem(key), preferencesKey), rawPreferences, '偏好损坏时不静默覆盖原配置');
    await page.reload({ waitUntil: 'domcontentloaded' });
    assert.equal(await page.locator('#result').isVisible(), true, '偏好损坏在启动时明确报告');
    assert.match(await page.locator('#result').textContent(), /偏好|preferences/i);
    const before = await tasks(page);
    assert.equal(await page.evaluate(() => window.settingsValidation.addWaitingWithFeedback()), false, '损坏偏好期间队列保存拒绝新增任务');
    assert.deepEqual(await tasks(page), before, '被拒绝的新增不改变内存任务');
    assert.equal(await page.evaluate(key => localStorage.getItem(key), historyKey), rawHistory);
    assert.equal(await page.locator('#result').getAttribute('role'), 'alert', '真实错误处理器显式报告保存拒绝');
    assert.match(await page.locator('#result').textContent(), /偏好|preferences/i);
    assert.equal(await page.locator('#start-queue').isDisabled(), true);
    await page.locator('#nav-settings').click();
    await page.locator('#settings-notification').selectOption('failures');
    await expectPreference(page, 'notification', 'failures');
    assert.equal(await page.evaluate(() => window.settingsValidation.addWaitingWithFeedback()), true, '明确保存新偏好后恢复队列新增');
    assert.equal((await tasks(page)).length, before.length + 1);
    assert.equal(await page.locator('#start-queue').isDisabled(), false, '偏好恢复后等待任务可启动，但本夹具从不启动');
    checks.push({ operation: 'invalid-preferences-recovery', raw_history_preserved: true, raw_preferences_not_overwritten_on_load: true, add_rejected_with_explicit_error: true, memory_unchanged_on_rejection: true, explicit_setting_save_restores_queue: true });
  } finally { await invalidPreferencesContext.close(); }

  const legacyHistoryContext = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const page = await setup(legacyHistoryContext, { history });
    await page.evaluate(() => { window.settingsValidation.addWaiting(); window.settingsValidation.seedRecoveryMetadata(); });
    const before = await tasks(page);
    const waiting = before.find(task => task.status === 'waiting');
    const recoveryBefore = await page.evaluate(() => window.settingsValidation.recoveryMetadata());
    assert.ok(waiting);
    await page.locator('#nav-vault').click();
    await page.locator('#history-panel').evaluate(details => { details.open = true; });
    await page.locator('#clear-history').click();
    assert.equal(await page.locator('#settings-confirm-history').isVisible(), true, '旧历史入口也进入相同确认流程');
    assert.deepEqual(await tasks(page), before, '旧入口确认前保留全部任务');
    await page.locator('#settings-cancel-history').click();
    assert.deepEqual(await tasks(page), before, '取消旧入口清空后保留记录');
    await page.locator('#nav-vault').click();
    await page.locator('#history-panel').evaluate(details => { details.open = true; });
    await page.locator('#clear-history').click();
    await page.locator('#settings-confirm-history').click();
    await page.waitForFunction(() => window.settingsValidation.tasks().length === 1);
    assert.deepEqual(await tasks(page), [waiting], '旧入口只清理终态记录，等待任务保留');
    assert.deepEqual(await stored(page, historyKey), [waiting]);
    assert.equal(await page.evaluate(() => window.settingsValidation.recoveryMetadata()), recoveryBefore);
    checks.push({ operation: 'legacy-history-clear-confirmation', shared_confirmation: true, cancellation_keeps_records: true, pending_kept: true, recovery_metadata_kept: true });
  } finally { await legacyHistoryContext.close(); }

  const outputContext = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const output = 'D:\\settings-fixture\\default-output';
    const page = await setup(outputContext, { preferences: { defaultOutputDirectory: output } });
    assert.equal(await page.locator('#settings-default-output').inputValue(), output);
    await page.locator('#nav-vault').click();
    assert.equal(await page.locator('#output-path').inputValue(), output, '初始新加密表单采用已保存的默认输出目录');
    await page.locator('#decrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), '', '加密默认目录不泄漏到恢复操作');
    const decryptDraft = 'D:\\settings-fixture\\restore-draft';
    await page.locator('#output-path').fill(decryptDraft);
    await page.locator('#verify-tab').click();
    assert.equal(await page.locator('#output-fields').isHidden(), true);
    await page.locator('#encrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), output, '返回加密时保留加密默认目录');
    await page.locator('#output-path').fill('D:\\settings-fixture\\custom-draft');
    await page.locator('#decrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), decryptDraft, '恢复草稿与加密草稿分别保留');
    await page.locator('#encrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), 'D:\\settings-fixture\\custom-draft');
    await page.locator('#nav-settings').click();
    await page.locator('#settings-reset-output').click();
    await expectPreference(page, 'defaultOutputDirectory', '');
    await page.locator('#nav-vault').click();
    assert.equal(await page.locator('#output-path').inputValue(), 'D:\\settings-fixture\\custom-draft', '更改默认目录不覆盖当前草稿');
    await page.locator('#decrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), decryptDraft, '更改加密默认目录不覆盖恢复草稿');
    await page.locator('#encrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), 'D:\\settings-fixture\\custom-draft');
    await page.reload({ waitUntil: 'domcontentloaded' });
    assert.equal(await page.locator('#output-path').inputValue(), '', '下一次新表单采用已重置的默认目录');
    checks.push({ operation: 'output-directory-default', persisted_path_used_by_new_form: true, current_draft_preserved: true, reset_applies_to_next_form: true });
    checks.push({ operation: 'mode-specific-output-drafts', encryption_default_not_used_for_restore: true, encryption_and_restore_drafts_independent: true, changed_default_keeps_both_drafts: true });
  } finally { await outputContext.close(); }

  for (const width of [375, 720, 1080, 1692]) for (const scheme of ['light', 'dark']) for (const language of ['zh-CN', 'en']) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, locale: language, colorScheme: scheme });
    try {
      const page = await setup(context);
      await page.locator('#language').selectOption(language);
      await page.locator('#theme').selectOption(scheme);
      assert.equal(await page.locator('html').getAttribute('lang'), language);
      assert.match(await page.locator('#settings-history-title').textContent(), language === 'zh-CN' ? /历史/ : /History/);
      const standard = await dimensions(page, { width, scheme, language, density: 'standard', font: 'standard' });
      await page.locator('#settings-density').selectOption('compact');
      await page.locator('#settings-font-size').selectOption('large');
      const larger = await dimensions(page, { width, scheme, language, density: 'compact', font: 'large' });
      const checkboxTarget = await page.locator('#settings-history-paths').evaluate(input => ({ width: input.parentElement.getBoundingClientRect().width, height: input.parentElement.getBoundingClientRect().height }));
      assert.ok(checkboxTarget.width >= 44 && checkboxTarget.height >= 44, '历史路径开关点击区域至少44px');
      await page.locator('#settings-history-paths').focus();
      await page.keyboard.press('Space');
      assert.equal(await page.locator('#settings-confirm-history').isVisible(), true);
      await page.keyboard.press('Escape');
      assert.equal(await page.locator('#settings-history-paths').evaluate(input => document.activeElement === input), true, 'Escape取消后返回来源开关');
      await page.locator('.settings-data-locations').evaluate(details => { details.open = true; });
      await dimensions(page, { width, scheme, language, details: 'open' });
      await page.evaluate(() => scrollTo(0, 0));
      const screenshot = `${width}-${scheme}-${language}-compact-large.png`;
      await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
      checks.push({ operation: 'responsive-settings', width, scheme, language, standard, compact_large: larger, keyboard_confirmation: true, touch_target: checkboxTarget, screenshot });
    } finally { await context.close(); }
  }
  assert.deepEqual(errors, []); assert.deepEqual(blockedRequests, []); passed = true;
} catch (error) { errors.push(String(error)); throw error; }
finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors, blocked_requests: blockedRequests,
    limitations: ['Browser checks exercise production UI handlers and real localStorage; native dialogs, notifications, tray behavior and real file protection are not invoked.', 'Waiting tasks are added to the real queue as metadata and are never started. Recovery and protection fixtures supply read-only display metadata, not successful file-operation responses.', 'The output directory preference is seeded in localStorage because the native picker is disabled in browsers. Native Windows DPI and notification delivery require desktop acceptance.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`Settings UI checks passed: ${checks.length}`);
