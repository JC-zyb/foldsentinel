import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 复用已有 Playwright/浏览器。夹具只提供更新元数据，不联网、不开外部页面、不执行文件操作。
const { values } = parseArgs({ options: {
  playwright: { type: 'string' }, browser: { type: 'string' }, report: { type: 'string' },
  url: { type: 'string', default: 'http://127.0.0.1:1420/' }
} });
assert.ok(values.playwright && values.browser && values.report, '需要 --playwright、--browser 和 --report');
const url = new URL(values.url);
assert.ok(url.protocol === 'http:' && url.hostname === '127.0.0.1', '仅测试本机开发服务');
const reportPath = resolve(values.report);
await mkdir(dirname(reportPath), { recursive: true });
const reportFile = await open(reportPath, 'wx');
const screenshotRoot = join(dirname(reportPath), 'screenshots');
await mkdir(screenshotRoot);
const version = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')).version;
const { chromium } = await import(pathToFileURL(resolve(values.playwright)).href);
const browser = await chromium.launch({ executablePath: resolve(values.browser), headless: true, args: ['--no-proxy-server'] });
const checks = [], errors = [], blockedRequests = [];
let passed = false;
const releaseUrl = 'https://github.com/JC-zyb/foldsentinel/releases/tag/v99.1.0';
const notes = '<script>window.updateNoteExecuted = true</script>\n<img src=x onerror="window.updateNoteExecuted=true">\n更新说明 / Release notes\n' + 'long-note-'.repeat(60);
const metadata = status => ({
  current_version: version, latest_version: status === 'no_release' ? null : status === 'available' ? '99.1.0' : status === 'ahead' ? '0.1.0' : version,
  status, release_url: status === 'no_release' ? null : releaseUrl, published_at: status === 'no_release' ? null : '2026-11-05T06:00:00Z',
  notes: status === 'no_release' ? null : notes, prerelease: true
});

async function setup(context) {
  await context.route('**/*', route => {
    const target = new URL(route.request().url());
    if (target.origin === url.origin) return route.continue();
    blockedRequests.push(target.href);
    return route.abort();
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: 30000 });
  await page.locator('#nav-settings').click();
  const production = page.locator('#update-settings');
  assert.equal(await production.locator('#check-updates').isDisabled(), true, '浏览器预览必须显式禁用桌面更新检查');
  assert.match(await production.locator('#update-status').textContent(), /桌面程序|desktop app/);
  // 替换宿主而非生产处理器；仍在真实设置布局内使用生产 UpdatesUi。
  await page.evaluate(async () => {
    const { UpdatesUi } = await import('/ui/updates.ts');
    const previous = document.querySelector('#update-settings');
    const container = document.createElement('div'); container.id = 'update-settings'; previous.replaceWith(container);
    const fixture = { calls: 0, opens: [], pendingCheck: null, pendingOpen: null, openMode: 'resolve', ui: null };
    fixture.ui = new UpdatesUi(container, {
      isDesktop: () => true,
      check: () => {
        fixture.calls += 1;
        return new Promise((resolve, reject) => { fixture.pendingCheck = { resolve, reject }; });
      },
      open: target => {
        fixture.opens.push(target);
        if (fixture.openMode === 'reject') return Promise.reject(new Error('Explicit fixture browser failure'));
        if (fixture.openMode === 'hold') return new Promise((resolve, reject) => { fixture.pendingOpen = { resolve, reject }; });
        return Promise.resolve();
      }
    });
    window.updateUiFixture = fixture;
  });
  assert.equal(await page.evaluate(() => window.updateUiFixture.calls), 0, '进入设置和初始化组件不自动请求');
  await page.locator('#nav-vault').click(); await page.locator('#nav-settings').click();
  assert.equal(await page.evaluate(() => window.updateUiFixture.calls), 0, '重新进入设置不自动请求');
  return page;
}

async function setLanguage(page, language) {
  await page.locator('#language').selectOption(language);
  await page.evaluate(() => window.updateUiFixture.ui.translate());
  assert.equal(await page.locator('html').getAttribute('lang'), language);
}

async function finishCheck(page, result, error = null) {
  await page.evaluate(({ result, error }) => {
    const pending = window.updateUiFixture.pendingCheck;
    if (!pending) throw new Error('Missing pending fixture check');
    window.updateUiFixture.pendingCheck = null;
    if (error) pending.reject(error); else pending.resolve(result);
  }, { result, error });
  await page.waitForFunction(() => document.querySelector('#check-updates').getAttribute('aria-busy') === 'false');
}

async function checkLayout(page, name, width, scheme, language) {
  const result = await page.evaluate(() => ({
    width: innerWidth, documentWidth: document.documentElement.scrollWidth,
    outside: [...document.querySelectorAll('#settings-panel button, #settings-panel select, #settings-panel input, #update-status, #update-notes-text')].filter(element => {
      const box = element.getBoundingClientRect();
      return box.width > 0 && box.height > 0 && (box.left < -1 || box.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2);
    }).map(element => element.id || element.tagName)
  }));
  assert.ok(result.documentWidth <= result.width + 1, JSON.stringify({ name, width, scheme, language, result }));
  assert.deepEqual(result.outside, [], JSON.stringify({ name, width, scheme, language, result }));
  const screenshot = `${width}-${scheme}-${language}-${name}.png`;
  await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
  checks.push({ operation: name, width, scheme, language, no_horizontal_overflow: true, screenshot });
}

try {
  const interaction = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const page = await setup(interaction);
    const check = page.locator('#check-updates');
    const status = page.locator('#update-status');
    assert.equal(await status.getAttribute('role'), 'status');
    assert.equal(await status.getAttribute('aria-live'), 'polite');
    assert.match(await status.textContent(), /尚未检查/);
    checks.push({ operation: 'manual-only', production_preview_disabled: true, entry_and_navigation_do_not_request: true });

    await check.click();
    assert.equal(await check.isDisabled(), true);
    assert.equal(await check.getAttribute('aria-busy'), 'true');
    assert.match(await status.textContent(), /正在检查/);
    // 额外发一次 DOM click，验证处理器本身也防并发，不能只依赖 disabled 外观。
    await check.dispatchEvent('click');
    assert.equal(await page.evaluate(() => window.updateUiFixture.calls), 1);
    await setLanguage(page, 'en');
    assert.match(await status.textContent(), /Checking for updates/);
    await page.locator('#nav-vault').click(); await page.locator('#nav-settings').click();
    assert.equal(await check.isDisabled(), true, '切页保留进行中状态');
    assert.equal(await page.evaluate(() => window.updateUiFixture.calls), 1);
    await page.locator('#theme').focus();
    await finishCheck(page, metadata('available'));
    assert.equal(await page.locator('#theme').evaluate(element => document.activeElement === element), true, '异步结果不抢焦点');
    assert.match(await status.textContent(), /99\.1\.0/);
    assert.equal(await page.locator('#update-checked-at').isVisible(), true);
    assert.equal(await page.locator('#update-published-at').isVisible(), true);
    assert.match(await page.locator('#update-published-at').textContent(), /Preview release/);
    await page.locator('#update-notes > summary').click();
    assert.equal(await page.locator('#update-notes-text').textContent(), notes);
    assert.equal(await page.locator('#update-notes-text script, #update-notes-text img').count(), 0);
    assert.equal(await page.evaluate(() => window.updateNoteExecuted), undefined);
    await setLanguage(page, 'zh-CN');
    assert.match(await status.textContent(), /发现新版本/);
    assert.equal(await page.locator('#update-notes').getAttribute('open'), '', '翻译不折叠已展开说明');
    await page.locator('#nav-vault').click(); await page.locator('#nav-settings').click();
    assert.match(await status.textContent(), /发现新版本/);
    assert.equal(await page.evaluate(() => window.updateUiFixture.calls), 1);
    checks.push({ operation: 'loading-and-available', duplicate_request_blocked: true, result_survives_navigation_and_translation: true, external_focus_preserved: true, notes_rendered_as_text: true });

    await page.evaluate(() => { window.updateUiFixture.openMode = 'hold'; });
    const openRelease = page.locator('#open-release');
    await openRelease.click();
    assert.equal(await openRelease.isDisabled(), true);
    await openRelease.dispatchEvent('click');
    assert.deepEqual(await page.evaluate(() => window.updateUiFixture.opens), [releaseUrl]);
    await page.evaluate(() => { window.updateUiFixture.pendingOpen.reject(new Error('Explicit fixture browser failure')); });
    await page.locator('#update-open-error').waitFor({ state: 'visible' });
    assert.equal(await openRelease.isEnabled(), true);
    await page.evaluate(() => { window.updateUiFixture.openMode = 'resolve'; });
    await openRelease.click();
    assert.equal(await page.locator('#update-open-error').isHidden(), true);
    assert.deepEqual(await page.evaluate(() => window.updateUiFixture.opens), [releaseUrl, releaseUrl]);
    checks.push({ operation: 'open-release', validated_metadata_url_forwarded: true, duplicate_open_blocked: true, failure_visible_and_retryable: true, no_external_page_opened: true });

    for (const [state, zh, en] of [
      ['up_to_date', /最新发布/, /latest published version/],
      ['ahead', /本地版本高于/, /local version is newer/],
      ['no_release', /暂未找到/, /No comparable published version/]
    ]) {
      await check.click(); await finishCheck(page, metadata(state));
      assert.match(await status.textContent(), zh);
      await setLanguage(page, 'en'); assert.match(await status.textContent(), en);
      assert.equal(await page.locator('#update-release').isVisible(), state !== 'no_release');
      await setLanguage(page, 'zh-CN');
      checks.push({ operation: state, bilingual_status: true, release_visibility_matches_metadata: true });
    }
    for (const code of ['network', 'timeout', 'rate_limited', 'invalid_response', 'unavailable', 'busy', 'unexpected_fixture_error']) {
      await check.click(); await finishCheck(page, null, code);
      assert.equal(await check.isEnabled(), true);
      assert.equal(await page.locator('#update-release').isHidden(), true, '失败后不显示旧发布结果');
      assert.equal(await page.locator('#update-checked-at').isHidden(), true, '失败不能伪装成功检查时间');
      assert.match(await status.getAttribute('class'), /update-error/);
      const chinese = await status.textContent();
      await setLanguage(page, 'en');
      assert.notEqual(await status.textContent(), chinese, '错误状态同步翻译');
      await setLanguage(page, 'zh-CN');
      checks.push({ operation: `error-${code}`, explicit_error: true, retry_enabled: true, old_success_hidden: true, bilingual_status: true });
    }
    await check.click(); await finishCheck(page, metadata('up_to_date'));
    assert.ok(!(await status.getAttribute('class')).includes('update-error'), '重试成功清除错误状态');
    checks.push({ operation: 'retry-success', error_cleared_after_success: true });
  } finally { await interaction.close(); }

  for (const width of [375, 720, 1080]) for (const scheme of ['light', 'dark']) for (const language of ['zh-CN', 'en']) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, colorScheme: scheme, locale: language });
    try {
      const page = await setup(context);
      await setLanguage(page, language);
      await page.locator('#theme').selectOption(scheme);
      await page.locator('#check-updates').click();
      await checkLayout(page, 'checking', width, scheme, language);
      await finishCheck(page, metadata('available'));
      await page.locator('#update-notes > summary').click();
      await checkLayout(page, 'available-notes', width, scheme, language);
      await page.locator('#check-updates').click(); await finishCheck(page, null, 'network');
      await checkLayout(page, 'network-error', width, scheme, language);
    } finally { await context.close(); }
  }
  assert.deepEqual(blockedRequests, [], '真实页面及组件夹具不得发出外站请求');
  assert.deepEqual(errors, []);
  passed = true;
} catch (error) { errors.push(String(error)); throw error; }
finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors, blocked_requests: blockedRequests,
    limitations: ['Uses the production UpdatesUi with explicit metadata and external-open dependency fixtures. It does not prove live GitHub availability, WinHTTP behavior or default-browser launch.', 'All external requests are blocked and reported. No file operations, downloads or installations are simulated.', 'Browser width/theme checks do not prove native Windows DPI or screen-reader operation.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`Updates UI checks passed: ${checks.length}`);
