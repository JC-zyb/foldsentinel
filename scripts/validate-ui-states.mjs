import assert from 'node:assert/strict';
import { parseArgs } from 'node:util';
import { open } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// 使用已有浏览器验证真实界面入口和服务刷新状态，不接入桌面 IPC 或执行文件操作。
const { values } = parseArgs({ options: {
  playwright: { type: 'string' }, browser: { type: 'string' }, report: { type: 'string' },
  url: { type: 'string', default: 'http://127.0.0.1:1420/' }
} });
assert.ok(values.playwright && values.browser && values.report, '需要 --playwright、--browser 和 --report');
const url = new URL(values.url);
assert.ok(url.protocol === 'http:' && url.hostname === '127.0.0.1', '仅测试本机开发服务');
const reportFile = await open(resolve(values.report), 'wx');
const { chromium } = await import(pathToFileURL(resolve(values.playwright)).href);
const browser = await chromium.launch({ executablePath: resolve(values.browser), headless: true, args: ['--no-proxy-server'] });
const checks = [];
const errors = [];
let passed = false;
try {
  const context = await browser.newContext({ viewport: { width: 1080, height: 820 }, locale: 'zh-CN' });
  await context.addInitScript(() => localStorage.setItem('foldsentinel.history.v1', JSON.stringify([
    { id: 'history-focus-a', input: 'D:\\ui-state-fixture\\history-focus-a.fse', output: null, operation: 'verify', status: 'failed', code: 'cancelled', createdAt: 1, endedAt: 2 },
    { id: 'history-focus-b', input: 'D:\\ui-state-fixture\\history-focus-b.fse', output: null, operation: 'verify', status: 'failed', code: 'cancelled', createdAt: 3, endedAt: 4 }
  ])));
  await context.route('**/*', async route => {
    const requestUrl = new URL(route.request().url());
    if (requestUrl.origin !== url.origin) { await route.abort(); return; }
    if (requestUrl.pathname === '/ui/main.ts') {
      // 仅在测试响应中访问真实队列与反馈函数，生产代码不暴露测试接口，也不替换任何界面处理器。
      const response = await route.fetch();
      await route.fulfill({ response, body: await response.text() + '\nwindow.uiStateTaskQueue = queue; window.uiStateShowResult = showResult;\n' });
      return;
    }
    await route.continue();
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url.href, { waitUntil: 'domcontentloaded' });

  // 只切换展示属性来复现运行中开始按钮隐藏的条件，点击的仍为生产入口。
  await page.evaluate(() => {
    document.querySelector('#queue-overview').hidden = false;
    document.querySelector('#start-queue').hidden = true;
    document.querySelector('#queue-controls').hidden = false;
    document.querySelector('#pause-queue').hidden = false;
    document.querySelector('#job-progress').hidden = false;
  });
  await page.locator('#review-queue').click();
  assert.equal(await page.locator('#queue-title').evaluate(heading => document.activeElement === heading), true, '运行中查看队列聚焦可见标题');
  const queueTitle = await page.locator('#queue-title').boundingBox();
  assert.ok(queueTitle && queueTitle.y >= 0 && queueTitle.y + queueTitle.height <= 820, '队列标题完整进入视口');
  checks.push({ operation: 'review-running-queue', synthetic_presentation_only: true, visible_heading_focused: true });

  await page.locator('#history-summary').click();
  const historicalAction = page.locator('#history-list .task-item').filter({ hasText: 'history-focus-b.fse' }).locator('[data-task-action="retry"]');
  await page.evaluate(() => {
    window.uiStateTaskQueue.add([
      { input: 'D:\\ui-state-fixture\\task-a.fse', output: null, operation: 'verify' },
      { input: 'D:\\ui-state-fixture\\task-b.fse', output: null, operation: 'verify' }
    ], 'UI state fixture only');
  });
  await historicalAction.focus();
  await page.evaluate(() => {
    // 真实队列驱动开始/结束；执行器只停在可控取消边界，不模拟文件操作成功。
    window.pendingTaskRun = window.uiStateTaskQueue.start(() => new Promise((resolve, reject) => {
      window.cancelFixtureTask = () => reject(new Error('ui_state_cancelled'));
    }), () => 'cancelled');
  });
  assert.equal(await historicalAction.evaluate(button => document.activeElement === button), true, '任务开始重建后按任务 id 保留历史动作焦点');
  checks.push({ operation: 'task-start-rebuild', real_queue: true, same_history_action_focused: true });
  await page.evaluate(async () => {
    window.uiStateTaskQueue.pause();
    window.cancelFixtureTask();
    await window.pendingTaskRun;
  });
  assert.equal(await historicalAction.evaluate(button => document.activeElement === button), true, '任务结束重建后保留同一历史动作焦点');
  checks.push({ operation: 'task-end-rebuild', real_queue: true, same_history_action_focused: true });

  await page.locator('#task-list [data-task-action="cancel"]').focus();
  await page.evaluate(() => {
    window.pendingTaskRun = window.uiStateTaskQueue.start(() => new Promise((resolve, reject) => {
      window.cancelFixtureTask = () => reject(new Error('ui_state_cancelled'));
    }), () => 'cancelled');
  });
  assert.equal(await page.locator('#queue-title').evaluate(heading => document.activeElement === heading), true, '等待项开始后原取消动作消失时聚焦稳定队列标题');
  checks.push({ operation: 'task-start-removes-action', real_queue: true, heading_focus_fallback: true });
  await page.locator('#language').focus();
  await page.evaluate(async () => {
    window.uiStateTaskQueue.pause();
    window.cancelFixtureTask();
    await window.pendingTaskRun;
  });
  assert.equal(await page.locator('#language').evaluate(select => document.activeElement === select), true, '队列结束重建不抢占列表外焦点');
  checks.push({ operation: 'task-rebuild-external-focus', real_queue: true, external_focus_preserved: true });
  await page.evaluate(() => window.uiStateTaskQueue.add([
    { input: 'D:\\ui-state-fixture\\cancel-waiting.fse', output: null, operation: 'verify' }
  ], 'UI state fixture only'));
  await page.locator('#task-list [data-task-action="cancel"]').click();
  assert.equal(await page.locator('#queue-title').evaluate(heading => document.activeElement === heading), true, '真实取消等待项处理器重建后聚焦稳定队列标题');
  assert.equal(await page.locator('#task-list [data-task-action="cancel"]').count(), 0, '取消项离开等待列表');
  checks.push({ operation: 'cancel-waiting-task', real_click_handler: true, heading_focus_fallback: true });

  await page.evaluate(() => {
    window.uiStateTaskQueue.add([
      { input: 'D:\\ui-state-fixture\\pause-a.fse', output: null, operation: 'verify' },
      { input: 'D:\\ui-state-fixture\\pause-b.fse', output: null, operation: 'verify' }
    ], 'UI state fixture only');
    window.startedPauseTasks = [];
    window.holdPauseTask = task => {
      window.startedPauseTasks.push(task.id);
      return new Promise((resolve, reject) => {
        window.cancelPauseTask = () => reject(new Error('ui_state_cancelled'));
      });
    };
    window.pendingPauseRun = window.uiStateTaskQueue.start(window.holdPauseTask, () => 'cancelled');
  });
  await page.locator('#pause-queue').click();
  const pauseStatus = page.locator('#queue-pause-status');
  assert.equal(await pauseStatus.isVisible(), true, '点击暂停后队列旁立即显示反馈');
  assert.match(await pauseStatus.textContent(), /将在当前任务结束后暂停/, '显示现有中文暂停说明');
  assert.equal(await pauseStatus.evaluate(status => document.activeElement === status && status.previousElementSibling.id === 'queue-controls'), true, '暂停反馈紧邻控制区且接收焦点');
  const pauseStatusBox = await pauseStatus.boundingBox();
  assert.ok(pauseStatusBox && pauseStatusBox.y >= 0 && pauseStatusBox.y + pauseStatusBox.height <= 820, '暂停反馈完整进入视口');
  assert.equal(await page.locator('#pause-queue').isDisabled(), true, '已接受暂停请求时禁用重复暂停');
  await page.locator('#language').selectOption('en');
  assert.match(await pauseStatus.textContent(), /The queue will pause after the current task/, '暂停状态同步切换英文');
  await page.locator('#language').selectOption('zh-CN');
  assert.match(await pauseStatus.textContent(), /将在当前任务结束后暂停/, '暂停状态同步切回中文');
  await page.evaluate(async () => { window.cancelPauseTask(); await window.pendingPauseRun; });
  assert.deepEqual(await page.evaluate(() => ({
    started: window.startedPauseTasks.length, waiting: window.uiStateTaskQueue.tasks.filter(task => task.status === 'waiting').length,
    running: window.uiStateTaskQueue.running, pauseRequested: window.uiStateTaskQueue.pauseRequested
  })), { started: 1, waiting: 1, running: false, pauseRequested: true }, '当前任务结束后保留下一等待项，不再启动');
  checks.push({ operation: 'queue-pause-feedback', real_click_handler: true, nearby_status_visible: true, bilingual_status: true, duplicate_pause_disabled: true, next_task_not_started: true });
  await page.evaluate(() => {
    window.pendingPauseRun = window.uiStateTaskQueue.start(window.holdPauseTask, () => 'cancelled');
  });
  assert.equal(await pauseStatus.isHidden(), true, '恢复队列后清除暂停提示');
  assert.equal(await pauseStatus.textContent(), '', '恢复队列后清空暂停提示内容');
  assert.deepEqual(await page.evaluate(() => ({ started: window.startedPauseTasks.length, pauseRequested: window.uiStateTaskQueue.pauseRequested })),
    { started: 2, pauseRequested: false }, '真实重新开始重置暂停状态并启动下一项');
  await page.evaluate(async () => { window.cancelPauseTask(); await window.pendingPauseRun; });
  checks.push({ operation: 'queue-resume-feedback', real_queue: true, pause_status_cleared: true, waiting_task_started: true });

  for (const destination of ['attributes', 'vault']) {
    await page.evaluate(() => window.uiStateShowResult('paused'));
    assert.equal(await page.locator('#result').isVisible(), true);
    await page.locator(`#nav-${destination}`).click();
    assert.equal(await page.locator('#result').isHidden(), true, '切页清除上一页的全局操作反馈');
    assert.equal(await page.locator('#page-title').evaluate(title => document.activeElement === title), true);
  }
  checks.push({ operation: 'page-feedback-context', real_navigation_handlers: true, previous_page_result_cleared: true, destination_title_focused: true });

  await page.evaluate(async () => {
    const { ManagedProtectionUi } = await import('/ui/managed.ts');
    const container = document.createElement('section'); container.id = 'managed-state-check';
    document.querySelector('main').append(container);
    const response = { version: 1, error: null, transport: { host: 'service', healthy: true, last_tick_error: null }, data: {
      registered: 1, roots: [{ id: 'focus-fixture', name: '界面焦点夹具', note: '', mode: 'locked', password_required: true,
        monitoring: true, unlocked_until: null, restoring: false, last_error: null, entries: 1 }]
    } };
    window.serviceStateResponse = response;
    window.holdServiceRead = false;
    window.rejectServiceRead = false;
    window.serviceStateUi = new ManagedProtectionUi(container, {
      // 合成响应仅为只读状态组件夹具；变更、文件选择和文件操作一律拒绝。
      request: command => {
        if (command.action !== 'status') throw new Error('UI state check must not change service protection');
        if (!window.holdServiceRead) return Promise.resolve(response);
        return new Promise((resolve, reject) => {
          window.completeServiceRead = () => window.rejectServiceRead ? reject({ code: 'service_unavailable' }) : resolve(response);
        });
      },
      run: () => { throw new Error('UI state check must not run file operations'); },
      isDesktop: () => true, isBlocked: () => false,
      choose: () => { throw new Error('UI state check must not choose files'); }
    });
    await window.serviceStateUi.refresh();
    window.holdServiceRead = true;
  });
  const service = page.locator('#managed-state-check');
  const action = service.locator('[data-managed-action="unlock"]');
  const help = service.locator('.managed-service-help');
  await help.locator('summary').click();
  await action.focus();
  await page.evaluate(() => { window.pendingServiceRefresh = window.serviceStateUi.refresh(true); });
  assert.equal(await action.isDisabled(), true, '真实刷新期间禁用项目按钮');
  await page.evaluate(async () => { window.completeServiceRead(); await window.pendingServiceRefresh; });
  assert.equal(await action.evaluate(button => document.activeElement === button), true, '自动刷新结束恢复被禁用按钮的焦点');
  assert.equal(await help.getAttribute('open'), '', '真实刷新保留服务说明展开');
  checks.push({ operation: 'service-refresh', real_refresh_method: true, button_focus_restored: true, help_open_preserved: true });

  await action.focus();
  await page.evaluate(() => { window.pendingServiceRefresh = window.serviceStateUi.refresh(true); });
  await page.locator('#language').focus();
  await page.evaluate(async () => { window.completeServiceRead(); await window.pendingServiceRefresh; });
  assert.equal(await page.locator('#language').evaluate(select => document.activeElement === select), true, '异步刷新不抢回用户主动移出的焦点');
  checks.push({ operation: 'service-refresh-user-navigation', external_focus_preserved: true });

  await page.evaluate(async () => {
    const first = window.serviceStateResponse.data.roots[0];
    window.serviceStateResponse.data = { registered: 20, roots: Array.from({ length: 20 }, (_, index) => ({
      ...first, id: `editor-fixture-${index}`, name: `无输入确认表单 ${index}`, password_required: index === 0
    })) };
    window.holdServiceRead = false;
    await window.serviceStateUi.refresh(true);
    window.holdServiceRead = true;
  });
  for (const kind of ['remove', 'continuous']) {
    const origin = service.locator(`[data-managed-action="${kind}"]`).last();
    await origin.click();
    assert.equal(await service.locator('#managed-editor input').count(), 0, '无密码记录的确认表单没有输入框');
    const confirm = service.locator('#managed-editor [data-managed-action="submit"]');
    assert.equal(await confirm.evaluate(button => document.activeElement === button), true, '无输入确认表单打开后聚焦确认按钮');
    const confirmBox = await confirm.boundingBox();
    assert.ok(confirmBox && confirmBox.y >= 0 && confirmBox.y + confirmBox.height <= 820, '长列表上方的确认按钮进入视口');
    await service.locator('#managed-editor button[type="button"]').click();
    assert.equal(await origin.evaluate(button => document.activeElement === button), true, '取消无输入表单后返回同一记录动作');
    const originBox = await origin.boundingBox();
    assert.ok(originBox && originBox.y >= 0 && originBox.y + originBox.height <= 820, '取消后原记录动作进入视口');
    checks.push({ operation: `service-editor-${kind}`, real_component_handlers: true, no_input_editor_focus_visible: true, cancelled_origin_focus_visible: true });
  }

  await action.focus();
  await page.evaluate(() => {
    window.rejectServiceRead = true;
    window.pendingServiceRefresh = window.serviceStateUi.refresh(true);
  });
  await page.evaluate(async () => { window.completeServiceRead(); await window.pendingServiceRefresh; });
  assert.equal(await action.isDisabled(), true, '服务读取失败后项目动作仍禁用');
  assert.equal(await service.locator('#managed-title').evaluate(heading => document.activeElement === heading), true, '失败刷新聚焦可读标题而非禁用按钮');
  assert.equal(await service.locator('#managed-feedback').isVisible(), true, '刷新失败有明确反馈');
  checks.push({ operation: 'service-refresh-failure', stale_actions_disabled: true, heading_focus_fallback: true, failure_visible: true });

  assert.deepEqual(errors, []);
  await context.close();
  passed = true;
} catch (error) {
  errors.push(String(error));
  throw error;
} finally {
  await browser.close();
  await reportFile.writeFile(JSON.stringify({ schema_version: 1, tested_at: new Date().toISOString(), passed, checks, errors,
    limitations: ['Synthetic UI records, controlled queue cancellation and read-only service responses; no IPC, file operations or native Windows DPI coverage.'] }, null, 2) + '\n');
  await reportFile.close();
}
console.log(`UI dynamic state checks passed: ${checks.length}`);
