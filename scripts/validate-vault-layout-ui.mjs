import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 仅验证三个包操作页的生产渲染和本地草稿；恢复与预览夹具只提供内存展示元数据。
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
const checks = [], errors = [], blockedRequests = [], screenshots = [];
const operations = ['encrypt', 'decrypt', 'verify'];
let passed = false;

async function setup(context, { language = 'zh-CN', theme = 'light', density = 'standard', fontSize = 'standard' } = {}) {
  await context.addInitScript(seed => {
    localStorage.setItem('foldsentinel.language', seed.language);
    localStorage.setItem('foldsentinel.theme', seed.theme);
    localStorage.setItem('foldsentinel.preferences.v1', JSON.stringify({ density: seed.density, fontSize: seed.fontSize }));
  }, { language, theme, density, fontSize });
  await context.route('**/*', async route => {
    const target = new URL(route.request().url());
    if (target.origin !== url.origin) { blockedRequests.push(route.request().url()); await route.abort(); return; }
    if (target.pathname === '/ui/main.ts') {
      const response = await route.fetch();
      // 测试响应增加只读词条与内存展示入口，不替换处理器，不提供 Tauri IPC。
      await route.fulfill({ response, body: await response.text() + `
window.vaultLayoutValidation = {
  text: key => t(key),
  language: value => { setLanguage(value); translate(); },
  recovery: (includeInterrupted = false) => {
    recordState.snapshot = { roots: [
      { id: 'vault-layout-one', name: '布局展示记录一', mode: 'locked', note: '', entry_count: 1 },
      { id: 'vault-layout-two', name: '布局展示记录二', mode: 'hidden', note: '', entry_count: 1 }
    ], total: 2, overall_total: 2, offset: 0 };
    renderRecords();
    jobs = includeInterrupted ? [{ id: 'vault-layout-invalid-record', status: 'invalid', record: null, code: 'invalid' }] : [];
    renderJobs();
  },
  previewPresentation: () => {
    packagePreview = { input: 'D:\\\\vault-layout-fixture\\\\display-only.fse', value: {
      format_version: 2, file_count: 0, directory_count: 0, content_bytes: 0,
      total_entries: 0, entries: [], truncated: false
    } };
    renderPreview();
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
  await page.waitForFunction(() => !!window.vaultLayoutValidation);
  await page.evaluate(() => window.vaultLayoutValidation.recovery());
  return page;
}

async function message(page, key) { return page.evaluate(key => window.vaultLayoutValidation.text(key), key); }
async function dimensions(page, metadata) {
  await page.evaluate(() => scrollTo(0, 0));
  const result = await page.locator('#vault-panel').evaluate(panel => {
    const visible = element => {
      const box = element.getBoundingClientRect();
      return box.width > 0 && box.height > 0 && !element.closest('[hidden]') && !element.classList.contains('sr-only');
    };
    const main = document.querySelector('main'), style = getComputedStyle(main);
    const source = panel.querySelector('.vault-source').getBoundingClientRect();
    const password = panel.querySelector('.vault-password').getBoundingClientRect();
    const add = panel.querySelector('#add-to-queue').getBoundingClientRect();
    const form = panel.querySelector('#vault-form').getBoundingClientRect();
    const queue = panel.querySelector('#queue-title').getBoundingClientRect();
    const outside = [...panel.querySelectorAll('button, input, summary, p, h2, label, strong')].filter(visible).filter(element => {
      const box = element.getBoundingClientRect();
      return box.left < -1 || box.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2;
    }).map(element => element.id || element.className || element.tagName);
    return {
      viewport: innerWidth, page: document.documentElement.scrollWidth, outside,
      workspace: main.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight),
      columns: { source: { x: source.x, y: source.y }, password: { x: password.x, y: password.y } },
      add: { y: add.y, bottom: add.bottom }, form_height: form.height, queue_heading_y: queue.y
    };
  });
  assert.ok(result.page <= result.viewport + 1, JSON.stringify({ ...metadata, result }));
  assert.deepEqual(result.outside, [], JSON.stringify({ ...metadata, result }));
  if (result.workspace >= 720) {
    assert.ok(Math.abs(result.columns.source.y - result.columns.password.y) <= 1 && result.columns.password.x > result.columns.source.x,
      JSON.stringify({ ...metadata, message: '宽容器使用来源、密码双列', result }));
  } else {
    assert.ok(result.columns.password.y > result.columns.source.y && Math.abs(result.columns.source.x - result.columns.password.x) <= 1,
      JSON.stringify({ ...metadata, message: '窄容器按来源、密码单列排列', result }));
  }
  return result;
}

async function modeContract(page, operation) {
  assert.equal(await page.locator('#confirm-field').isVisible(), operation === 'encrypt');
  assert.equal(await page.locator('#confirm-password').evaluate(input => input.required), operation === 'encrypt');
  assert.equal(await page.locator('#output-fields').isVisible(), operation !== 'verify');
  assert.equal(await page.locator('#pick-folder').isVisible(), operation === 'encrypt');
  assert.equal(await page.locator('#password').getAttribute('autocomplete'), operation === 'encrypt' ? 'new-password' : 'current-password');
  assert.equal(await page.locator(`#${operation}-tab`).getAttribute('aria-pressed'), 'true');
  assert.equal(await page.locator('.segmented button[aria-pressed="true"]').count(), 1);
  assert.equal(await page.locator('#pick-file').textContent(), await message(page, operation === 'encrypt' ? 'pickFiles' : 'pickPackages'));
  assert.equal(await page.locator('#input-path').getAttribute('placeholder'), await message(page, operation === 'encrypt' ? 'sourcePlaceholder' : 'packagePlaceholder'));
  assert.equal(await page.locator('#output-path').getAttribute('placeholder'), await message(page, operation === 'decrypt' ? 'restoreOutputPlaceholder' : 'outputPlaceholder'));
  assert.equal(await page.locator('#vault-notice').textContent(), await message(page, operation === 'encrypt' ? 'notice' : operation === 'decrypt' ? 'restoreNotice' : 'verifyNotice'));
  if (operation !== 'verify') {
    assert.equal(await page.locator('#output-help').textContent(), await message(page, operation === 'encrypt' ? 'outputHelp' : 'restoreOutputHelp'));
  }
  assert.ok((await page.locator('#password-help').textContent()).trim());
  const fields = await page.locator('.vault-password .password-grid').evaluate(grid => {
    const input = grid.querySelector('#password').getBoundingClientRect();
    const confirm = grid.querySelector('#confirm-password').getBoundingClientRect();
    const main = document.querySelector('main'), style = getComputedStyle(main);
    return {
      ratio: input.width / grid.getBoundingClientRect().width,
      input: { y: input.y, right: input.right }, confirm: { x: confirm.x, y: confirm.y },
      workspace: main.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
    };
  });
  if (operation !== 'encrypt') assert.ok(fields.ratio >= .98, `包操作的单个密码框应占整行：${JSON.stringify(fields)}`);
  else if (fields.workspace >= 1000) {
    assert.ok(Math.abs(fields.input.y - fields.confirm.y) <= 1 && fields.input.right <= fields.confirm.x + 1,
      `大屏创建页密码与确认密码应并排：${JSON.stringify(fields)}`);
  }
  for (const id of ['password', 'confirm-password', 'output-path', 'input-path']) {
    assert.equal(await page.locator(`label[for="${id}"]`).count(), 1, `${id} 保留可访问标签`);
  }
}

async function capture(page, name, fullPage = false) {
  await page.evaluate(() => scrollTo(0, 0));
  await page.mouse.move(0, 0);
  await page.waitForTimeout(200);
  await page.screenshot({ path: join(screenshotRoot, name), fullPage });
  screenshots.push(name);
}

try {
  for (const width of [375, 720, 960, 961, 1080, 1440]) {
    for (const language of ['zh-CN', 'en']) {
      for (const theme of ['light', 'dark']) {
        const context = await browser.newContext({ viewport: { width, height: 820 }, locale: language });
        try {
          const page = await setup(context, { language, theme });
          assert.equal(await page.locator('html').getAttribute('lang'), language);
          assert.equal(await page.locator('#attribute-alert').isVisible(), true);
          let encryptionPasswordHelp;
          for (const operation of operations) {
            await page.locator(`#${operation}-tab`).click();
            await modeContract(page, operation);
            if (operation === 'encrypt') encryptionPasswordHelp = await page.locator('#password-help').textContent();
            else assert.notEqual(await page.locator('#password-help').textContent(), encryptionPasswordHelp, '包操作使用已有密码说明');
            const metadata = { css_width: width, height: 820, language, theme, operation };
            const layout = await dimensions(page, metadata);
            if (width === 1080) assert.ok(layout.add.y >= 0 && layout.add.bottom <= 820, JSON.stringify({ ...metadata, message: '有恢复提醒时默认加入队列动作完整位于首屏', layout }));
            checks.push({ ...metadata, field_and_copy_contract: true, no_horizontal_overflow: true, adaptive_columns: true, ...layout });
            if (width === 1440 && (language === 'zh-CN' && theme === 'light' || language === 'en' && theme === 'dark')) {
              await page.setViewportSize({ width, height: 900 });
              await capture(page, `vault-${width}-900-${operation}-${language}-${theme}.png`);
              await page.setViewportSize({ width, height: 820 });
            }
            if (width === 375 && language === 'zh-CN' && theme === 'light' && operation === 'encrypt') {
              await capture(page, 'vault-375-encrypt-zh-CN-light.png', true);
            }
          }
        } finally { await context.close(); }
      }
    }
  }

  for (const [width, language, theme, density, fontSize] of [
    [375, 'en', 'dark', 'compact', 'large'],
    [720, 'zh-CN', 'light', 'standard', 'large'],
    [961, 'en', 'dark', 'standard', 'large'],
    [1080, 'en', 'light', 'compact', 'large']
  ]) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, locale: language });
    try {
      const page = await setup(context, { language, theme, density, fontSize });
      await page.evaluate(() => window.vaultLayoutValidation.recovery(true));
      for (const operation of operations) {
        await page.locator(`#${operation}-tab`).click();
        const metadata = { css_width: width, language, theme, density, fontSize, operation, two_recovery_alerts: true };
        const layout = await dimensions(page, metadata);
        await modeContract(page, operation);
        checks.push({ ...metadata, no_horizontal_overflow: true, adaptive_columns: true, ...layout });
      }
      if (width === 375) await capture(page, 'vault-375-verify-en-dark-large-compact.png', true);
    } finally { await context.close(); }
  }

  const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const page = await setup(context);
    await page.locator('#manual-sources > summary').click();
    const source = 'D:\\vault-layout-fixture\\source.txt';
    await page.locator('#input-path').fill(source);
    await page.locator('#output-path').fill('D:\\vault-layout-fixture\\encrypted');
    await page.locator('#password').fill('display-only-password');
    await page.locator('#confirm-password').fill('display-only-password');
    await page.locator('#show-password').check();
    await page.evaluate(() => window.vaultLayoutValidation.previewPresentation());
    assert.equal(await page.locator('#package-preview').isVisible(), true);
    await page.locator('#decrypt-tab').click();
    assert.equal(await page.locator('#package-preview').isHidden(), true, '切换操作清除预览');
    assert.equal(await page.locator('#password').inputValue(), '');
    assert.equal(await page.locator('#confirm-password').inputValue(), '');
    assert.equal(await page.locator('#show-password').isChecked(), false);
    assert.equal(await page.locator('#password').getAttribute('type'), 'password');
    assert.equal(await page.locator('#input-path').inputValue(), source);
    assert.equal(await page.locator('#output-path').inputValue(), '', '恢复不继承加密输出草稿');
    await page.locator('#output-path').fill('D:\\vault-layout-fixture\\restored');
    await page.locator('#verify-tab').click();
    assert.equal(await page.locator('#output-fields').isHidden(), true);
    await page.locator('#encrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), 'D:\\vault-layout-fixture\\encrypted');
    await page.locator('#decrypt-tab').click();
    assert.equal(await page.locator('#output-path').inputValue(), 'D:\\vault-layout-fixture\\restored');
    assert.equal(await page.locator('#input-path').inputValue(), source);
    assert.equal(await page.locator('#source-list > li').count(), 1);
    checks.push({ operation: 'mode-transitions', sources_preserved: true, separate_output_drafts: true, password_and_preview_cleared: true });

    for (const operation of operations) {
      await page.locator(`#${operation}-tab`).click();
      const output = await page.locator('#output-path').inputValue();
      await page.locator('#password').fill('display-only-password');
      for (const language of ['en', 'zh-CN']) {
        await page.evaluate(language => window.vaultLayoutValidation.language(language), language);
        assert.equal(await page.locator('html').getAttribute('lang'), language);
        await modeContract(page, operation);
        assert.equal(await page.locator('#password-help').textContent(), await message(page, operation === 'encrypt' ? 'passwordHelp' : 'packagePasswordHelp'));
        assert.equal(await page.locator('#input-path').inputValue(), source);
        assert.equal(await page.locator('#output-path').inputValue(), output);
        assert.equal(await page.locator('#password').inputValue(), 'display-only-password', '页内翻译不经过导航或清除当前输入');
      }
      checks.push({ operation, scenario: 'in-place-language-switch', mode_copy_and_placeholders_preserved: true, input_values_preserved: true });
    }

    await page.locator('#input-path').fill(`D:\\vault-layout-fixture\\${'很长的目录名称'.repeat(24)}\\source.fse\nD:\\vault-layout-fixture\\second.fse`);
    const layout = await dimensions(page, { operation: 'expanded-long-paths' });
    assert.equal(await page.locator('#source-list > li').count(), 2);
    await page.locator('#source-list > li button').last().click();
    assert.equal(await page.locator('#source-list > li').count(), 1, '长清单仍可逐项移除');
    await page.locator('#clear-sources').click();
    assert.equal(await page.locator('#draft').isHidden(), true);
    await page.locator('#manual-sources > summary').focus();
    await page.keyboard.press('Enter');
    assert.equal(await page.locator('#manual-sources').evaluate(details => details.open), false, '手动输入支持键盘折叠');
    checks.push({ operation: 'expanded-sources', long_paths_no_overflow: true, removal_and_clear: true, keyboard_disclosure: true, ...layout });
  } finally { await context.close(); }

  assert.deepEqual(errors, [], '不应出现页面脚本错误');
  assert.deepEqual(blockedRequests, [], '不应发起外部网络请求');
  passed = true;
} catch (error) {
  errors.push(String(error));
  throw error;
} finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({
    schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors, blocked_requests: blockedRequests, screenshots,
    limitations: ['Only browser rendering, input and draft behavior are checked; no native file operation or IPC is invoked.', 'Recovery and preview metadata are explicit presentation fixtures, not evidence of successful protection, verification or restore.', 'Browser viewport tests do not replace native Windows DPI, screen reader or real WebView2 acceptance.']
  }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`Vault layout checks passed: ${checks.length}; screenshots: ${screenshots.length}`);
