import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 夹具只切换登记快照与组件展示状态，保留生产渲染和事件；不连接 IPC 或执行文件操作。
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
const checks = [], errors = [], requests = [];
let passed = false;

async function setup(context) {
  await context.route('**/*', async route => {
    const requestUrl = new URL(route.request().url());
    if (requestUrl.origin !== url.origin) { requests.push(route.request().url()); await route.abort(); return; }
    if (requestUrl.pathname === '/ui/main.ts') {
      const response = await route.fetch();
      // 仅测试响应附加访问器，生产模块不导出测试 API，也不替换现有处理器。
      await route.fulfill({ response, body: await response.text() + `
window.protectionLayoutFixture = {
  message: key => t(key),
  records: ({ snapshot = null, query = '', search = query, stale = false, loading = false } = {}) => {
    recordState.snapshot = snapshot; recordState.stale = stale; recordState.loading = loading;
    recordState.error = stale ? new Error('Explicit read failure fixture') : null;
    recordsQuery = query; input('protection-search').value = search;
    renderRecords();
  },
  rejectRefresh: async () => {
    try { await recordState.refresh(() => Promise.reject(new Error('Explicit read failure fixture'))); }
    catch (error) { if (error.message !== 'Explicit read failure fixture') throw error; }
    renderRecords();
  },
  selection: () => [...selectedProtection],
  servicePresentation: () => {
    managedUi.available = true; managedUi.stale = false;
    managedUi.snapshot = { registered: 1, roots: [{ id: 'layout-service', name: '服务展示夹具', note: '', mode: 'locked', password_required: true, monitoring: true, unlocked_until: null, restoring: false, last_error: null, entries: 1 }] };
    managedUi.render();
  },
  serviceVisibility: () => managedUi.visible,
  serviceEditorOpen: () => managedUi.editor !== null
};
` });
      return;
    }
    await route.continue();
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url.href, { waitUntil: 'domcontentloaded' });
  await page.locator('#nav-attributes').click();
  return page;
}

const empty = { roots: [], total: 0, overall_total: 0, offset: 0 };
const populated = { roots: [
  { id: 'layout-one', name: '项目资料', note: '本地登记夹具', mode: 'hidden-locked', entry_count: 7 },
  { id: 'layout-two', name: 'Long project name / '.repeat(9), note: '同名备注', mode: 'locked', duplicate_name: true, entry_count: 2 }
], total: 2, overall_total: 2, offset: 0 };
async function records(page, state) { await page.evaluate(state => window.protectionLayoutFixture.records(state), state); }
async function text(page, key) { return page.evaluate(key => window.protectionLayoutFixture.message(key), key); }
async function assertLocalSelected(page) {
  assert.equal(await page.locator('#protection-local-tab').getAttribute('aria-selected'), 'true');
  assert.equal(await page.locator('#protection-local-tab').getAttribute('tabindex'), '0');
  assert.equal(await page.locator('#protection-service-tab').getAttribute('aria-selected'), 'false');
  assert.equal(await page.locator('#protection-service-tab').getAttribute('tabindex'), '-1');
  assert.equal(await page.locator('#protection-local-panel').isVisible(), true);
  assert.equal(await page.locator('#managed-service-panel').isHidden(), true);
  assert.equal(await page.evaluate(() => window.protectionLayoutFixture.serviceVisibility()), false);
}
async function dimensions(page, metadata) {
  const result = await page.locator('#protection-local-panel').evaluate(panel => ({
    viewport: innerWidth, page: document.documentElement.scrollWidth,
    outside: [...panel.querySelectorAll('p, button, input, select, summary, h2, strong')].filter(element => {
      const box = element.getBoundingClientRect();
      return box.width > 0 && box.height > 0 && (box.left < -1 || box.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2);
    }).map(element => element.id || element.className || element.tagName)
  }));
  assert.ok(result.page <= result.viewport + 1, JSON.stringify({ ...metadata, result }));
  assert.deepEqual(result.outside, [], JSON.stringify({ ...metadata, result }));
}

try {
  const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const page = await setup(context);
    await assertLocalSelected(page);
    for (const [tabId, panelId] of [['protection-local-tab', 'protection-local-panel'], ['protection-service-tab', 'managed-service-panel']]) {
      assert.equal(await page.locator(`#${tabId}`).getAttribute('role'), 'tab');
      assert.equal(await page.locator(`#${tabId}`).getAttribute('aria-controls'), panelId);
      assert.equal(await page.locator(`#${panelId}`).getAttribute('role'), 'tabpanel');
      assert.equal(await page.locator(`#${panelId}`).getAttribute('aria-labelledby'), tabId);
    }
    assert.equal(await page.locator('[role=tablist]').count(), 1);
    await page.locator('#protection-local-tab').focus();
    await page.keyboard.press('ArrowRight');
    assert.equal(await page.locator('#protection-service-tab').evaluate(tab => document.activeElement === tab), true);
    assert.equal(await page.locator('#managed-service-panel').isVisible(), true);
    assert.equal(await page.evaluate(() => window.protectionLayoutFixture.serviceVisibility()), true);
    await page.keyboard.press('ArrowRight'); await assertLocalSelected(page);
    await page.keyboard.press('End');
    assert.equal(await page.locator('#protection-service-tab').getAttribute('aria-selected'), 'true');
    await page.keyboard.press('Home'); await assertLocalSelected(page);
    await page.keyboard.press('ArrowLeft');
    assert.equal(await page.locator('#protection-service-tab').getAttribute('aria-selected'), 'true');
    await page.locator('#protection-local-tab').click();
    checks.push({ operation: 'accessible-protection-tabs', aria_associations: true, roving_tabindex: true, arrow_wrap_and_home_end: true, inactive_service_not_visible: true });

    await records(page, {});
    assert.equal(await page.locator('#protection-search-form').isVisible(), true, '未读取状态保留搜索');
    assert.equal(await page.locator('#root-list .protection-empty').count(), 0, '未知状态不能声称没有登记');
    await page.evaluate(() => window.protectionLayoutFixture.rejectRefresh());
    assert.equal(await page.locator('#records-feedback').isVisible(), true);
    assert.equal(await page.locator('#root-list .protection-empty').count(), 0);
    assert.doesNotMatch(await page.locator('#root-list').textContent(), new RegExp(await text(page, 'emptyRoots')));
    checks.push({ operation: 'unknown-and-failed-read', unknown_not_empty: true, failure_exposed: true });

    await records(page, { snapshot: empty });
    assert.equal(await page.locator('#root-list .protection-empty strong').textContent(), await text(page, 'emptyRoots'));
    assert.equal(await page.locator('#root-list .protection-empty p').textContent(), await text(page, 'protectionEmptyHelp'));
    for (const id of ['protection-search-form', 'root-summary', 'protection-bulk-toolbar', 'roots-pagination', 'protection-columns', 'protection-verification-help']) {
      assert.equal(await page.locator(`#${id}`).isHidden(), true, `真空态隐藏 ${id}`);
    }
    assert.equal(await page.locator('#refresh-records').isVisible(), true, '空态仍可刷新');
    assert.equal(await page.locator('#refresh-records').evaluate(button => !!button.closest('#protection-records .records-heading')), true);
    for (const id of ['pick-attribute-file', 'pick-attribute-folder', 'open-attribute-form']) {
      assert.equal(await page.locator(`#${id}`).evaluate(button => !!button.closest('.protection-toolbar')), true, `添加入口 ${id} 保留在顶部工具栏`);
    }
    assert.equal(await page.locator('#protection-search-form').evaluate(form => !!form.closest('#protection-records .records-heading')), true);
    checks.push({ operation: 'genuine-empty', concise_title_and_help: true, search_summary_bulk_pagination_hidden: true, refresh_owned_by_records: true });

    await records(page, { snapshot: empty, loading: true });
    assert.equal(await page.locator('#root-list .protection-empty').count(), 0, '读取中不能显示已确认空态');
    assert.equal(await page.locator('#root-list').getAttribute('aria-busy'), 'true');
    await records(page, { snapshot: empty });
    await page.evaluate(() => window.protectionLayoutFixture.rejectRefresh());
    assert.equal(await page.locator('#root-list .protection-empty').count(), 0, '上次无数据且刷新失败不能仍显示已确认空态');
    assert.equal(await page.locator('#records-feedback').isVisible(), true);
    checks.push({ operation: 'empty-to-loading-or-failure', no_unverified_empty_claim: true, busy_and_error_visible: true });

    await records(page, { snapshot: { ...empty, overall_total: 2 }, query: 'missing' });
    assert.equal(await page.locator('#protection-search-form').isVisible(), true);
    assert.equal(await page.locator('#protection-search').inputValue(), 'missing');
    assert.equal(await page.locator('#root-list').textContent(), await text(page, 'noProtectionMatches'));
    await page.locator('#protection-search').fill('');
    await page.locator('#protection-search-form').evaluate(form => form.requestSubmit());
    assert.equal(await page.locator('#protection-search').inputValue(), '', '搜索无匹配后仍可清空提交');
    await records(page, { snapshot: empty, search: '尚未提交的搜索' });
    assert.equal(await page.locator('#protection-search-form').isVisible(), true, '未提交搜索不能因空快照隐藏');
    assert.equal(await page.locator('#protection-search').inputValue(), '尚未提交的搜索');
    checks.push({ operation: 'filtered-empty-and-search-draft', recovery_search_visible: true, unsubmitted_search_preserved: true });

    await records(page, { snapshot: populated });
    assert.equal(await page.locator('#root-list .root-record').count(), 2);
    await page.locator('#select-protection-page').check();
    assert.deepEqual(await page.evaluate(() => window.protectionLayoutFixture.selection()), ['layout-one', 'layout-two']);
    assert.equal(await page.locator('#verify-selected-protection').isVisible(), true);
    await page.locator('#restore-selected-protection').click();
    assert.equal(await page.locator('#protection-restore-confirm').isVisible(), true);
    assert.equal(await page.locator('#protection-restore-items li').count(), 2);
    await page.locator('#cancel-selected-restore').click();
    assert.equal(await page.locator('#protection-restore-confirm').isHidden(), true);
    await page.evaluate(() => window.protectionLayoutFixture.rejectRefresh());
    assert.equal(await page.locator('#root-list .root-record').count(), 2, '读取失败保留上次记录');
    assert.equal(await page.locator('#root-list [data-root-action=restore]').first().isDisabled(), true);
    checks.push({ operation: 'registered-records-and-selection', production_selection_handler: true, restore_confirmation_and_cancel: true, stale_records_retained_and_actions_disabled: true, no_file_action_performed: true });

    await records(page, { snapshot: { ...populated, total: 52, overall_total: 52 } });
    assert.equal(await page.locator('#roots-pagination').isVisible(), true);
    assert.equal(await page.locator('#roots-previous').isDisabled(), true);
    assert.equal(await page.locator('#roots-next').isEnabled(), true);
    await page.locator('#open-attribute-form').click();
    assert.equal(await page.locator('#attribute-form').isVisible(), true);
    assert.equal(await page.locator('#attribute-path').evaluate(field => document.activeElement === field), true);
    await page.locator('#attribute-path').fill('D:\\layout-fixture\\unsubmitted.txt');
    await page.locator('#protection-service-tab').click();
    await page.evaluate(() => window.protectionLayoutFixture.servicePresentation());
    await page.locator('#managed-root-list [data-managed-action=unlock]').click();
    await page.locator('#managed-current-password').fill('UI-only-public-password');
    await page.locator('#protection-local-tab').click();
    assert.equal(await page.evaluate(() => window.protectionLayoutFixture.serviceEditorOpen()), false);
    assert.equal(await page.locator('#managed-service-panel input[type=password]').count(), 0, '离开服务视图清除口令编辑器');
    assert.equal(await page.locator('#attribute-path').inputValue(), 'D:\\layout-fixture\\unsubmitted.txt', '切换保护视图保留本地路径草稿');
    await page.locator('#nav-settings').click(); await page.locator('#nav-attributes').click();
    await assertLocalSelected(page);
    await page.locator('#cancel-attribute-add').click();
    assert.equal(await page.locator('#attribute-path').inputValue(), '');
    checks.push({ operation: 'drafts-pagination-and-sensitive-editor', pagination_retained: true, manual_entry_focus: true, local_draft_preserved_between_views: true, service_password_editor_cleared: true, cancellation_clears_path: true });
  } finally { await context.close(); }

  for (const width of [375, 720, 1080]) for (const scheme of ['light', 'dark']) for (const locale of ['zh-CN', 'en']) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, colorScheme: scheme, locale });
    try {
      const page = await setup(context);
      await page.locator('#nav-settings').click();
      await page.locator('#language').selectOption(locale); await page.locator('#theme').selectOption(scheme);
      await page.locator('#nav-attributes').click();
      for (const state of ['empty', 'filtered-empty', 'registered', 'expanded-help']) {
        const snapshot = state === 'empty' || state === 'expanded-help' ? empty : state === 'filtered-empty' ? { ...empty, overall_total: 2 } : populated;
        await records(page, { snapshot, query: state === 'filtered-empty' ? 'missing' : '' });
        assert.equal(await page.locator('.protection-description details').count(), 2, '保留备份及保护说明两个可展开入口');
        if (state === 'expanded-help') await page.locator('.protection-description details').evaluateAll(details => details.forEach(detail => { detail.open = true; }));
        const order = await page.locator('#protection-records').evaluate(section => ({
          afterToolbar: !!(document.querySelector('.protection-toolbar').compareDocumentPosition(section) & Node.DOCUMENT_POSITION_FOLLOWING),
          beforeHelp: [...document.querySelectorAll('.protection-description details')].every(detail => !!(section.compareDocumentPosition(detail) & Node.DOCUMENT_POSITION_FOLLOWING))
        }));
        assert.deepEqual(order, { afterToolbar: true, beforeHelp: true }, '任务列表在添加入口之后、说明和备份之前');
        await page.evaluate(() => scrollTo(0, 0));
        if (width === 1080) {
          const box = await page.locator('#protection-records').boundingBox();
          assert.ok(box && box.y >= 0 && box.y < 820, `默认窗口列表进入首屏: ${JSON.stringify({ width, scheme, locale, state, box })}`);
          if (state === 'registered') {
            const first = await page.locator('#root-list .root-record').first().boundingBox();
            assert.ok(first && first.y < 820, '已登记列表首行进入首屏；长名称和多条记录允许纵向滚动');
          } else assert.ok(box.y + box.height <= 820, `默认窗口空态卡片首屏完整可见: ${JSON.stringify({ width, scheme, locale, state, box })}`);
        }
        await dimensions(page, { width, scheme, locale, state });
        const screenshot = `${width}-${scheme}-${locale}-${state}.png`;
        await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
        checks.push({ operation: 'layout', width, scheme, locale, state, screenshot, list_before_help: true, no_horizontal_overflow: true });
      }
    } finally { await context.close(); }
  }
  assert.deepEqual(requests, []); assert.deepEqual(errors, []); passed = true;
} catch (error) { errors.push(String(error)); throw error; }
finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors, blocked_requests: requests,
    limitations: ['Synthetic registration and service presentation snapshots exercise production rendering and UI handlers only; no native IPC, file action or actual service installation is performed.', 'Browser viewport and themes do not verify native Windows DPI or real service identity.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`Protection layout UI checks passed: ${checks.length}`);
