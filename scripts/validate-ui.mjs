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
        // 合成数据只用于真实组件的排版检查，不接入 IPC 或模拟成功属性操作。
        await page.evaluate(async () => {
          const { createAttributeRecord, createRootRecord } = await import('/ui/attributes.ts');
          const roots = document.querySelector('#root-list');
          roots.replaceChildren(createRootRecord({ path: `D:\\test\\${'中文最外层路径'.repeat(18)}`, entry_count: 54 }, () => {}, () => {}), createRootRecord({ path: 'D:\\<img src=x onerror=alert(1)>.txt', entry_count: 1 }, () => {}, () => {}));
          const statuses = ['consistent', 'changed', 'missing', 'unreadable'];
          const list = document.querySelector('#record-list');
          list.replaceChildren(...statuses.map((state, index) => createAttributeRecord({
            entry: { path: `D:\\test\\${'中文长路径'.repeat(18)}\\${index}.txt`, mode: 'hidden', original_attributes: 32, managed_attributes: index === 0 ? 3 : 7 },
            current_attributes: index > 1 ? null : index === 0 ? 34 : 32,
            expected_attributes: index === 0 ? 2 : 6, state, ...(index === 2 ? { os_code: 2 } : {})
          }, () => {})));
          list.append(createAttributeRecord({
            entry: { path: 'D:\\<img src=x onerror=alert(1)>.txt', mode: 'readonly', original_attributes: 32, managed_attributes: 3 },
            current_attributes: 33, expected_attributes: 1, state: 'consistent'
          }, () => {}));
        });
        assert.equal(await page.locator('#record-list .record').count(), 5);
        assert.equal(await page.locator('#record-list img').count(), 0, '路径必须按文本显示');
        assert.equal(await page.locator('#root-list .root-record').count(), 2);
        assert.equal(await page.locator('#root-list img').count(), 0, '已保存路径必须按文本显示');
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
        checks.push({ css_width: width, device_scale_factor: scale, operation: 'attributes', language, no_horizontal_overflow: true, synthetic_status_layout: true, paths_rendered_as_text: true, screenshot });
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
