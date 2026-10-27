import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { resolve, dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

// 使用已有 Playwright 和浏览器，不自动安装依赖或修改系统缩放。
const { values } = parseArgs({ options: {
  playwright: { type: 'string' }, browser: { type: 'string' }, report: { type: 'string' },
  url: { type: 'string', default: 'http://127.0.0.1:1420/' }
} });
assert.ok(values.playwright && values.browser && values.report, '需要 --playwright、--browser 和 --report');
const url = new URL(values.url);
assert.ok(url.protocol === 'http:' && url.hostname === '127.0.0.1', '仅测试本机开发服务');
const reportPath = resolve(values.report);
const version = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')).version;
const reportFile = await open(reportPath, 'wx');
const screenshotRoot = join(dirname(reportPath), 'screenshots');
await mkdir(screenshotRoot);
const { chromium } = await import(pathToFileURL(resolve(values.playwright)).href);
const browser = await chromium.launch({ executablePath: resolve(values.browser), headless: true, args: ['--no-proxy-server'] });
const checks = [];
const errors = [];
let failed = false;
try {
  for (const [width, scale] of [[1080, 1], [720, 1], [720, 1.25], [720, 1.5], [576, 1.25], [480, 1.5]]) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, deviceScaleFactor: scale, locale: 'en-US' });
    try {
      // 测试不访问外站，也不接入桌面 IPC 或模拟文件操作成功。
      await context.route('**/*', route => new URL(route.request().url()).origin === url.origin ? route.continue() : route.abort());
      const page = await context.newPage();
      page.on('pageerror', error => errors.push(error.message));
      // Vite 的 HMR 长连接不作为界面就绪条件；等待实际控件可操作。
      await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: 30000 });
      await page.locator('#language').selectOption('en');
      await page.locator('#manual-sources > summary').click();
      for (const operation of ['encrypt', 'decrypt', 'verify']) {
        await page.locator(`#${operation}-tab`).click();
        const source = `D:\\test\\${'中文长路径'.repeat(18)}\\alpha.txt\nD:\\test\\资料\\beta.bin`;
        await page.locator('#input-path').fill(source);
        await page.locator('#password').fill('public-layout-test-only');
        if (operation === 'encrypt') await page.locator('#confirm-password').fill('public-layout-test-only');
        for (const language of ['zh-CN', 'en']) {
          await page.locator('#language').selectOption(language);
          assert.equal(await page.locator('html').getAttribute('lang'), language);
          assert.equal(await page.locator('#input-path').inputValue(), source);
          assert.equal(await page.locator('#password').inputValue(), 'public-layout-test-only');
          if (operation === 'encrypt') assert.equal(await page.locator('#confirm-password').inputValue(), 'public-layout-test-only');
          assert.equal(await page.locator('#source-list > li').count(), 2);
          const layout = await page.evaluate(() => {
            const outside = [...document.querySelectorAll('button, input, select, textarea, summary')].filter(element => {
              const box = element.getBoundingClientRect();
              return box.width > 0 && box.height > 0 && (box.left < -1 || box.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2);
            }).map(element => element.id || element.tagName);
            return { width: innerWidth, documentWidth: document.documentElement.scrollWidth, outside };
          });
          assert.ok(layout.documentWidth <= layout.width + 1, JSON.stringify({ width, scale, language, layout }));
          assert.deepEqual(layout.outside, [], JSON.stringify({ width, scale, language, layout }));
          const screenshot = `ui-${width}-${scale}-${operation}-${language}.png`;
          await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
          checks.push({ css_width: width, device_scale_factor: scale, operation, language, no_horizontal_overflow: true, sources_and_password_preserved: true, screenshot });
        }
      }
      await page.locator('#nav-attributes').click();
      for (const language of ['zh-CN', 'en']) {
        await page.locator('#language').selectOption(language);
        await page.evaluate(() => scrollTo(0, 0));
        assert.equal(await page.locator('#attribute-form').isVisible(), false, '添加表单默认收起');
        const addEntry = await page.locator('#pick-attribute-file').boundingBox();
        assert.ok(addEntry && addEntry.y >= 0 && addEntry.y + addEntry.height <= 820, '添加入口在首屏可见');
        await page.locator('#open-attribute-form').click();
        assert.equal(await page.locator('#attribute-form').isVisible(), true);
        await page.locator('#attribute-path').fill('D:\\private-parent\\private-child.txt');
        await page.locator('#cancel-attribute-add').click();
        assert.equal(await page.locator('#attribute-form').isVisible(), false, '取消添加关闭面板');
        assert.equal(await page.locator('#attribute-path').inputValue(), '', '取消添加清空新选择路径');
        // 合成数据只用于真实组件的排版检查，不接入 IPC 或模拟成功属性操作。
        await page.evaluate(async () => {
          const { createRootRecord, syncRootControls } = await import('/ui/attributes.ts');
          const roots = document.querySelector('#root-list');
          // 故意附加旧字段，确保真实渲染组件也不把它们写进文本、title、aria 或表单。
          const record = { id: 'synthetic-root', name: '中文最外层名称'.repeat(18), mode: 'hidden-locked', note: '工作资料', duplicate_name: true, path: 'D:\\private-parent\\private-child.txt', entry_count: 54 };
          window.selectedProtectionMode = null;
          window.savedProtectionNote = null;
          roots.replaceChildren(
            createRootRecord(record, () => {}, mode => { window.selectedProtectionMode = mode; }, {
              verify: () => {}, saveNote: note => { window.savedProtectionNote = note; },
              feedback: { text: '布局反馈 / Layout feedback' },
              verification: { state: 'consistent', checked_at: 1793095200000 }
            }),
            createRootRecord({ id: 'second-id', name: '<img src=x onerror=alert(1)>.txt', mode: 'hidden-readonly' }, () => {}, () => {}),
            createRootRecord({ id: 'mixed-id', name: '混合方式', mode: null }, () => {}, mode => { window.selectedProtectionMode = mode; })
          );
          syncRootControls(roots, false, false);
        });
        assert.deepEqual(await page.locator('#attribute-mode option').evaluateAll(options => options.map(option => option.value)), ['none', 'hidden', 'locked', 'readonly', 'readonly-acl', 'hidden-locked']);
        const first = page.locator('#root-list .root-record').first();
        const mixed = page.locator('#root-list .root-record').nth(2);
        await first.locator('[data-root-action="expand-name"]').click();
        assert.equal(await first.locator('[data-root-action="expand-name"]').getAttribute('aria-expanded'), 'true');
        await first.locator('[data-root-action="expand-name"]').click();
        assert.equal(await mixed.locator('[data-root-action="apply"]').isDisabled(), true, '混合方式必须先选择具体模式');
        await mixed.locator('select').selectOption('hidden');
        assert.equal(await mixed.locator('.root-mode-pending').isVisible(), true, '修改后显示未应用');
        assert.equal(await mixed.locator('[data-root-action="apply"]').isEnabled(), true);
        await first.locator('select').selectOption('locked');
        await first.locator('[data-root-action="apply"]').click();
        assert.equal(await page.evaluate(() => window.selectedProtectionMode), 'locked');
        assert.equal(await first.locator('[data-root-action="apply"]').evaluate(button => document.activeElement === button), true, '行操作回调不应抢焦点');
        await first.locator('[data-root-action="edit-note"]').click();
        const note = first.locator('.root-note-editor input');
        await note.fill('a'.repeat(81));
        assert.equal(await first.locator('[data-root-action="save-note"]').isDisabled(), true, '按Unicode字符限制备注长度');
        await note.fill('😀'.repeat(80));
        assert.equal(await first.locator('[data-root-action="save-note"]').isEnabled(), true, '80个Unicode字符可保存，不能按UTF-16长度误拒绝');
        await note.fill('同名项目备注');
        await first.locator('[data-root-action="save-note"]').click();
        assert.equal(await page.evaluate(() => window.savedProtectionNote), '同名项目备注');
        await first.locator('[data-root-action="cancel-note"]').click();
        await page.evaluate(async () => {
          const { createRootRecord, syncRootControls } = await import('/ui/attributes.ts');
          let draft = '保存失败后仍保留的备注';
          const row = createRootRecord({ id: 'draft-id', name: '资料', mode: 'hidden', note: '旧备注' }, () => {}, () => {}, {
            saveNote: () => {}, noteDraft: draft, onNoteDraft: value => { draft = value; }
          });
          document.querySelector('#root-list').append(row);
          const input = row.querySelector('.root-note-editor input');
          if (row.querySelector('.root-note-editor').hidden || input.value !== draft) throw new Error('重建记录时必须恢复未保存备注');
          syncRootControls(row, true, false); syncRootControls(row, false, true); syncRootControls(row, false, false);
          if (input.value !== draft) throw new Error('忙碌或刷新失败不能清空备注草稿');
          row.querySelector('[data-root-action="cancel-note"]').click();
          if (draft !== undefined) throw new Error('取消编辑必须清除备注草稿');
          row.remove();
        });
        await page.evaluate(async () => {
          const { syncRootControls } = await import('/ui/attributes.ts');
          syncRootControls(document.querySelector('#root-list'), false, true);
        });
        assert.equal(await first.locator('[data-root-action="restore"]').isDisabled(), true, '过期视图不可操作恢复');
        assert.equal(await first.locator('[data-root-action="expand-name"]').isEnabled(), true, '过期视图仍可展开名称');
        await page.evaluate(async () => {
          const { syncRootControls } = await import('/ui/attributes.ts');
          syncRootControls(document.querySelector('#root-list'), false, false);
        });
        assert.equal(await page.locator('#record-list, #state-location').count(), 0, '内部明细和状态文件位置不进入页面');
        assert.equal(await page.locator('#root-list .root-record').count(), 3);
        assert.equal(await page.locator('#root-list img').count(), 0, '最外层名称必须按文本显示');
        const markup = await page.locator('#attributes-panel').evaluate(element => element.outerHTML);
        for (const secret of ['private-parent', 'private-child', 'state.json']) assert.ok(!markup.includes(secret), `属性页面不得包含 ${secret}`);
        assert.equal(await page.locator('#attribute-path').inputValue(), '');
        assert.ok(!(await page.locator('#root-list').ariaSnapshot()).includes('private-'), '可访问性标签不得暴露隐藏明细');
        const layout = await page.evaluate(() => ({
          width: innerWidth, documentWidth: document.documentElement.scrollWidth,
          outside: [...document.querySelectorAll('#attributes-panel button, #attributes-panel input, #attributes-panel select')].filter(element => {
            const box = element.getBoundingClientRect();
            return box.width > 0 && box.height > 0 && (box.left < -1 || box.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2);
          }).map(element => element.id || element.className)
        }));
        assert.ok(layout.documentWidth <= layout.width + 1, JSON.stringify({ width, scale, language, layout }));
        assert.deepEqual(layout.outside, [], JSON.stringify({ width, scale, language, layout }));
        const screenshot = `ui-${width}-${scale}-attributes-${language}.png`;
        await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
        checks.push({ css_width: width, device_scale_factor: scale, operation: 'attributes', language, no_horizontal_overflow: true, synthetic_root_layout: true, only_outer_names_in_dom_and_accessibility: true, names_rendered_as_text: true, add_entry_above_fold: true, add_cancel_clears_path: true, name_expansion: true, mixed_mode_requires_selection: true, mode_changes_explicit: true, note_unicode_limit: true, note_draft_preserved: true, stale_records_disable_actions: true, row_action_keeps_focus: true, screenshot });
      }
      await page.locator('#language').focus();
      await page.keyboard.press('Tab');
      assert.equal(await page.evaluate(() => {
        const element = document.activeElement;
        return element !== document.body && parseFloat(getComputedStyle(element).outlineWidth) >= 2;
      }), true, '键盘焦点必须可见');
      await page.reload({ waitUntil: 'domcontentloaded' });
      assert.equal(await page.locator('#language').inputValue(), 'en', '重启保留语言偏好');
      assert.equal(await page.locator('#package-preview').isVisible(), false, '预览不持久保存');
    } finally {
      await context.close();
    }
  }
  assert.deepEqual(errors, []);
} catch (error) {
  failed = true;
  errors.push(String(error));
  throw error;
} finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({
    schema_version: 1, version, tested_at: new Date().toISOString(), passed: !failed, checks, errors,
    limitations: ['Browser device scale factor tests are not native Windows DPI or clean-VM validation.', 'This harness tests UI layout and preferences only; it does not simulate successful desktop file operations.']
  }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`UI layout checks passed: ${checks.length}`);
