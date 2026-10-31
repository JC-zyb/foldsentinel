import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { resolve, dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

// 复用已安装的 Playwright/Chrome；合成数据只展示组件，不连接 IPC 或执行文件操作。
const { values } = parseArgs({ options: {
  playwright: { type: 'string' }, browser: { type: 'string' }, report: { type: 'string' },
  'forced-colors-only': { type: 'boolean', default: false },
  url: { type: 'string', default: 'http://127.0.0.1:1420/' }
} });
assert.ok(values.playwright && values.browser && values.report, '需要 --playwright、--browser 和 --report');
const url = new URL(values.url);
assert.ok(url.protocol === 'http:' && url.hostname === '127.0.0.1', '仅测试本机开发服务');
const reportPath = resolve(values.report);
const reportFile = await open(reportPath, 'wx');
const screenshotRoot = join(dirname(reportPath), 'screenshots');
await mkdir(screenshotRoot);
const version = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')).version;
const { chromium } = await import(pathToFileURL(resolve(values.playwright)).href);
const browser = await chromium.launch({ executablePath: resolve(values.browser), headless: true, args: ['--no-proxy-server'] });
const checks = [], errors = [];
let passed = false;

// 每个视图都读取浏览器实际样式。半透明背景由根到叶逐层合成，不用父元素单层颜色猜测。
async function audit(page, name, width, scheme, language) {
  await page.keyboard.press('Tab');
  const result = await page.evaluate(() => {
    const rgba = value => {
      const numbers = value.match(/[\d.]+/g)?.map(Number);
      if (!numbers || numbers.length < 3) throw new Error(`不支持的颜色：${value}`);
      return [...numbers.slice(0, 3), numbers[3] ?? 1];
    };
    const over = (top, bottom) => {
      const alpha = top[3] + bottom[3] * (1 - top[3]);
      return [0, 1, 2].map(i => (top[i] * top[3] + bottom[i] * bottom[3] * (1 - top[3])) / alpha).concat(alpha);
    };
    const background = element => {
      const chain = []; for (let current = element; current; current = current.parentElement) chain.unshift(current);
      return chain.reduce((color, current) => over(rgba(getComputedStyle(current).backgroundColor), color), [255, 255, 255, 1]);
    };
    const luminance = color => color.slice(0, 3).map(v => v / 255).map(v => v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4).reduce((sum, v, i) => sum + v * [.2126, .7152, .0722][i], 0);
    const contrast = (a, b) => (Math.max(luminance(a), luminance(b)) + .05) / (Math.min(luminance(a), luminance(b)) + .05);
    const visible = element => {
      if (element.closest('[hidden], .sr-only')) return false;
      for (let parent = element.parentElement; parent; parent = parent.parentElement) {
        if (parent.matches('details:not([open])') && !parent.querySelector(':scope > summary')?.contains(element)) return false;
      }
      const style = getComputedStyle(element), box = element.getBoundingClientRect();
      return style.visibility === 'visible' && style.display !== 'none' && box.width > 1 && box.height > 1 && element.getClientRects().length > 0;
    };
    const enabled = element => !element.closest('button:disabled, input:disabled, select:disabled, textarea:disabled, fieldset:disabled');
    const id = element => element.id || `${element.tagName.toLowerCase()}.${String(element.className).replaceAll(' ', '.')}`;
    const text = [], placeholders = [], boundaries = [], focus = [], wrapped = [], unsupported = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const node = walker.currentNode, element = node.parentElement;
      if (!node.textContent.trim() || !visible(element) || !enabled(element) || element.closest('script, style, option')) continue;
      const range = document.createRange(); range.selectNodeContents(node);
      if (![...range.getClientRects()].some(rect => rect.width > 1 && rect.height > 1)) continue;
      const bg = background(element), style = getComputedStyle(element);
      text.push({ element: id(element), ratio: contrast(over(rgba(style.color), bg), bg) });
      if (style.backgroundImage !== 'none' || style.opacity !== '1') unsupported.push(id(element));
    }
    const controls = [...document.querySelectorAll('button, input, select, textarea, summary, [tabindex]')].filter(element => visible(element) && enabled(element));
    for (const element of controls) {
      const style = getComputedStyle(element), bg = background(element), outside = background(element.parentElement);
      if (element.matches('input, textarea') && element.getAttribute('placeholder') && !element.value) {
        placeholders.push({ element: id(element), ratio: contrast(over(rgba(getComputedStyle(element, '::placeholder').color), bg), bg) });
      }
      if (element.matches('input:not([type=checkbox]), select, textarea')) {
        const border = over(rgba(style.borderTopColor), outside);
        boundaries.push({ element: id(element), ratio: Math.max(contrast(border, outside), contrast(bg, outside)) });
        if (element.value) text.push({ element: id(element), ratio: contrast(over(rgba(style.color), bg), bg) });
      } else if (element.matches('.primary, .secondary')) {
        boundaries.push({ element: id(element), ratio: Math.max(contrast(over(rgba(style.borderTopColor), outside), outside), contrast(bg, outside)) });
      }
      // 文字按钮和 summary 由文字辨识；其文字已经按 4.5:1 检查。原生 checkbox 外观不能仅靠 CSS 颜色完整判定。
      element.focus({ preventScroll: true });
      const focused = getComputedStyle(element);
      focus.push({ element: id(element), width: parseFloat(focused.outlineWidth), ratio: contrast(over(rgba(focused.outlineColor), outside), outside), visible: element.matches(':focus-visible') });
    }
    if (innerWidth >= 1080) for (const element of document.querySelectorAll('.primary')) {
      if (!visible(element)) continue;
      const range = document.createRange(); range.selectNodeContents(element);
      const lines = [...range.getClientRects()].filter(rect => rect.width > 0).map(rect => Math.round(rect.top));
      if (new Set(lines).size > 1) wrapped.push(id(element));
    }
    const overflow = [...document.querySelectorAll('button, input, select, textarea, summary')].filter(element => {
      if (!visible(element)) return false;
      const rect = element.getBoundingClientRect();
      return rect.left < -1 || rect.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2;
    }).map(id);
    document.activeElement?.blur();
    return { documentWidth: document.documentElement.scrollWidth, viewport: innerWidth, text, placeholders, boundaries, focus, wrapped, overflow, unsupported: [...new Set(unsupported)] };
  });
  const failures = {
    text: result.text.filter(item => item.ratio < 4.5), placeholders: result.placeholders.filter(item => item.ratio < 4.5),
    boundaries: result.boundaries.filter(item => item.ratio < 3), focus: result.focus.filter(item => item.ratio < 3 || item.width < 2 || !item.visible),
    wrapped: result.wrapped, overflow: result.overflow
  };
  assert.ok(result.documentWidth <= result.viewport + 1, JSON.stringify({ name, width, scheme, language, documentWidth: result.documentWidth }));
  for (const [kind, failed] of Object.entries(failures)) assert.deepEqual(failed, [], JSON.stringify({ name, width, scheme, language, kind, failed }));
  assert.deepEqual(result.unsupported, [], '可见文本的背景图片或透明度需要单独人工判定');
  const screenshot = `${width}-${scheme}-${language}-${name}.png`;
  await page.evaluate(() => scrollTo(0, 0));
  await page.screenshot({ path: join(screenshotRoot, screenshot), fullPage: true });
  checks.push({ width, scheme, language, view: name, screenshot, text_samples: result.text.length, placeholder_samples: result.placeholders.length, boundary_samples: result.boundaries.length, focus_samples: result.focus.length,
    minimum_text_contrast: Math.min(...result.text.map(item => item.ratio)), minimum_boundary_contrast: Math.min(...result.boundaries.map(item => item.ratio)), minimum_focus_contrast: Math.min(...result.focus.map(item => item.ratio)), no_horizontal_overflow: true, desktop_primary_actions_single_line: true });
}

async function tokens(page, scheme, width, language) {
  const result = await page.evaluate(() => {
    const style = getComputedStyle(document.documentElement), names = ['text', 'text-secondary', 'muted', 'placeholder', 'page-bg', 'surface', 'surface-subtle', 'surface-soft', 'surface-hover', 'surface-pressed', 'control-bg', 'blue', 'on-primary', 'primary-bg', 'primary-hover', 'primary-pressed', 'selection-text', 'selection-bg', 'selected-text', 'selected-bg', 'step-text', 'step-bg', 'info-text', 'info-bg', 'success-text', 'success-bg', 'danger-text', 'danger-bg', 'warning', 'warning-bg', 'success', 'danger', 'warning-inline', 'nav-item-text', 'nav-bg', 'nav-hover', 'nav-on-selected', 'nav-selected', 'nav-muted', 'nav-footer-text', 'control-border', 'focus', 'nav-accent'];
    return { dark: matchMedia('(prefers-color-scheme: dark)').matches, colorScheme: style.colorScheme, tokens: Object.fromEntries(names.map(name => [name, style.getPropertyValue(`--${name}`).trim()])) };
  });
  assert.equal(result.dark, scheme === 'dark');
  assert.equal(result.tokens['page-bg'], scheme === 'dark' ? '#0e1923' : '#f4f7fa');
  const luminance = hex => {
    hex = hex.slice(1); if (hex.length === 3) hex = hex.split('').map(value => value + value).join('');
    return hex.match(/.{2}/g).slice(0, 3).map(value => parseInt(value, 16) / 255).map(value => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4).reduce((sum, value, index) => sum + value * [.2126, .7152, .0722][index], 0);
  };
  const pairs = [];
  for (const foreground of ['text', 'text-secondary', 'muted', 'blue']) for (const background of ['page-bg', 'surface', 'surface-subtle', 'surface-soft', 'surface-hover', 'surface-pressed']) pairs.push([foreground, background, 4.5]);
  pairs.push(['placeholder', 'control-bg', 4.5], ['on-primary', 'primary-bg', 4.5], ['on-primary', 'primary-hover', 4.5], ['on-primary', 'primary-pressed', 4.5], ['selection-text', 'selection-bg', 4.5], ['selected-text', 'selected-bg', 4.5], ['step-text', 'step-bg', 4.5], ['info-text', 'info-bg', 4.5], ['success-text', 'success-bg', 4.5], ['danger-text', 'danger-bg', 4.5], ['warning', 'warning-bg', 4.5], ['success', 'surface', 4.5], ['danger', 'surface', 4.5], ['warning-inline', 'surface', 4.5], ['nav-item-text', 'nav-bg', 4.5], ['nav-item-text', 'nav-hover', 4.5], ['nav-on-selected', 'nav-selected', 4.5], ['nav-muted', 'nav-bg', 4.5], ['nav-footer-text', 'nav-bg', 4.5], ['control-border', 'control-bg', 3], ['control-border', 'surface-subtle', 3], ['control-border', 'surface-soft', 3], ['focus', 'page-bg', 3], ['focus', 'surface', 3], ['nav-accent', 'nav-selected', 3]);
  const ratios = pairs.map(([foreground, background, minimum]) => {
    const a = luminance(result.tokens[foreground]), b = luminance(result.tokens[background]);
    return { foreground, background, minimum, ratio: (Math.max(a, b) + .05) / (Math.min(a, b) + .05) };
  });
  for (const pair of ratios) assert.ok(pair.ratio >= pair.minimum, JSON.stringify(pair));
  checks.push({ width, scheme, language, view: 'computed-semantic-tokens', system_preference_applied: true, color_scheme: result.colorScheme, pairs: ratios });
}

async function forcedColors() {
  const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, forcedColors: 'active', locale: 'en' });
  try {
    // 直接加载生产页面，只切换页面与操作模式，不注入渲染器或调用文件操作。
    await context.route('**/*', route => new URL(route.request().url()).origin === url.origin ? route.continue() : route.abort());
    const page = await context.newPage(); page.on('pageerror', error => errors.push(error.message));
    await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: 30000 });
    await page.locator('#language').selectOption('en');
    assert.equal(await page.evaluate(() => matchMedia('(forced-colors: active)').matches), true);
    const inspect = () => page.evaluate(() => {
      const systemColor = name => {
        const probe = document.createElement('span'); probe.style.color = name; probe.hidden = true; document.body.append(probe);
        const value = getComputedStyle(probe).color; probe.remove(); return value;
      };
      return { buttonText: systemColor('ButtonText'), highlight: systemColor('Highlight'), controls: Object.fromEntries(
        [...document.querySelectorAll('.nav-item, .segmented button')].map(element => {
          const style = getComputedStyle(element), box = element.getBoundingClientRect();
          return [element.id, { decoration: style.textDecorationLine, decorationStyle: style.textDecorationStyle,
            decorationColor: style.textDecorationColor, decorationThickness: parseFloat(style.textDecorationThickness),
            outlineStyle: style.outlineStyle, outlineColor: style.outlineColor, outlineWidth: parseFloat(style.outlineWidth),
            focusVisible: element.matches(':focus-visible'), width: box.width, height: box.height }];
        })) };
    });
    const assertSelection = (result, selected, unselected) => {
      for (const id of selected) {
        const control = result.controls[id];
        assert.equal(control.decoration, 'underline', `${id} 的选中状态须使用实体下划线`);
        assert.equal(control.decorationStyle, 'solid');
        assert.ok(control.decorationThickness >= 3);
        assert.equal(control.decorationColor, result.buttonText, `${id} 的选中线须遵循系统文字色`);
      }
      for (const id of unselected) assert.equal(result.controls[id].decoration, 'none', `${id} 未选中时不显示选中线`);
    };
    const assertFocus = (result, id) => {
      const control = result.controls[id];
      assert.equal(control.focusVisible, true, `${id} 的键盘焦点须可见`);
      assert.equal(control.outlineStyle, 'solid'); assert.ok(control.outlineWidth >= 3);
      assert.equal(control.outlineColor, result.highlight, '焦点使用系统 Highlight，与选中下划线分别表达');
    };
    const initial = await inspect();
    assertSelection(initial, ['nav-vault', 'encrypt-tab'], ['nav-attributes', 'decrypt-tab', 'verify-tab']);
    await page.keyboard.press('Tab'); await page.locator('#nav-attributes').focus();
    const navFocus = await inspect();
    assertSelection(navFocus, ['nav-vault', 'encrypt-tab'], ['nav-attributes', 'decrypt-tab', 'verify-tab']);
    assertFocus(navFocus, 'nav-attributes');
    assert.equal(navFocus.controls['nav-vault'].outlineStyle, 'none', '选中但未聚焦的导航无需焦点轮廓');
    await page.locator('#nav-attributes').click();
    assertSelection(await inspect(), ['nav-attributes', 'encrypt-tab'], ['nav-vault', 'decrypt-tab', 'verify-tab']);
    await page.locator('#nav-vault').click(); await page.locator('#decrypt-tab').click();
    await page.keyboard.press('Tab'); await page.locator('#encrypt-tab').focus();
    const modeFocus = await inspect();
    assertSelection(modeFocus, ['nav-vault', 'decrypt-tab'], ['nav-attributes', 'encrypt-tab', 'verify-tab']);
    assertFocus(modeFocus, 'encrypt-tab');
    assert.equal(modeFocus.controls['decrypt-tab'].outlineStyle, 'none', '选中但未聚焦的操作模式无需焦点轮廓');
    for (const [id, control] of Object.entries(initial.controls)) {
      assert.equal(modeFocus.controls[id].width, control.width, `${id} 切换选中与焦点不得改变宽度`);
      assert.equal(modeFocus.controls[id].height, control.height, `${id} 切换选中与焦点不得改变高度`);
    }
    const screenshot = '1080-forced-colors-en-selection-and-focus.png';
    await page.screenshot({ path: join(screenshotRoot, screenshot) });
    checks.push({ width: 1080, language: 'en', view: 'forced-colors-selection-and-focus', screenshot,
      production_page: true, selected_navigation_and_mode_underlined: true, selection_tracks_changes: true,
      independent_keyboard_focus: true, control_dimensions_preserved: true, computed: { navFocus, modeFocus } });
  } finally { await context.close(); }
}

try {
  if (!values['forced-colors-only']) for (const width of [375, 720, 1080]) for (const scheme of ['light', 'dark']) for (const language of ['zh-CN', 'en']) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, colorScheme: scheme, locale: language });
    try {
      await context.route('**/*', async route => {
        const target = new URL(route.request().url());
        if (target.origin !== url.origin) { await route.abort(); return; }
        if (target.pathname === '/ui/main.ts') {
          const response = await route.fetch();
          // 仅测试响应暴露生产渲染器；不更改处理器，不执行任务，不生成文件成功结果。
          await route.fulfill({ response, body: await response.text() + '\nwindow.uiThemeFixture = { queue, renderTasks, setSources, setPreview: value => { packagePreview = value; renderPreview(); }, setProgress: value => { element("job-progress").hidden = false; updateProgress(value); } };\n' });
          return;
        }
        await route.continue();
      });
      const page = await context.newPage(); page.on('pageerror', error => errors.push(error.message));
      await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: 30000 });
      await page.locator('#language').selectOption(language);
      await tokens(page, scheme, width, language);
      for (const operation of ['encrypt', 'decrypt', 'verify']) {
        await page.locator(`#${operation}-tab`).click();
        await page.evaluate(() => { window.uiThemeFixture.queue.tasks = []; window.uiThemeFixture.renderTasks(); window.uiThemeFixture.setSources([]); document.querySelector('#job-progress').hidden = true; });
        await audit(page, `${operation}-empty`, width, scheme, language);
        await page.locator('#manual-sources > summary').click();
        await page.locator('#input-path').fill('D:\\ui-theme-fixture\\资料\\alpha.txt\nD:\\ui-theme-fixture\\资料\\alpha.txt');
        await page.locator('#password').fill('UI theme fixture only');
        if (operation === 'encrypt') await page.locator('#confirm-password').fill('UI theme fixture only');
        await page.evaluate(operation => {
          const queue = window.uiThemeFixture.queue;
          queue.tasks = ['succeeded', 'failed', 'running', 'waiting'].map((status, index) => ({ id: `theme-${index}`, input: `D:\\ui-theme-fixture\\${status}.fse`, output: null, operation, status, createdAt: 1793095200000, endedAt: status === 'running' || status === 'waiting' ? undefined : 1793095201000, code: status === 'failed' ? 'authentication' : undefined }));
          window.uiThemeFixture.renderTasks();
          document.querySelector('#history-panel').open = true;
          window.uiThemeFixture.setProgress({ phase: 'preparing', completed: 21, total: 64, current: 'UI theme fixture only' });
          if (operation === 'verify') window.uiThemeFixture.setPreview({ input: 'D:\\ui-theme-fixture\\preview.fse', value: { format_version: 1, file_count: 1, directory_count: 0, content_bytes: 42, total_entries: 1, truncated: false, entries: [{ path: '资料 / report.txt', size: 42, directory: false }] } });
        }, operation);
        await audit(page, `${operation}-synthetic-statuses`, width, scheme, language);
        await page.evaluate(() => { document.querySelector('#manual-sources').open = false; document.querySelector('#package-preview').hidden = true; });
      }
      await page.locator('#nav-attributes').click();
      await audit(page, 'protection-empty', width, scheme, language);
      await page.locator('#open-attribute-form').click();
      await page.evaluate(async () => {
        const { createRootRecord } = await import('/ui/attributes.ts');
        const { t } = await import('/ui/i18n.ts');
        const fail = () => { throw new Error('Theme fixture cannot change files'); };
        document.querySelector('#root-list').replaceChildren(...['consistent', 'changed', 'unavailable'].map((state, index) => createRootRecord({ id: `theme-root-${index}`, name: `${t('managedRoots')} ${index + 1}`, mode: 'locked', note: index ? t('notePlaceholder') : '', duplicate_name: index === 1 }, fail, fail, { verification: { state, checked_at: 1793095200000 }, verify: fail, saveNote: fail, noteDraft: index === 0 ? '' : undefined, feedback: { text: t(index === 2 ? 'error_service_unavailable' : 'verificationConsistent'), error: index === 2 }, busy: index === 2, aclRestoreConfirmation: index === 1 ? { confirm: fail, cancel: fail } : undefined })));
      });
      await audit(page, 'protection-synthetic-statuses', width, scheme, language);
      // 用真实服务组件展示只读快照和加载属性；所有变更和文件选择都显式拒绝。
      await page.evaluate(async () => {
        const { ManagedProtectionUi } = await import('/ui/managed.ts');
        const original = document.querySelector('#managed-service-panel'); original.remove();
        const container = document.createElement('section'); container.id = 'theme-service'; document.querySelector('#attributes-panel').append(container);
        const fail = () => { throw new Error('Theme fixture cannot invoke IPC or change files'); };
        window.themeServiceUi = new ManagedProtectionUi(container, { request: fail, run: fail, isDesktop: () => true, isBlocked: () => false, choose: fail });
        for (const element of [...document.querySelector('#attributes-panel').children]) if (element !== container) element.hidden = true;
      });
      for (const state of ['empty', 'success', 'warning', 'error', 'busy']) {
        await page.evaluate(state => {
          const ui = window.themeServiceUi;
          const root = { id: 'theme-service-root', name: 'Theme fixture / 服务状态', note: '', mode: 'locked', password_required: true, monitoring: state !== 'warning', unlocked_until: null, restoring: false, last_error: state === 'error' ? 'service_unavailable' : null, entries: 1 };
          ui.available = state !== 'error'; ui.stale = state === 'error'; ui.loading = state === 'busy';
          ui.snapshot = { registered: state === 'empty' ? 0 : 1, roots: state === 'empty' ? [] : [root] };
          ui.transport = { host: 'service', healthy: state !== 'warning', last_tick_error: state === 'warning' ? 'service_unavailable' : null };
          ui.feedback = state === 'error' ? { code: 'service_unavailable', error: true } : null;
          ui.editor = state === 'success' ? { kind: 'enroll' } : null;
          ui.render();
          document.querySelector('#theme-service .managed-service-help').open = true;
        }, state);
        await audit(page, `service-${state}`, width, scheme, language);
      }
    } finally { await context.close(); }
  }
  await forcedColors();
  assert.deepEqual(errors, []); passed = true;
} catch (error) { errors.push(String(error)); throw error; }
finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors,
    limitations: ['Synthetic records and service snapshots test presentation only; no IPC or successful file-operation simulation.', 'Computed styles cover visible text nodes, values, placeholders, field/button boundaries and keyboard focus in the listed views; this is not a proof for every possible DOM or service state.', 'Text-only buttons are identified by their AA text; native checkbox rendering, browser validation bubbles, Windows high contrast and native DPI require visual or native checks.', 'Background alpha is composited through ancestors; background images and translucent text containers fail for manual review rather than receiving a guessed contrast ratio.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`UI theme checks passed: ${checks.length}`);
