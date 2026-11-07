import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { mkdir, open, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 夹具只提供 SCM、只读服务状态和安装结果元数据；不实际安装/启动服务或执行文件操作。
const { values } = parseArgs({ options: {
  playwright: { type: 'string' }, browser: { type: 'string' }, report: { type: 'string' },
  url: { type: 'string', default: 'http://127.0.0.1:1420/' }
} });
assert.ok(values.playwright && values.browser && values.report, '需要 --playwright、--browser 和 --report');
const url = new URL(values.url);
assert.equal(url.protocol, 'http:'); assert.equal(url.hostname, '127.0.0.1');
const reportPath = resolve(values.report), screenshotRoot = join(dirname(reportPath), 'screenshots');
await mkdir(dirname(reportPath), { recursive: true });
const reportFile = await open(reportPath, 'wx');
await mkdir(screenshotRoot);
const version = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')).version;
const { chromium } = await import(pathToFileURL(resolve(values.playwright)).href);
const browser = await chromium.launch({ executablePath: resolve(values.browser), headless: true, args: ['--no-proxy-server'] });
const errors = [], requests = [], checks = [];
let passed = false;

async function setup(context) {
  await context.route('**/*', route => {
    if (new URL(route.request().url()).origin === url.origin) return route.continue();
    requests.push(route.request().url()); return route.abort();
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url.href, { waitUntil: 'domcontentloaded' });
  await page.locator('#nav-attributes').click();
  await page.locator('#protection-service-tab').click();
  await page.evaluate(async () => {
    const { ManagedProtectionUi } = await import('/ui/managed.ts');
    const previous = document.querySelector('#managed-service-panel');
    const container = document.createElement('section'); container.id = 'managed-service-panel';
    container.setAttribute('role', 'tabpanel'); container.setAttribute('aria-labelledby', 'protection-service-tab');
    previous.replaceWith(container);
    const fixture = { state: 'absent', success: false, desktop: true, blocked: false, probeFailure: false,
      holdProbe: false, holdStatus: false, probeCalls: 0, calls: 0, installCalls: 0,
      completeProbe: null, completeStatus: null, pendingInstall: null, ui: null };
    fixture.ui = new ManagedProtectionUi(container, {
      request: command => {
        fixture.calls += 1;
        if (command.action !== 'status') throw new Error('Only read-only status is permitted');
        const response = { version: 1, error: null, transport: { host: 'service', healthy: true, last_tick_error: null }, data: { registered: 1,
          roots: [{ id: 'fixture-one', name: '只读服务元数据', note: 'status fixture', mode: 'locked', password_required: true,
            monitoring: true, unlocked_until: null, restoring: false, last_error: null, entries: 1 }] } };
        if (!fixture.holdStatus) return fixture.success ? Promise.resolve(response) : Promise.reject({ code: 'service_unavailable' });
        return new Promise((resolve, reject) => {
          fixture.completeStatus = () => fixture.success ? resolve(response) : reject({ code: 'service_unavailable' });
        });
      },
      installationStatus: () => {
        fixture.probeCalls += 1;
        if (fixture.probeFailure) return Promise.reject(new Error('Explicit query-permission fixture'));
        if (!fixture.holdProbe) return Promise.resolve(fixture.state);
        return new Promise(resolve => { fixture.completeProbe = () => resolve(fixture.state); });
      },
      installService: () => {
        fixture.installCalls += 1;
        return new Promise((resolve, reject) => { fixture.pendingInstall = { resolve, reject }; });
      },
      run: async action => { fixture.blocked = true; try { await action(); } finally { fixture.blocked = false; } },
      isDesktop: () => fixture.desktop, isBlocked: () => fixture.blocked,
      choose: () => { throw new Error('No file selection permitted'); }
    });
    window.serviceStatusFixture = fixture;
  });
  return page;
}

async function language(page, value) {
  await page.locator('#nav-settings').click();
  await page.locator('#language').selectOption(value);
  await page.locator('#nav-attributes').click();
  await page.evaluate(() => window.serviceStatusFixture.ui.translate());
}

async function refresh(page, state) {
  await page.evaluate(async state => {
    const fixture = window.serviceStatusFixture; fixture.state = state;
    await fixture.ui.refresh();
  }, state);
}

async function beginInstall(page) {
  const before = await page.evaluate(() => window.serviceStatusFixture.installCalls);
  await page.locator('#managed-install').click();
  await page.waitForFunction(() => !!window.serviceStatusFixture.pendingInstall);
  assert.equal(await page.evaluate(() => window.serviceStatusFixture.installCalls), before + 1);
  assert.equal(await page.locator('#managed-install').isDisabled(), true);
  assert.equal(await page.locator('#managed-refresh').isDisabled(), true);
  assert.equal(await page.locator('#managed-add').isDisabled(), true);
  assert.equal(await page.locator('#managed-service-panel').getAttribute('aria-busy'), 'true');
  await page.locator('#managed-install').evaluate(button => { button.click(); button.click(); });
  assert.equal(await page.evaluate(() => window.serviceStatusFixture.installCalls), before + 1, '安装过程中重复点击不得发起第二次请求');
}

async function completeInstall(page, outcome, { reject = false, state = 'absent', success = false, holdStatus = false } = {}) {
  await page.evaluate(({ outcome, reject, state, success, holdStatus }) => {
    const f = window.serviceStatusFixture;
    f.state = state; f.success = success; f.holdStatus = holdStatus;
    const pending = f.pendingInstall; f.pendingInstall = null;
    if (reject) pending.reject(outcome); else pending.resolve(outcome);
  }, { outcome, reject, state, success, holdStatus });
  if (holdStatus) await page.waitForFunction(() => !!window.serviceStatusFixture.completeStatus);
  else await page.waitForFunction(() => !document.querySelector('#managed-refresh').disabled);
}

async function feedbackIs(page, key, role) {
  const expected = await page.evaluate(async key => (await import('/ui/i18n.ts')).t(key), key);
  const feedback = page.locator('#managed-install-feedback');
  assert.equal(await feedback.textContent(), expected);
  assert.equal(await feedback.getAttribute('role'), role);
}

try {
  const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const page = await setup(context), status = page.locator('#managed-service-status');
    assert.equal(await page.evaluate(() => window.serviceStatusFixture.installCalls), 0, '初始化不得自动请求安装');
    for (const [state, zh, en] of [
      ['absent', /尚未安装 SYSTEM/, /is not installed/], ['stopped', /当前已停止/, /installed but stopped/],
      ['starting', /正在启动/, /is starting/], ['running', /无法连接或验证/, /cannot connect or verify/],
      ['stopping', /正在停止/, /is stopping/], ['invalid', /不符合预期/, /unexpected install path/],
      ['inaccessible', /不能确认是否已安装/, /installation cannot be confirmed/]
    ]) {
      await refresh(page, state); assert.match(await status.textContent(), zh);
      assert.equal(await page.locator('#managed-add').isDisabled(), true);
      assert.match(await page.locator('#managed-root-list').textContent(), /不能确认/);
      assert.doesNotMatch(await status.textContent(), /^已连接/);
      assert.equal(await page.locator('#managed-install').count(), state === 'absent' ? 1 : 0, '仅明确未安装时显示安装入口');
      if (state === 'absent') assert.equal(await page.locator('#managed-install').isEnabled(), true);
      await language(page, 'en'); assert.match(await status.textContent(), en);
      await language(page, 'zh-CN');
      checks.push({ operation: state, bilingual: true, actions_disabled: true, records_remain_unknown: true });
    }
    await page.evaluate(() => { window.serviceStatusFixture.probeFailure = true; });
    await refresh(page, 'absent'); assert.match(await status.textContent(), /不能确认是否已安装/);
    assert.doesNotMatch(await status.textContent(), /尚未安装 SYSTEM/);
    await page.evaluate(() => { const f = window.serviceStatusFixture; f.probeFailure = false; f.state = 'malformed'; });
    await refresh(page, 'malformed'); assert.match(await status.textContent(), /不能确认是否已安装/);
    assert.equal(await page.locator('#managed-install').count(), 0, '查询失败不能当作未安装并提供可用安装入口');
    checks.push({ operation: 'probe-failure-and-malformed', never_absent: true });

    const probesBefore = await page.evaluate(() => window.serviceStatusFixture.probeCalls);
    await page.evaluate(async () => { const f = window.serviceStatusFixture; f.success = true; await f.ui.refresh(); });
    assert.match(await status.textContent(), /^已连接/); assert.equal(await page.locator('#managed-add').isEnabled(), true);
    assert.equal(await page.evaluate(() => window.serviceStatusFixture.probeCalls), probesBefore, 'RPC成功后不需SCM猜测连接状态');
    assert.match(await page.locator('#managed-root-list').textContent(), /只读服务元数据/);
    await page.evaluate(() => { window.serviceStatusFixture.success = false; });
    await refresh(page, 'stopped');
    assert.match(await status.textContent(), /上次记录/); assert.match(await status.textContent(), /当前已停止/);
    assert.match(await page.locator('#managed-root-list').textContent(), /只读服务元数据/);
    assert.equal(await page.locator('#managed-root-list [data-managed-action="unlock"]').isDisabled(), true);
    checks.push({ operation: 'authenticated-rpc-and-stale-snapshot', connected_only_after_rpc: true, previous_records_preserved: true, stale_actions_disabled: true });

    const refreshButton = page.locator('#managed-refresh');
    await refreshButton.focus();
    await page.evaluate(() => { const f = window.serviceStatusFixture; f.holdProbe = true; f.state = 'starting'; window.pendingServiceProbe = f.ui.refresh(true); });
    await page.waitForFunction(() => !!window.serviceStatusFixture.completeProbe);
    assert.equal(await refreshButton.isDisabled(), true);
    await page.locator('#nav-settings').click(); await page.locator('#language').focus();
    await page.evaluate(async () => { window.serviceStatusFixture.completeProbe(); await window.pendingServiceProbe; });
    assert.equal(await page.locator('#language').evaluate(element => document.activeElement === element), true);
    await page.locator('#nav-attributes').click();
    await refreshButton.focus();
    await page.evaluate(() => { const f = window.serviceStatusFixture; f.completeProbe = null; window.pendingServiceProbe = f.ui.refresh(true); });
    await page.waitForFunction(() => !!window.serviceStatusFixture.completeProbe);
    await page.evaluate(async () => { window.serviceStatusFixture.completeProbe(); await window.pendingServiceProbe; window.serviceStatusFixture.holdProbe = false; });
    assert.equal(await refreshButton.evaluate(element => document.activeElement === element), true);
    checks.push({ operation: 'pending-probe-focus', navigation_focus_preserved: true, disabled_refresh_focus_restored: true });
  } finally { await context.close(); }

  const installContext = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  try {
    const page = await setup(installContext), status = page.locator('#managed-service-status');
    await language(page, 'zh-CN'); await refresh(page, 'absent');
    const add = page.locator('#managed-add'), install = page.locator('#managed-install');

    await page.evaluate(() => { const f = window.serviceStatusFixture; f.blocked = true; f.ui.sync(); });
    assert.equal(await install.isDisabled(), true);
    await install.evaluate(button => button.click());
    assert.equal(await page.evaluate(() => window.serviceStatusFixture.installCalls), 0, '其他任务忙时不得发起服务安装');
    await page.evaluate(() => { const f = window.serviceStatusFixture; f.blocked = false; f.desktop = false; f.ui.translate(); });
    if (await install.count()) {
      assert.equal(await install.isDisabled(), true); await install.evaluate(button => button.click());
    }
    assert.equal(await page.evaluate(() => window.serviceStatusFixture.installCalls), 0, '浏览器预览不得请求服务安装');
    await page.evaluate(() => { const f = window.serviceStatusFixture; f.desktop = true; f.ui.translate(); });
    checks.push({ operation: 'install-entry-availability', explicit_absent_only: true, no_automatic_install: true, blocked_and_preview_disabled: true });

    await install.focus(); await beginInstall(page);
    await completeInstall(page, { status: 'cancelled' });
    await feedbackIs(page, 'managedServiceInstallCancelled', 'status');
    assert.equal(await install.isEnabled(), true); assert.equal(await add.isDisabled(), true);
    assert.equal(await install.evaluate(element => document.activeElement === element), true, '取消授权后恢复安装按钮焦点');
    assert.doesNotMatch(await status.textContent(), /^已连接/);
    checks.push({ operation: 'install-cancel-and-retry', no_duplicate_request: true, busy_controls_disabled: true, cancellation_not_success: true, install_focus_restored: true });

    await beginInstall(page);
    await page.locator('#nav-settings').click(); await page.locator('#language').focus();
    await completeInstall(page, { code: 'service_unavailable' }, { reject: true });
    assert.equal(await page.locator('#language').evaluate(element => document.activeElement === element), true, '安装失败不得夺回页面外焦点');
    await page.locator('#nav-attributes').click();
    await feedbackIs(page, 'managedServiceInstallFailed', 'alert');
    assert.equal(await install.isEnabled(), true); assert.equal(await add.isDisabled(), true);
    await refresh(page, 'absent'); await feedbackIs(page, 'managedServiceInstallFailed', 'alert');
    checks.push({ operation: 'install-failure', retry_available: true, no_connected_claim: true, result_survives_refresh: true, navigation_focus_preserved: true });

    await beginInstall(page); await language(page, 'en');
    assert.equal(await install.isDisabled(), true);
    assert.match(await install.textContent(), /installing/i);
    await completeInstall(page, { status: 'cancelled' });
    await feedbackIs(page, 'managedServiceInstallCancelled', 'status');
    await language(page, 'zh-CN'); await feedbackIs(page, 'managedServiceInstallCancelled', 'status');
    checks.push({ operation: 'install-language-during-request', busy_state_preserved: true, translated_result: true });

    await beginInstall(page); await completeInstall(page, { status: 'unexpected' });
    await feedbackIs(page, 'managedServiceInstallFailed', 'alert');
    assert.equal(await install.isEnabled(), true); assert.equal(await add.isDisabled(), true);
    checks.push({ operation: 'install-malformed-outcome', explicit_failure: true, retry_available: true });

    const callsBeforeInstall = await page.evaluate(() => window.serviceStatusFixture.calls);
    await beginInstall(page); await completeInstall(page, { status: 'installed' }, { state: 'running' });
    await feedbackIs(page, 'managedServiceInstallUnverified', 'alert');
    assert.equal(await add.isDisabled(), true); assert.equal(await install.count(), 0);
    assert.ok(await page.evaluate(() => window.serviceStatusFixture.calls) > callsBeforeInstall, '安装流程结束后必须重新发起生产RPC状态请求');
    assert.doesNotMatch(await status.textContent(), /^已连接/);
    await refresh(page, 'running'); await feedbackIs(page, 'managedServiceInstallUnverified', 'alert');
    await page.evaluate(() => { window.serviceStatusFixture.success = true; });
    await refresh(page, 'running'); await feedbackIs(page, 'managedServiceInstalled', 'status');
    assert.equal(await add.isEnabled(), true); assert.match(await status.textContent(), /^已连接/);
    checks.push({ operation: 'install-complete-unverified-rpc', installer_outcome_not_connection: true, later_rpc_updates_result: true, actions_only_after_rpc: true });

    await page.evaluate(() => { window.serviceStatusFixture.success = false; });
    await refresh(page, 'absent'); await beginInstall(page);
    await completeInstall(page, { status: 'installed' }, { state: 'running', success: true, holdStatus: true });
    assert.equal(await add.isDisabled(), true, '状态RPC尚未返回时不得启用登记');
    assert.equal(await page.locator('#managed-refresh').isDisabled(), true);
    await page.evaluate(() => {
      const f = window.serviceStatusFixture; f.holdStatus = false; f.completeStatus(); f.completeStatus = null;
    });
    await page.waitForFunction(() => !document.querySelector('#managed-add').disabled);
    await feedbackIs(page, 'managedServiceInstalled', 'status');
    assert.equal(await install.count(), 0); assert.match(await status.textContent(), /^已连接/);
    assert.equal(await page.locator('#managed-title').evaluate(element => document.activeElement === element), true, '安装入口消失后焦点应落在服务标题');
    checks.push({ operation: 'install-complete-pending-rpc', actions_wait_for_rpc: true, connection_verified: true, hidden_entry_focus_fallback: true });

    await page.evaluate(() => { window.serviceStatusFixture.success = false; });
    await refresh(page, 'running'); await feedbackIs(page, 'managedServiceInstallUnverified', 'alert');
    assert.equal(await add.isDisabled(), true); assert.doesNotMatch(await status.textContent(), /^已连接/);
    await page.evaluate(() => { window.serviceStatusFixture.success = true; });
    await refresh(page, 'running'); await feedbackIs(page, 'managedServiceInstalled', 'status');
    assert.equal(await add.isEnabled(), true);
    checks.push({ operation: 'install-connection-lost-and-restored', stale_success_corrected: true, actions_disabled_until_reconnected: true });
  } finally { await installContext.close(); }

  for (const width of [375, 720, 1080]) for (const scheme of ['light', 'dark']) for (const locale of ['zh-CN', 'en']) {
    const context = await browser.newContext({ viewport: { width, height: 820 }, colorScheme: scheme, locale });
    try {
      const page = await setup(context); await language(page, locale);
      await page.locator('#nav-settings').click(); await page.locator('#theme').selectOption(scheme);
      await page.locator('#nav-attributes').click();
      for (const state of ['absent', 'installing', 'running', 'invalid']) {
        await refresh(page, state === 'installing' ? 'absent' : state);
        if (state === 'installing') await beginInstall(page);
        await page.locator('#managed-service-panel').scrollIntoViewIfNeeded();
        const dimensions = await page.locator('#managed-service-panel').evaluate(panel => ({
          viewport: innerWidth, page: document.documentElement.scrollWidth,
          outside: [...panel.querySelectorAll('p, button, input')].filter(element => {
            const box = element.getBoundingClientRect();
            return box.width > 0 && box.height > 0 && (box.left < -1 || box.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 2);
          }).map(element => element.id || element.tagName)
        }));
        assert.ok(dimensions.page <= dimensions.viewport + 1, JSON.stringify(dimensions));
        assert.deepEqual(dimensions.outside, [], JSON.stringify({ width, scheme, locale, state, dimensions }));
        const screenshot = `${width}-${scheme}-${locale}-${state}.png`;
        await page.locator('#managed-service-panel').screenshot({ path: join(screenshotRoot, screenshot) });
        checks.push({ operation: 'layout', width, scheme, locale, state, screenshot, no_horizontal_overflow: true });
        if (state === 'installing') await completeInstall(page, { status: 'cancelled' });
      }
    } finally { await context.close(); }
  }
  assert.deepEqual(requests, []); assert.deepEqual(errors, []); passed = true;
} catch (error) { errors.push(String(error)); throw error; }
finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, version, tested_at: new Date().toISOString(), passed, checks, errors,
    blocked_requests: requests, limitations: ['Production component with explicit SCM, read-only service status and installer outcome metadata fixtures; no actual service installation, start or file action is performed.', 'SCM, UAC, real pipe identity and native Windows DPI require separate acceptance.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`Service status UI checks passed: ${checks.length}`);
