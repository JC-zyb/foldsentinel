import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 仅在本机浏览器的模块响应里替换 IPC 依赖，运行真实 UI 事件及状态转换。
// 内存响应不证明真实属性操作成功；本脚本不接触文件、SCM、注册表或原生对话框。
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
let passed = false;

const transport = String.raw`
const addFlowTransport = {
  desktop: false, roots: [], ids: new Map(), calls: [], failPaths: new Set(), failRefresh: false,
  holdPreview: false, holdOperation: false, releasePreview: null, releaseOperation: null,
  activeOperations: 0, peakOperations: 0, unexpected: []
};
function isTauri() { return addFlowTransport.desktop; }
async function invoke(command, args = {}) {
  const fixture = addFlowTransport;
  fixture.calls.push({ command, ...structuredClone(args) });
  if (command === 'preview_protection_add') {
    if (fixture.holdPreview) await new Promise(resolve => { fixture.releasePreview = resolve; });
    return args.paths.map((path, index) => ({ index, name: protectionOuterName(path), duplicate_name: false }));
  }
  if (command === 'protection_status') {
    if (fixture.failRefresh) throw { code: 'io' };
    const query = (args.query ?? '').toLocaleLowerCase();
    const roots = fixture.roots.filter(root => (root.name + ' ' + (root.note ?? '')).toLocaleLowerCase().includes(query));
    return { roots: structuredClone(roots.slice(args.offset ?? 0, (args.offset ?? 0) + 50)), total: roots.length, overall_total: fixture.roots.length, offset: args.offset ?? 0 };
  }
  if (command === 'attribute_operation') {
    fixture.activeOperations++; fixture.peakOperations = Math.max(fixture.peakOperations, fixture.activeOperations);
    try {
      if (fixture.holdOperation) await new Promise(resolve => { fixture.releaseOperation = resolve; });
      if (fixture.failPaths.has(args.path)) throw { code: 'permission_denied' };
      let id = fixture.ids.get(args.path);
      if (!id) { id = 'add-fixture-' + (fixture.ids.size + 1); fixture.ids.set(args.path, id); }
      fixture.roots = fixture.roots.filter(root => root.id !== id);
      if (args.mode !== 'none') fixture.roots.push({ id, name: protectionOuterName(args.path), note: args.note ?? '', mode: args.mode });
      return;
    } finally { fixture.activeOperations--; }
  }
  if (command === 'restore_attributes') { fixture.roots = fixture.roots.filter(root => root.id !== args.id); return; }
  if (command === 'service_installation_status') return 'absent';
  if (command === 'managed_protection') throw { code: 'service_unavailable' };
  if (command === 'get_autostart') return false;
  fixture.unexpected.push(command); throw new Error('Unexpected fixture command: ' + command);
}
`;
const accessors = String.raw`
window.protectionAddFixture = {
  start: async () => { addFlowTransport.desktop = true; await refreshRecords(0); },
  message: (key, values) => t(key, values),
  configure: ({ failPaths = [], failRefresh = false, holdPreview = false, holdOperation = false } = {}) => {
    addFlowTransport.failPaths = new Set(failPaths); addFlowTransport.failRefresh = failRefresh;
    addFlowTransport.holdPreview = holdPreview; addFlowTransport.holdOperation = holdOperation;
  },
  releasePreview: () => { addFlowTransport.holdPreview = false; addFlowTransport.releasePreview?.(); },
  releaseOperation: () => { addFlowTransport.holdOperation = false; addFlowTransport.releaseOperation?.(); },
  seedQuery: query => { recordsQuery = query; input('protection-search').value = query; renderRecords(); },
  seedOldVerification: () => { for (const root of recordState.snapshot?.roots ?? []) rootVerification.set(root.id, { state: 'consistent', checked_at: 1 }); renderRecords(); },
  state: () => ({ busy: uiBusy, previewBusy: protectionPreviewBusy, previewFailed: protectionPreviewFailed,
    draft: [...protectionDraft], notes: [...protectionDraftNotes], results: protectionAddResults.map(result => ({ path: result.item.path, succeeded: result.succeeded })),
    query: recordsQuery, rootFeedback: [...rootFeedback], recordNotice, stale: recordState.stale,
    calls: structuredClone(addFlowTransport.calls), roots: structuredClone(addFlowTransport.roots), unexpected: [...addFlowTransport.unexpected], peakOperations: addFlowTransport.peakOperations,
    waitingPreview: !!addFlowTransport.releasePreview, waitingOperation: !!addFlowTransport.releaseOperation })
};
`;

async function setup(context, language) {
  await context.route('**/*', async route => {
    const requestUrl = new URL(route.request().url());
    if (requestUrl.origin !== url.origin) { blockedRequests.push(route.request().url()); await route.abort(); return; }
    if (requestUrl.pathname === '/ui/main.ts') {
      const response = await route.fetch();
      const source = await response.text();
      const dependency = /import\s*\{\s*Channel,\s*invoke,\s*isTauri\s*\}/;
      assert.match(source, dependency, '生产依赖导入变化时显式更新夹具，不能静默改错测试目标');
      await route.fulfill({ response, body: transport + source.replace(dependency, 'import { Channel, invoke as nativeInvoke, isTauri as nativeIsTauri }') + accessors });
      return;
    }
    await route.continue();
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url.href, { waitUntil: 'domcontentloaded' });
  await page.locator('#nav-settings').click(); await page.locator('#language').selectOption(language);
  await page.locator('#nav-attributes').click();
  await page.evaluate(() => window.protectionAddFixture.start());
  return page;
}
async function state(page) { return page.evaluate(() => window.protectionAddFixture.state()); }
async function configure(page, options = {}) { await page.evaluate(options => window.protectionAddFixture.configure(options), options); }
async function idle(page) { await page.waitForFunction(() => { const state = window.protectionAddFixture.state(); return !state.busy && !state.previewBusy; }); }
async function add(page, path, note = '') {
  await page.locator('#open-attribute-form').click();
  await page.locator('#attribute-path').fill(path); await page.locator('#append-protection-path').click(); await idle(page);
  const notes = page.locator('#protection-draft-list input');
  if (note) await notes.last().fill(note);
}
async function apply(page, retry = false) { await page.locator(retry ? '#retry-protection-add' : '#apply-protection-draft').click(); await idle(page); }
async function verifyNoUnexpected(page) { assert.deepEqual((await state(page)).unexpected, []); }
async function withPage(language, run, options = {}) {
  const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: language, ...options });
  try { const page = await setup(context, language); await run(page); await verifyNoUnexpected(page); }
  finally { await context.close(); }
}

const alpha = 'D:\\add-ui-fixture\\alpha.txt', beta = 'D:\\add-ui-fixture\\beta.txt';
try {
  for (const language of ['zh-CN', 'en']) {
    await withPage(language, async page => {
      await page.locator('#open-attribute-form').click();
      assert.equal(await page.locator('#apply-protection-draft').isDisabled(), true, '没有草稿和路径时禁止提交');
      await page.locator('#attribute-path').fill(alpha);
      assert.equal(await page.locator('#apply-protection-draft').isDisabled(), true, '未确认的路径须先通过添加按钮或Enter加入草稿');
      await page.locator('#attribute-path').press('Enter'); await idle(page);
      assert.deepEqual((await state(page)).draft, [alpha]);
      assert.equal(await page.locator('#attribute-path').inputValue(), '');
      await page.locator('#protection-draft-list input').fill('控制字符\u0001');
      assert.equal(await page.locator('#apply-protection-draft').isDisabled(), true, '非法备注阻止提交');
      await page.locator('#protection-draft-list input').fill('review note');
      await page.evaluate(() => window.protectionAddFixture.seedQuery('unmatched filter'));
      await apply(page);
      assert.equal(await page.locator('#attribute-form').isHidden(), true, '成功且刷新通过后收起添加区');
      assert.equal(await page.locator('#roots-title').evaluate(element => document.activeElement === element), true, '完成后焦点进入记录标题');
      const result = await state(page);
      assert.deepEqual(result.draft, []); assert.deepEqual(result.results, []);
      assert.equal(result.query, 'unmatched filter'); assert.equal(await page.locator('#protection-search').inputValue(), 'unmatched filter');
      assert.equal(await page.locator('#root-list .root-record').count(), 0, '保留用户查询，不能把无匹配冒充未登记');
      assert.equal(await page.locator('#protection-search-form').isVisible(), true);
      assert.equal(await page.locator('#records-feedback').isVisible(), true);
      await page.locator('#protection-search').fill(''); await page.locator('#search-protection').click(); await idle(page);
      assert.equal(await page.locator('#root-list .root-record').count(), 1, '清除查询后显示真实新增登记');
      await add(page, beta);
      assert.equal(await page.locator('#records-feedback').isHidden(), true, '开始新批次清理旧成功反馈');
      assert.equal(await page.locator('#protection-add-results').textContent(), '');
      checks.push({ operation: 'empty-manual-success-new-batch', language, simulated_ipc: true, no_file_action: true, submit_guard: true, filter_preserved_and_recoverable: true, completed_form_collapsed: true, current_feedback_only: true });
    });
    await withPage(language, async page => {
      await configure(page, { failPaths: [beta] });
      await add(page, alpha, 'successful note'); await add(page, beta, 'retained failure note'); await apply(page);
      let result = await state(page);
      assert.equal(await page.locator('#attribute-form').isVisible(), true);
      assert.deepEqual(result.draft, [beta]); assert.deepEqual(result.notes, [[beta.toLowerCase(), 'retained failure note']]);
      assert.equal(result.results.filter(item => item.succeeded).length, 1); assert.equal(result.results.filter(item => !item.succeeded).length, 1);
      assert.equal(await page.locator('#retry-protection-add').isVisible(), true);
      assert.equal(await page.locator('#protection-draft-list input').inputValue(), 'retained failure note');
      await configure(page); await apply(page, true); result = await state(page);
      const operations = result.calls.filter(call => call.command === 'attribute_operation');
      assert.deepEqual(operations.map(call => call.path), [alpha, beta, beta]);
      assert.equal(operations[2].note, 'retained failure note'); assert.equal(result.peakOperations, 1, '批量仍串行');
      assert.equal(await page.locator('#attribute-form').isHidden(), true);
      assert.equal(await page.locator('#root-list .root-record').count(), 2);
      checks.push({ operation: 'partial-failure-retry', language, simulated_ipc: true, no_file_action: true, failed_draft_and_note_retained: true, retry_only_failed: true, serial_operations: true });
    });
    await withPage(language, async page => {
      await configure(page, { failPaths: [alpha, beta] }); await add(page, alpha, 'a'); await add(page, beta, 'b'); await apply(page);
      const result = await state(page);
      assert.deepEqual(result.draft, [alpha, beta]); assert.equal(result.results.every(item => !item.succeeded), true);
      assert.equal(await page.locator('#attribute-form').isVisible(), true); assert.equal(await page.locator('#retry-protection-add').isVisible(), true);
      assert.equal(await page.locator('#root-list .root-record').count(), 0);
      checks.push({ operation: 'all-failed', language, simulated_ipc: true, no_file_action: true, drafts_retained: true, failures_explicit: true });
    });
    await withPage(language, async page => {
      await add(page, alpha); await apply(page);
      await page.evaluate(() => window.protectionAddFixture.seedOldVerification());
      assert.equal(await page.locator('#root-list .root-verification.consistent').count(), 1);
      await add(page, alpha, 'failed attempt note'); await configure(page, { failPaths: [alpha] }); await apply(page);
      const result = await state(page);
      assert.equal(await page.locator('#root-list .root-record').count(), 1, '失败不能抹除已有恢复入口');
      assert.equal(await page.locator('#root-list .root-verification.unchecked').count(), 1, '失败也可能部分变更，须撤销旧核验证明');
      assert.equal(await page.locator('#root-list .root-verification.consistent').count(), 0);
      assert.deepEqual(result.draft, [alpha]); assert.equal(result.results[0].succeeded, false);
      assert.equal(await page.locator('#protection-draft-list input').inputValue(), 'failed attempt note');
      assert.equal(await page.locator('#attribute-form').isVisible(), true);
      checks.push({ operation: 'failed-mutation-invalidates-verification', language, simulated_ipc: true, no_file_action: true, registered_row_retained: true, verification_cleared: true, failed_draft_and_note_retained: true });
    });
    await withPage(language, async page => {
      await configure(page, { failPaths: [alpha, beta] }); await add(page, alpha); await add(page, beta); await apply(page);
      await page.locator('#protection-draft-list .protection-draft-item').first().locator('button').click(); await idle(page);
      assert.deepEqual((await state(page)).draft, [beta]);
      await configure(page); await apply(page, true);
      const operations = (await state(page)).calls.filter(call => call.command === 'attribute_operation');
      assert.deepEqual(operations.map(call => call.path), [alpha, beta, beta], '重试不得重新应用用户已移除的失败项');
      checks.push({ operation: 'removed-failure-not-retried', language, simulated_ipc: true, no_file_action: true, removed_item_not_reintroduced: true });
    });
    await withPage(language, async page => {
      await add(page, alpha); await configure(page, { failRefresh: true }); await apply(page);
      const result = await state(page);
      assert.equal(result.roots.length, 1); assert.equal(result.stale, true); assert.deepEqual(result.draft, []);
      assert.equal(await page.locator('#attribute-form').isHidden(), true, '所有项目成功后收起空表单，刷新失败警告放在列表');
      assert.equal(await page.locator('#apply-protection-draft').isDisabled(), true, '刷新失败后不重复提交已经成功的项目');
      assert.equal(await page.locator('#retry-protection-add').isHidden(), true);
      assert.equal(await page.locator('#records-feedback').isVisible(), true);
      assert.equal((await state(page)).recordNotice.key, 'protectionBatchSummaryRefreshFailed');
      await configure(page); await page.locator('#refresh-records').click(); await idle(page);
      assert.equal(await page.locator('#root-list .root-record').count(), 1);
      assert.equal((await state(page)).calls.filter(call => call.command === 'attribute_operation').length, 1);
      checks.push({ operation: 'operation-success-refresh-failure', language, simulated_ipc: true, no_file_action: true, outcomes_separate: true, no_duplicate_apply: true, refresh_recovers: true });
    });
    await withPage(language, async page => {
      await add(page, alpha); await apply(page); await page.evaluate(() => window.protectionAddFixture.seedOldVerification());
      assert.equal(await page.locator('#root-list .root-verification.consistent').count(), 1);
      await page.locator('#root-list [data-root-action=restore]').click(); await idle(page);
      assert.equal(await page.locator('#root-list .root-record').count(), 0); assert.equal(await page.locator('#records-feedback').isVisible(), true);
      await add(page, alpha, 'new registration');
      assert.equal(await page.locator('#records-feedback').isHidden(), true); assert.deepEqual((await state(page)).rootFeedback, []);
      await page.locator('#attribute-mode').selectOption('locked'); await apply(page);
      assert.equal(await page.locator('#root-list .root-record').count(), 1);
      const restored = await page.evaluate(() => window.protectionAddFixture.message('attributeRestored'));
      assert.equal((await page.locator('#protection-local-panel').textContent()).includes(restored), false, '新登记不能携带旧恢复成功提示');
      assert.equal(await page.locator('#root-list select').inputValue(), 'locked');
      assert.equal(await page.locator('#root-list .root-verification.unchecked').count(), 1, '新变更不能复用上次核验证明');
      assert.equal((await state(page)).rootFeedback.length, 0);
      checks.push({ operation: 'restore-register-same-identity', language, simulated_ipc: true, no_file_action: true, stale_feedback_removed: true, current_mode_shown: true });
    });
    await withPage(language, async page => {
      await configure(page, { holdPreview: true }); await page.locator('#open-attribute-form').click();
      await page.locator('#attribute-path').fill(alpha); await page.locator('#append-protection-path').click();
      await page.waitForFunction(() => window.protectionAddFixture.state().waitingPreview);
      await page.locator('#cancel-attribute-add').click(); await page.evaluate(() => window.protectionAddFixture.releasePreview()); await idle(page);
      assert.equal(await page.locator('#attribute-form').isHidden(), true); assert.deepEqual((await state(page)).draft, []);
      assert.equal(await page.locator('#attribute-path').inputValue(), '');
      checks.push({ operation: 'cancel-preview-race', language, simulated_ipc: true, no_file_action: true, cancelled_draft_not_revived: true });
    });
    await withPage(language, async page => {
      await add(page, alpha); await configure(page, { holdOperation: true }); await page.locator('#apply-protection-draft').click();
      await page.waitForFunction(() => window.protectionAddFixture.state().waitingOperation);
      await page.locator('#protection-service-tab').click();
      await page.evaluate(() => window.protectionAddFixture.releaseOperation()); await idle(page);
      assert.equal(await page.locator('#protection-service-tab').getAttribute('aria-selected'), 'true');
      assert.equal(await page.locator('#protection-service-tab').evaluate(element => document.activeElement === element), true, '晚完成不得抢隐藏本地页焦点');
      checks.push({ operation: 'completion-after-view-change', language, simulated_ipc: true, no_file_action: true, service_view_focus_retained: true });
    });
    await withPage(language, async page => {
      await add(page, alpha); await configure(page, { holdOperation: true }); await page.locator('#apply-protection-draft').click();
      await page.waitForFunction(() => window.protectionAddFixture.state().waitingOperation);
      await page.locator('#protection-service-tab').click(); await page.locator('#protection-local-tab').click();
      assert.equal(await page.locator('#protection-local-tab').evaluate(element => document.activeElement === element), true);
      await page.evaluate(() => window.protectionAddFixture.releaseOperation()); await idle(page);
      assert.equal(await page.locator('#attribute-form').isHidden(), true);
      assert.equal(await page.locator('#protection-local-tab').getAttribute('aria-selected'), 'true');
      assert.equal(await page.locator('#protection-local-tab').evaluate(element => document.activeElement === element), true, '用户主动返回本地页签后，晚完成不得将焦点移到记录标题');
      checks.push({ operation: 'completion-after-return-to-local', language, simulated_ipc: true, no_file_action: true, manually_chosen_tab_focus_retained: true });
    });
  }
  for (const width of [375, 1080, 1692]) for (const scheme of ['light', 'dark']) for (const language of ['zh-CN', 'en']) {
    await withPage(language, async page => {
      await add(page, alpha, 'sample note');
      for (const stage of ['selected', 'failed', 'completed']) {
        if (stage === 'failed') { await configure(page, { failPaths: [alpha] }); await apply(page); }
        if (stage === 'completed') { await configure(page); await apply(page, true); }
        await page.evaluate(() => scrollTo(0, 0));
        const geometry = await page.locator('#protection-local-panel').evaluate(panel => ({
          viewport: innerWidth, page: document.documentElement.scrollWidth,
          outside: [...panel.querySelectorAll('button, input, select, summary, strong')].filter(element => {
            const box = element.getBoundingClientRect();
            return box.width > 0 && box.height > 0 && (box.left < -1 || box.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2);
          }).map(element => element.id || element.className || element.tagName)
        }));
        assert.ok(geometry.page <= geometry.viewport + 1, JSON.stringify({ width, scheme, language, stage, geometry }));
        assert.deepEqual(geometry.outside, [], JSON.stringify({ width, scheme, language, stage, geometry }));
        if (width >= 1080 && stage === 'selected') {
          const source = await page.locator('.protection-add-source').boundingBox(), policy = await page.locator('.protection-add-policy').boundingBox();
          assert.ok(source && policy && Math.abs(source.y - policy.y) < 2 && policy.x > source.x, '宽窗口来源和策略并排');
          const action = await page.locator('#apply-protection-draft').boundingBox();
          assert.ok(action && action.y + action.height <= 820, '宽窗口单项目提交入口首屏可见');
        }
        if (width === 375 && stage === 'selected') {
          const source = await page.locator('.protection-add-source').boundingBox(), policy = await page.locator('.protection-add-policy').boundingBox();
          assert.ok(source && policy && policy.y >= source.y + source.height - 1, '窄窗口自然单列');
        }
        const screenshot = `${width}-${scheme}-${language}-${stage}.png`;
        await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
        checks.push({ operation: 'add-flow-layout', width, scheme, language, stage, screenshot, simulated_ipc: true, no_file_action: true, no_horizontal_overflow: true });
      }
    }, { viewport: { width, height: 820 }, colorScheme: scheme });
  }
  assert.deepEqual(errors, []); assert.deepEqual(blockedRequests, []); passed = true;
} catch (error) { errors.push(String(error)); throw error; }
finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors, blocked_requests: blockedRequests,
    limitations: ['A browser-only in-memory IPC fixture drives production UI handlers. Simulated operation outcomes do not verify native IPC, actual file permissions or service installation.', 'Browser viewport, theme and focus tests do not replace Windows DPI, screen-reader or UAC acceptance.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`Protection add-flow UI checks passed: ${checks.length}`);
