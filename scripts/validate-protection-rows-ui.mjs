import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 只向本机模块响应附加状态访问器，不连接 IPC，不执行或模拟文件操作成功。
const { values } = parseArgs({ options: {
  playwright: { type: 'string' }, browser: { type: 'string' }, report: { type: 'string' },
  url: { type: 'string', default: 'http://127.0.0.1:1420/' }
} });
assert.ok(values.playwright && values.browser && values.report, '需要 --playwright、--browser 和 --report');
const url = new URL(values.url);
assert.ok(url.protocol === 'http:' && url.hostname === '127.0.0.1', '仅检查本机开发服务');
const reportPath = resolve(values.report), screenshotRoot = join(dirname(reportPath), 'screenshots');
await mkdir(dirname(reportPath), { recursive: true });
const reportFile = await open(reportPath, 'wx');
await mkdir(screenshotRoot);
const version = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')).version;
const { chromium } = await import(pathToFileURL(resolve(values.playwright)).href);
const browser = await chromium.launch({ executablePath: resolve(values.browser), headless: true, args: ['--no-proxy-server'] });
const checks = [], errors = [], externalRequests = [];
let passed = false;

const accessors = `
window.protectionRowsFixture = {
  records: (roots, stale = false) => {
    rootNoteEditorStates.clear(); rootNoteDrafts.clear(); pendingRootModes.clear(); rootFeedback.clear();
    selectedProtection.clear(); recordState.loading = false; recordState.stale = stale;
    recordState.snapshot = { roots, total: roots.length, overall_total: roots.length, offset: 0 };
    renderRecords();
  },
  redraw: () => renderRecords(),
  stale: value => { recordState.stale = value; renderRecords(); },
  busy: value => { uiBusy = value; renderRecords(); },
  state: () => ({ notes: [...rootNoteDrafts], pending: [...pendingRootModes], editors: [...rootNoteEditorStates] }),
  confirmation: () => {
    aclRestoreReviews.set('row-0', 'a'.repeat(64)); renderRecords();
  },
  message: key => t(key)
};
`;
async function setup(context) {
  await context.route('**/*', async route => {
    const target = new URL(route.request().url());
    if (target.origin !== url.origin) { externalRequests.push(route.request().url()); await route.abort(); return; }
    if (target.pathname === '/ui/main.ts') {
      const response = await route.fetch();
      await route.fulfill({ response, body: await response.text() + accessors });
    } else await route.continue();
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url.href, { waitUntil: 'domcontentloaded' });
  await page.locator('#nav-attributes').click();
  return page;
}
function roots(count, special = false) {
  return Array.from({ length: count }, (_, index) => ({
    id: `row-${index}`, name: index === 0 ? 'api.txt' : index === 1 ? 'fold测试.txt' : `Document-${index + 1}.txt`,
    mode: ['locked', 'readonly-acl', 'hidden-locked'][index % 3], note: index % 2 ? 'Work item / 工作资料' : ''
  })).map((root, index) => special && index === 0
    ? { ...root, name: '中文最外层名称 / Long outer name '.repeat(15), duplicate_name: true, note: '区分备注 / distinguishing note' }
    : root);
}
async function seed(page, count, special = false) {
  await page.evaluate(roots => window.protectionRowsFixture.records(roots), roots(count, special));
}
async function state(page) { return page.evaluate(() => window.protectionRowsFixture.state()); }
async function redraw(page) { await page.evaluate(() => window.protectionRowsFixture.redraw()); }
async function geometry(page) {
  return page.locator('#protection-records').evaluate(card => {
    const visible = element => !element.closest('.sr-only, [hidden]') && element.getClientRects().length && getComputedStyle(element).visibility === 'visible';
    const context = document.createElement('canvas').getContext('2d');
    return {
      viewport: innerWidth, pageWidth: document.documentElement.scrollWidth,
      rows: [...card.querySelectorAll('.root-record')].map(row => ({ height: row.getBoundingClientRect().height, width: row.getBoundingClientRect().width })),
      outside: [...card.querySelectorAll('button, input, select, p, span')].filter(element => {
        if (!visible(element)) return false;
        const box = element.getBoundingClientRect();
        return box.left < -1 || box.right > innerWidth + 1 || (element.scrollWidth > element.clientWidth + 2 && !element.matches('.root-name'));
      }).map(element => element.id || element.className),
      smallControls: [...card.querySelectorAll('button, select, input:not([type=checkbox])')].filter(element => visible(element) && element.getBoundingClientRect().height < 43).map(element => element.dataset.rootAction || element.id),
      clippedModes: [...card.querySelectorAll('.root-policy select')].filter(select => {
        if (!visible(select)) return false;
        const style = getComputedStyle(select);
        context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
        const textWidth = context.measureText(select.selectedOptions[0]?.textContent ?? '').width;
        return textWidth + parseFloat(style.paddingLeft) + parseFloat(style.paddingRight) + 24 > select.clientWidth;
      }).map(select => select.selectedOptions[0]?.textContent)
    };
  });
}
function checkGeometry(result, metadata) {
  assert.ok(result.pageWidth <= result.viewport + 1, JSON.stringify({ ...metadata, result }));
  assert.deepEqual(result.outside, [], JSON.stringify({ ...metadata, result }));
  assert.deepEqual(result.smallControls, [], JSON.stringify({ ...metadata, result }));
  assert.deepEqual(result.clippedModes, [], JSON.stringify({ ...metadata, result }));
}

try {
  for (const language of ['zh-CN', 'en']) {
    const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: language });
    await context.addInitScript(language => localStorage.setItem('foldsentinel.language', language), language);
    try {
      const page = await setup(context);
      await seed(page, 2);
      let row = page.locator('#root-list .root-record').first();
      assert.equal(await row.locator('.root-policy select[data-root-action=mode]').count(), 1);
      assert.equal(await row.locator('.root-policy select').isVisible(), true);
      assert.equal(await row.locator('.root-mode-value').count(), 0, '可编辑记录不再重复显示静态登记方式');
      assert.equal(await row.locator('.root-editor, [data-root-action=edit-protection], [data-root-action=apply]').count(), 0, '不再提供调整面板或二次应用入口');
      assert.equal(await row.locator('[data-root-action=verify]').isVisible(), true);
      assert.equal(await row.locator('[data-root-action=restore]').isVisible(), true);
      assert.ok(await row.locator('select').getAttribute('aria-label'), '登记方式控件具有可访问名称');
      await row.locator('select').focus();
      await row.locator('select').selectOption('hidden');
      assert.equal(await row.locator('.root-feedback.is-error').textContent(), await page.evaluate(() => window.protectionRowsFixture.message('desktopRequired')), '选择方式直接进入真实处理器；浏览器明确拒绝原生操作');
      assert.equal(await row.locator('select').evaluate(element => document.activeElement === element), true, '预览拒绝并重绘后恢复登记方式焦点');
      assert.deepEqual((await state(page)).pending, [['row-0', 'hidden']]);
      await redraw(page);
      assert.equal(await row.locator('select').inputValue(), 'hidden');
      assert.equal(await row.locator('select').isVisible(), true);
      await row.locator('select').focus();
      await row.locator('select').selectOption('locked');
      assert.equal(await row.locator('select').evaluate(element => document.activeElement === element), true);
      await row.locator('select').selectOption('hidden');
      assert.deepEqual((await state(page)).pending, [['row-0', 'hidden']]);
      assert.equal(await row.locator('.root-feedback.is-error').textContent(), await page.evaluate(() => window.protectionRowsFixture.message('desktopRequired')));
      assert.equal(await row.locator('select').inputValue(), 'hidden');
      assert.equal(await row.locator('select').evaluate(element => document.activeElement === element), true, '切换重试后继续保留方式控件焦点');
      checks.push({ operation: 'direct-mode-change', language, one_visible_mode_control: true, no_adjust_or_apply_entry: true, real_handler_refuses_preview_mutation: true, preview_redraw_restores_mode_focus: true, pending_survives_redraw: true, failed_selection_can_be_retried: true, no_file_action: true });

      await row.locator('[data-root-action=edit-note]').click();
      const note = row.locator('.root-note-editor input');
      await note.fill('a'.repeat(81));
      assert.equal(await row.locator('[data-root-action=save-note]').isDisabled(), true);
      await note.fill('😀'.repeat(80));
      assert.equal(await row.locator('[data-root-action=save-note]').isEnabled(), true);
      await note.fill('未保存的备注 / unsaved note'); await redraw(page);
      assert.equal(await note.inputValue(), '未保存的备注 / unsaved note');
      assert.equal(await note.isVisible(), true);
      await note.focus(); await page.keyboard.press('Escape');
      assert.equal(await note.isHidden(), true);
      assert.equal(await row.locator('[data-root-action=edit-note]').evaluate(element => document.activeElement === element), true);
      assert.equal((await state(page)).notes[0][1], '未保存的备注 / unsaved note');
      await redraw(page);
      assert.equal(await note.isHidden(), true, 'Escape 收起后重绘不能因草稿而自动重开');
      assert.equal(await row.locator('select').isVisible(), true);
      await row.locator('[data-root-action=edit-note]').click();
      assert.equal(await note.inputValue(), '未保存的备注 / unsaved note');
      await row.locator('[data-root-action=save-note]').click();
      assert.equal((await state(page)).notes[0][1], '未保存的备注 / unsaved note', '浏览器预览拒绝保存时继续保留草稿');
      await row.locator('[data-root-action=cancel-note]').click();
      assert.deepEqual((await state(page)).notes, []);
      checks.push({ operation: 'note-draft-and-validation', language, unicode_limit: true, redraw_preserves_draft: true, escape_collapses_without_losing_draft: true, collapsed_state_survives_redraw: true, explicit_cancel_clears_draft: true, preview_refuses_native_save: true, no_file_action: true });

      await seed(page, 2, true); row = page.locator('#root-list .root-record').first();
      await page.evaluate(() => window.protectionRowsFixture.stale(true));
      assert.equal(await row.locator('[data-root-action=restore]').isDisabled(), true);
      assert.equal(await row.locator('[data-root-action=verify]').isDisabled(), true);
      await row.locator('[data-root-action=expand-name]').click();
      assert.equal(await row.locator('[data-root-action=expand-name]').getAttribute('aria-expanded'), 'true');
      assert.equal(await row.locator('select').isDisabled(), true);
      await page.evaluate(() => window.protectionRowsFixture.stale(false));
      await page.evaluate(() => window.protectionRowsFixture.busy(true));
      assert.equal(await row.locator('select').isDisabled(), true);
      assert.equal(await row.locator('[data-root-action=restore]').isDisabled(), true);
      assert.equal(await row.locator('[data-root-action=verify]').isDisabled(), true);
      await page.evaluate(() => window.protectionRowsFixture.busy(false));
      assert.equal(await row.locator('select').isEnabled(), true);
      await page.evaluate(() => window.protectionRowsFixture.confirmation());
      assert.equal(await row.locator('.root-acl-confirm').isVisible(), true);
      await row.locator('[data-root-action=cancel-acl-restore]').click();
      assert.equal(await row.locator('.root-acl-confirm').count(), 0);
      assert.equal(await row.locator('[data-root-action=restore]').evaluate(element => document.activeElement === element), true);
      checks.push({ operation: 'busy-stale-and-restore-confirmation', language, busy_and_stale_block_mutation: true, stale_name_disclosure_available: true, restore_cancel_returns_focus: true, no_file_action: true });
    } finally { await context.close(); }
  }

  for (const width of [375, 720, 1080, 1692]) for (const scheme of ['light', 'dark']) for (const language of ['zh-CN', 'en']) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, colorScheme: scheme, locale: language });
    await context.addInitScript(({ language, scheme }) => { localStorage.setItem('foldsentinel.language', language); localStorage.setItem('foldsentinel.theme', scheme); }, { language, scheme });
    try {
      const page = await setup(context);
      for (const count of [2, 10, 50]) {
        await seed(page, count);
        assert.equal(await page.locator('#root-list .root-record').count(), count);
        assert.equal(await page.locator('#root-list .root-policy select:visible').count(), count, '每项直接显示唯一登记方式控件');
        assert.equal(await page.locator('#root-list .root-editor, #root-list [data-root-action=edit-protection], #root-list [data-root-action=apply]').count(), 0);
        assert.equal(await page.locator('#root-list .root-note-editor:visible').count(), 0, '备注默认收起以保持紧凑');
        assert.equal(await page.locator('#root-list [data-root-action=verify]:visible').count(), count);
        assert.equal(await page.locator('#root-list [data-root-action=restore]:visible').count(), count);
        const result = await geometry(page); checkGeometry(result, { width, scheme, language, count });
        const maxHeight = Math.max(...result.rows.map(row => row.height));
        assert.ok(maxHeight <= (width < 600 ? (language === 'en' ? 260 : 220) : 140), JSON.stringify({ width, scheme, language, count, maxHeight }));
        await page.locator('#protection-records').scrollIntoViewIfNeeded();
        const screenshot = `${width}-${scheme}-${language}-${count}-rows.png`;
        await page.screenshot({ path: join(screenshotRoot, screenshot) });
        checks.push({ operation: 'compact-many-rows', width, scheme, language, count, max_row_height: maxHeight, mode_controls_visible: true, notes_collapsed: true, selected_mode_text_readable: true, primary_actions_available: true, screenshot, no_file_action: true });
      }
      await seed(page, 2, true);
      await page.locator('#root-list [data-root-action=edit-note]').first().click();
      await page.locator('#root-list .root-note-editor input').first().fill('待保存 / still editing');
      const result = await geometry(page); checkGeometry(result, { width, scheme, language, stage: 'expanded' });
      const screenshot = `${width}-${scheme}-${language}-expanded.png`;
      await page.locator('#protection-records').scrollIntoViewIfNeeded();
      await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
      checks.push({ operation: 'expanded-note-editor-layout', width, scheme, language, long_name_and_duplicate_note: true, no_horizontal_overflow: true, selected_mode_text_readable: true, screenshot, no_file_action: true });
    } finally { await context.close(); }
  }
  assert.deepEqual(errors, []); assert.deepEqual(externalRequests, []); passed = true;
} catch (error) { errors.push(String(error)); throw error; }
finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors,
    limitations: ['Browser-only snapshots and production handlers. No native IPC, file operation, service or registry changes.', 'Desktop geometry and screenshots do not replace native Windows DPI or screen reader validation.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`Protection row UI checks passed: ${checks.length}`);
