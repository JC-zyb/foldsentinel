import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open, save } from '@tauri-apps/plugin-dialog';
import { detectLanguage, errorText, failureCode, language, setLanguage, t, type Language, type MessageKey } from './i18n';
import { batchSummary, HISTORY_KEY, MAX_SOURCES, inspectSources, loadHistory, mergeSourcePaths, serializeHistory, sourceKey, TaskQueue, type Operation, type Plan, type Progress, type Task, type VaultReport } from './tasks';
import { previewPage, type PackagePreview } from './preview';
import './style.css';

const LANGUAGE_KEY = 'foldsentinel.language';
let startupError: MessageKey | null = null;
try { setLanguage(detectLanguage(localStorage.getItem(LANGUAGE_KEY), navigator.languages)); }
catch { setLanguage(detectLanguage(null, navigator.languages)); startupError = 'storageFailed'; }
const text = (key: MessageKey) => `data-i18n="${key}"`;
const shield = '<svg viewBox="0 0 32 32" fill="none" aria-hidden="true"><path d="M16 3 27 7v8c0 7-7 12-11 14C12 27 5 22 5 15V7l11-4Z" stroke="currentColor" stroke-width="2"/><path d="m11 16 3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>';
document.querySelector<HTMLDivElement>('#app')!.innerHTML = `
  <aside class="sidebar">
    <div class="brand"><span class="brand-icon">${shield}</span><div>FoldSentinel<small ${text('brand')}></small></div></div>
    <p class="nav-label" ${text('local')}></p>
    <nav><button id="nav-vault" class="nav-item selected" aria-current="page" ${text('vault')}></button><button id="nav-attributes" class="nav-item" ${text('attributes')}></button></nav>
    <div class="sidebar-footer"><span class="local-dot"></span><span ${text('localOnly')}></span><small ${text('license')}></small></div>
  </aside>
  <main>
    <header><div><p class="eyebrow" ${text('eyebrow')}></p><h1 id="page-title"></h1><p id="page-description"></p></div><div class="language-control"><label for="language" ${text('language')}></label><select id="language"><option value="zh-CN">简体中文</option><option value="en">English</option></select></div></header>
    <section id="vault-panel">
      <div id="recovery-alert" class="notice recovery-alert" role="status" hidden><p id="recovery-alert-text"></p><button id="review-recovery" type="button" class="secondary" ${text('reviewRecovery')}></button></div>
      <div class="segmented" role="group"><button id="encrypt-tab" aria-pressed="true" ${text('encrypt')}></button><button id="decrypt-tab" aria-pressed="false" ${text('decrypt')}></button><button id="verify-tab" aria-pressed="false" ${text('verify')}></button></div>
      <form id="vault-form" class="panel"><fieldset id="vault-fields"><legend class="sr-only" ${text('vault')}></legend>
        <div class="section-heading"><span class="step">1</span><div><h2 ${text('selection')}></h2><p id="source-help"></p></div></div>
        <div class="source-picker"><p id="drop-help"></p><div class="selection-actions"><button id="pick-file" type="button" class="secondary" ${text('pickFiles')}></button><button id="pick-folder" type="button" class="secondary" ${text('pickFolders')}></button></div></div>
        <details id="manual-sources" class="manual-sources"><summary ${text('manualSources')}></summary><label for="input-path" ${text('source')}></label><textarea id="input-path" rows="3" spellcheck="false" autocomplete="off"></textarea></details>
        <section id="draft" class="draft" hidden><div class="records-heading"><strong id="draft-count" role="status"></strong><button id="clear-sources" type="button" class="text-button" ${text('clearSources')}></button></div><ul id="source-list"></ul><p id="source-warning" class="source-warning" role="status"></p></section>
        <div id="output-fields"><label for="output-path" ${text('outputDirectory')}></label><div class="path-row"><input id="output-path" spellcheck="false" autocomplete="off"><button id="pick-output" type="button" class="secondary" ${text('chooseDirectory')}></button></div><p class="field-help" ${text('outputHelp')}></p></div>
        <div class="divider"></div><div class="section-heading"><span class="step">2</span><div><h2 ${text('passwordTitle')}></h2><p ${text('passwordHelp')}></p></div></div>
        <div class="password-grid"><div><label for="password" ${text('password')}></label><input id="password" type="password" required maxlength="256" autocomplete="new-password"></div><div id="confirm-field"><label for="confirm-password" ${text('confirmPassword')}></label><input id="confirm-password" type="password" required maxlength="256" autocomplete="new-password"></div></div>
        <label class="checkbox"><input id="show-password" type="checkbox"><span ${text('showPassword')}></span></label><div class="notice" id="vault-notice"></div>
        <div class="divider"></div><div class="form-footer"><div class="section-heading"><span class="step">3</span><div><h2 ${text('reviewTitle')}></h2><p id="review-help"></p></div></div><button class="primary" type="submit" ${text('add')}></button></div>
        <details class="usage-details"><summary ${text('usageDetails')}></summary><p class="field-help" ${text('formatLimits')}></p><p class="field-help" ${text('passwordMemory')}></p></details>
      </fieldset></form>
      <section id="package-preview" class="panel preview-panel" hidden aria-labelledby="preview-title"><div class="records-heading"><h2 id="preview-title" ${text('previewTitle')}></h2><button id="close-preview" type="button" class="text-button" ${text('closePreview')}></button></div><p id="preview-package" class="preview-package"></p><p class="notice" ${text('previewAuthenticated')}></p><p id="preview-summary" class="preview-summary"></p><p id="preview-limit" class="notice" hidden></p><p id="preview-empty" class="field-help" hidden ${text('emptyPackage')}></p><div class="preview-table-scroll" tabindex="0" role="region" aria-labelledby="preview-title"><table id="preview-table"><caption class="sr-only" ${text('previewTitle')}></caption><thead><tr><th scope="col" ${text('previewPath')}></th><th scope="col" ${text('previewType')}></th><th scope="col" ${text('previewSize')}></th></tr></thead><tbody id="preview-entries"></tbody></table></div><div class="preview-pagination"><span id="preview-page" role="status"></span><div class="selection-actions"><button id="preview-previous" type="button" class="secondary" ${text('previousPage')}></button><button id="preview-next" type="button" class="secondary" ${text('nextPage')}></button><button id="preview-restore" type="button" class="primary" ${text('prepareRestore')}></button></div></div><p class="field-help" ${text('previewPrivacy')}></p></section>
      <section class="records queue"><h2 ${text('queue')}></h2><div id="job-progress" class="job-progress" hidden><div class="records-heading"><strong id="progress-phase" role="status"></strong><button id="cancel-job" class="secondary" ${text('cancel')}></button></div><progress id="progress-bar" max="100"></progress><p id="progress-detail"></p><p id="progress-speed"></p><p id="progress-current" class="field-help"></p></div><div class="queue-controls"><button id="start-queue" class="primary" ${text('start')}></button><button id="pause-queue" class="secondary" ${text('pause')}></button></div><div id="batch-result" class="batch-result" hidden><p id="batch-summary" role="status"></p><button id="retry-batch" type="button" class="secondary" ${text('retryBatch')}></button></div><p id="queue-summary" role="status"></p><ul id="task-list"></ul><details id="history-panel" class="history-panel"><summary id="history-summary"></summary><div class="records-heading"><p class="field-help" ${text('queueHelp')}></p><button id="clear-history" type="button" class="text-button" ${text('clearHistory')}></button></div><ul id="history-list"></ul></details></section>
      <section class="records recovery"><div class="records-heading"><h2 ${text('jobs')}></h2><button id="refresh-jobs" class="text-button" ${text('refreshJobs')}></button></div><p class="field-help" ${text('jobsHelp')}></p><ul id="job-list"></ul></section>
    </section>
    <section id="attributes-panel" hidden>
      <div class="notice attribute-notice" ${text('attributeNotice')}></div>
      <form id="attribute-form" class="panel"><fieldset id="attribute-fields"><legend class="sr-only" ${text('attributes')}></legend><h2 ${text('manageAttributes')}></h2><label for="attribute-path" ${text('attributePath')}></label><div class="path-row"><input id="attribute-path" required spellcheck="false" autocomplete="off"><button id="pick-attribute-file" class="secondary" type="button" ${text('pickFile')}></button><button id="pick-attribute-folder" class="secondary" type="button" ${text('pickFolder')}></button></div>
        <label for="attribute-mode" ${text('policy')}></label><select id="attribute-mode"><option value="hidden-readonly" ${text('hiddenReadonly')}></option><option value="hidden" ${text('hidden')}></option><option value="readonly" ${text('readonly')}></option></select><p class="field-help" ${text('attributeHelp')}></p><div class="attribute-actions"><button class="secondary" id="restore-attributes" type="button" ${text('restoreAttributes')}></button><button class="primary" type="submit" ${text('applyAttributes')}></button></div>
      </fieldset></form><div class="records"><div class="records-heading"><h2 ${text('records')}></h2><button id="refresh-records" class="text-button" ${text('refresh')}></button></div><p class="field-help" ${text('recordHelp')}></p><ul id="record-list"></ul><p id="state-location" class="field-help"></p></div>
    </section>
    <div id="result" role="status" aria-live="polite" tabindex="-1" hidden></div><section id="diagnostic" class="diagnostic" hidden><details><summary ${text('errorDetails')}></summary><p class="field-help" ${text('diagnosticPrivacy')}></p><pre id="diagnostic-text"></pre><div class="selection-actions"><button id="copy-diagnostic" type="button" class="secondary" ${text('copyDetails')}></button><button id="export-diagnostic" type="button" class="secondary" ${text('exportDetails')}></button></div><p id="diagnostic-status" role="status"></p></details></section><footer class="main-footer"><span id="activity"></span><span ${text('keepSource')}></span></footer>
  </main>`;

function element<T extends HTMLElement = HTMLElement>(id: string): T { return document.getElementById(id) as T; }
const input = (id: string) => element<HTMLInputElement>(id);
// 用应用语言报告必填错误，避免气泡语言受 WebView2 系统区域设置影响。
document.querySelectorAll('form').forEach(form => {
  form.addEventListener('invalid', event => {
    const field = event.target;
    if ((field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) && field.validity.valueMissing) {
      event.preventDefault(); showResult('requiredField', {}, true); field.focus();
    }
  }, true);
});
let operation: Operation = 'encrypt';
let page: 'vault' | 'attributes' = 'vault';
let uiBusy = false;
let historyBroken = false;
let cancelling = false;
let progress: Progress | null = null;
let startedAt = 0;
let activeTask: Task | null = null;
let lastResult: { key?: MessageKey; values?: Record<string, string | number>; code?: string; error: boolean } | null = null;
let jobs: RecoveryEntry[] = [];
let diagnostic: DiagnosticInput | null = null;
let diagnosticText = '';
let diagnosticStatus: MessageKey | null = null;
let packagePreview: { input: string; value: PackagePreview } | null = null;
let previewPageIndex = 0;
let snapshot: Snapshot | null = null;
let initialTasks: Task[] = [];
try { initialTasks = loadHistory(localStorage.getItem(HISTORY_KEY)); }
catch { historyBroken = true; startupError = 'historyInvalid'; }
let lastFailureId = [...initialTasks].reverse().find(task => task.status === 'failed')?.id;
const queue = new TaskQueue(initialTasks, tasks => {
  try { localStorage.setItem(HISTORY_KEY, serializeHistory(tasks)); }
  catch { throw new Error('storage_failed'); }
}, () => { renderTasks(); syncControls(); });

function showResult(key: MessageKey, values: Record<string, string | number> = {}, error = false): void {
  lastResult = { key, values, error }; renderResult(); element('result').focus();
}
function showError(error: unknown): void {
  console.error('FoldSentinel operation failed', error);
  if (error instanceof Error && error.message === 'storage_failed') { showResult('storageFailed', {}, true); return; }
  if (error instanceof Error && error.message === 'queue_limit') { showResult('queueLimit', {}, true); return; }
  lastResult = { code: failureCode(error), error: true }; captureDiagnostic(error); renderResult(); element('result').focus();
}
function renderResult(): void {
  const box = element('result'); box.hidden = !lastResult;
  if (!lastResult) return;
  box.textContent = lastResult.code ? errorText(lastResult.code) : t(lastResult.key!, lastResult.values);
  box.className = lastResult.error ? 'result error' : 'result success'; box.setAttribute('role', lastResult.error ? 'alert' : 'status');
}
interface DiagnosticInput { code: string; operation: Operation | null; os_codes: number[] }
function captureDiagnostic(error: unknown, operation: Operation | null = null): void {
  if (failureCode(error) === 'cancelled') return;
  const osCodes = error && typeof error === 'object' && 'os_codes' in error && Array.isArray(error.os_codes) ? error.os_codes : [];
  diagnostic = { code: failureCode(error), operation, os_codes: osCodes.filter(code => Number.isInteger(code) && code >= 0 && code <= 65535).slice(0, 8) };
  diagnosticText = JSON.stringify(diagnostic, null, 2); diagnosticStatus = null; renderDiagnostic();
}
function renderDiagnostic(): void {
  element('diagnostic').hidden = diagnostic === null;
  element('diagnostic-text').textContent = diagnosticText;
  element('diagnostic-status').textContent = diagnosticStatus ? t(diagnosticStatus) : '';
}
element('copy-diagnostic').onclick = () => { void runUi(async () => {
  if (!diagnostic) return;
  try {
    const report = await invoke<string>('diagnostic_report', { request: diagnostic });
    await navigator.clipboard.writeText(report); diagnosticStatus = 'detailsCopied';
  } catch (error) { console.error(error); diagnosticStatus = 'detailsFailed'; }
  renderDiagnostic();
}); };
element('export-diagnostic').onclick = () => { void runUi(async () => {
  const request = diagnostic;
  if (!request) return;
  try {
    const path = await save({ title: t('exportDetails'), defaultPath: 'FoldSentinel-diagnostic.json', filters: [{ name: 'JSON', extensions: ['json'] }] });
    if (!path) return;
    await invoke('export_diagnostic', { path, request }); diagnosticStatus = 'detailsExported';
  } catch (error) { console.error(error); diagnosticStatus = 'detailsFailed'; }
  renderDiagnostic();
}); };
function syncControls(): void {
  element<HTMLFieldSetElement>('vault-fields').disabled = uiBusy;
  element<HTMLFieldSetElement>('attribute-fields').disabled = uiBusy || queue.running;
  for (const id of ['refresh-jobs', 'refresh-records']) element<HTMLButtonElement>(id).disabled = uiBusy || queue.running;
  document.querySelectorAll<HTMLButtonElement>('#job-list button').forEach(button => { button.disabled = uiBusy || queue.running; });
  element<HTMLButtonElement>('start-queue').disabled = uiBusy || queue.running || !queue.tasks.some(t => t.status === 'waiting');
  element<HTMLButtonElement>('pause-queue').disabled = !queue.running;
  element<HTMLButtonElement>('retry-batch').disabled = uiBusy || !queue.tasks.some(task => task.batchId === [...queue.tasks].reverse().find(item => item.batchId)?.batchId && task.status === 'failed');
  element<HTMLButtonElement>('preview-restore').disabled = uiBusy || queue.running;
  for (const mode of ['encrypt', 'decrypt', 'verify']) element<HTMLButtonElement>(`${mode}-tab`).disabled = uiBusy;
  element('activity').textContent = !isTauri() ? t('preview') : queue.running || uiBusy ? t('working') : t('ready');
}
async function runUi(action: () => Promise<void>): Promise<void> {
  if (uiBusy) return;
  if (!isTauri()) { showResult('desktopRequired', {}, true); return; }
  uiBusy = true; syncControls();
  try { await action(); } catch (error) { showError(error); } finally { uiBusy = false; syncControls(); }
}
function clearPasswords(): void {
  input('password').value = ''; input('confirm-password').value = ''; input('show-password').checked = false;
  input('password').type = 'password'; input('confirm-password').type = 'password';
}
function modeLabels(): void {
  for (const mode of ['encrypt', 'decrypt', 'verify']) { element(`${mode}-tab`).classList.toggle('active', operation === mode); element(`${mode}-tab`).setAttribute('aria-pressed', String(operation === mode)); }
  element('source-help').textContent = t(operation === 'encrypt' ? 'sourceHelp' : 'packageHelp');
  element('drop-help').textContent = t(operation === 'encrypt' ? 'dropHelp' : 'packageDropHelp');
  element('review-help').textContent = t(operation === 'verify' ? 'verifyReviewHelp' : 'reviewHelp');
  element('pick-folder').hidden = operation !== 'encrypt'; element('output-fields').hidden = operation === 'verify';
  element('confirm-field').hidden = operation !== 'encrypt'; input('confirm-password').required = operation === 'encrypt';
  input('password').autocomplete = operation === 'encrypt' ? 'new-password' : 'current-password';
  element('vault-notice').textContent = t(operation === 'verify' ? 'verifyNotice' : 'notice');
}
function setMode(mode: Operation): void { if (operation !== mode) { operation = mode; clearPasswords(); clearPreview(); } modeLabels(); }
function translate(): void {
  document.querySelectorAll<HTMLElement>('[data-i18n]').forEach(el => { el.textContent = t(el.dataset.i18n as MessageKey); });
  element<HTMLSelectElement>('language').value = language;
  element<HTMLTextAreaElement>('input-path').placeholder = t('sourcePlaceholder'); input('output-path').placeholder = t('outputPlaceholder');
  element('page-title').textContent = t(page); element('page-description').textContent = t(page === 'vault' ? 'vaultDescription' : 'attributesDescription');
  modeLabels(); renderSources(); renderTasks(); renderJobs(); renderRecords(); renderResult(); renderDiagnostic(); renderPreview(); syncControls(); if (progress) updateProgress(progress);
}
element<HTMLSelectElement>('language').onchange = () => {
  const next = element<HTMLSelectElement>('language').value as Language;
  try { localStorage.setItem(LANGUAGE_KEY, next); setLanguage(next); translate(); }
  catch { element<HTMLSelectElement>('language').value = language; showResult('storageFailed', {}, true); }
};
for (const mode of ['encrypt', 'decrypt', 'verify'] as Operation[]) element(`${mode}-tab`).onclick = () => setMode(mode);
input('show-password').onchange = () => { const type = input('show-password').checked ? 'text' : 'password'; input('password').type = type; input('confirm-password').type = type; };

function setSources(paths: string[]): void { clearPreview(); element<HTMLTextAreaElement>('input-path').value = paths.join('\n'); renderSources(); }
function appendSources(paths: string[]): void {
  const before = inspectSources(element<HTMLTextAreaElement>('input-path').value).paths;
  const merged = mergeSourcePaths(before, paths);
  setSources(merged);
  const duplicates = before.length + paths.length - merged.length;
  if (duplicates > 0) showResult('duplicatesIgnored', { n: duplicates });
}
function renderSources(): void {
  const draft = inspectSources(element<HTMLTextAreaElement>('input-path').value);
  element('draft').hidden = draft.paths.length === 0;
  element('draft-count').textContent = t('selectedCount', { n: draft.paths.length });
  const list = element('source-list'); list.replaceChildren();
  for (const path of draft.paths.slice(0, MAX_SOURCES)) {
    const item = document.createElement('li'); const label = document.createElement('span'); label.textContent = path;
    const remove = button('removeSource', () => {
      const next = inspectSources(element<HTMLTextAreaElement>('input-path').value).paths.filter(p => sourceKey(p) !== sourceKey(path));
      setSources(next); element('pick-file').focus();
    });
    remove.setAttribute('aria-label', t('removePath', { path })); item.append(label, remove); list.append(item);
  }
  element('source-warning').textContent = [draft.duplicates ? t('duplicatesWarning', { n: draft.duplicates }) : '', draft.overlaps.length ? t('overlapWarning') : '', draft.paths.length > MAX_SOURCES ? t('queueLimit') : ''].filter(Boolean).join('\n');
}
element('input-path').oninput = () => { clearPreview(); renderSources(); };
element('clear-sources').onclick = () => { setSources([]); element('pick-file').focus(); };
async function chooseSources(folder: boolean): Promise<void> {
  await runUi(async () => {
    try { const paths = await open({ directory: folder, multiple: true, title: t(folder ? 'pickFolders' : 'pickFiles') }); if (paths) appendSources(typeof paths === 'string' ? [paths] : paths); }
    catch (error) { console.error(error); showResult('dialogFailed', {}, true); }
  });
}
element('pick-file').onclick = () => { void chooseSources(false); };
element('pick-folder').onclick = () => { void chooseSources(true); };
element('pick-output').onclick = () => { void runUi(async () => {
  try { const path = await open({ directory: true, multiple: false, title: t('chooseDirectory') }); if (typeof path === 'string') input('output-path').value = path; }
  catch (error) { console.error(error); showResult('dialogFailed', {}, true); }
}); };

element<HTMLFormElement>('vault-form').onsubmit = event => {
  event.preventDefault();
  if (historyBroken) { showResult('historyInvalid', {}, true); return; }
  const draft = inspectSources(element<HTMLTextAreaElement>('input-path').value);
  if (!draft.paths.length) { showResult('sourcesRequired', {}, true); element('pick-file').focus(); return; }
  if (draft.paths.length > MAX_SOURCES) { showResult('queueLimit', {}, true); return; }
  if (draft.overlaps.length) { showResult('overlapWarning', {}, true); return; }
  if (operation === 'encrypt' && input('password').value !== input('confirm-password').value) { showResult('passwordMismatch', {}, true); input('confirm-password').focus(); return; }
  void runUi(async () => {
    const paths = draft.paths;
    const plans = await invoke<Plan[]>('prepare_tasks', { inputs: paths, operation, directory: operation === 'verify' ? null : input('output-path').value.trim() || null, reserved: queue.tasks.filter(t => ['waiting', 'running'].includes(t.status)).map(t => t.output).filter(Boolean) });
    queue.add(plans, input('password').value); clearPasswords(); setSources([]); showResult('added', { n: plans.length });
    element('task-list').scrollIntoView({ block: 'nearest' });
  });
};
function bytes(value: number): string {
  if (value < 1024) return `${new Intl.NumberFormat(language, { maximumFractionDigits: 0 }).format(value)} B`;
  const unit = Math.min(Math.floor(Math.log2(value) / 10), 4);
  return `${new Intl.NumberFormat(language, { minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(value / 1024 ** unit)} ${['B', 'KiB', 'MiB', 'GiB', 'TiB'][unit]}`;
}
function clearPreview(): void { packagePreview = null; previewPageIndex = 0; element('preview-entries').replaceChildren(); element('preview-package').textContent = ''; element('package-preview').hidden = true; }
function renderPreview(): void {
  element('package-preview').hidden = packagePreview === null;
  if (!packagePreview) return;
  const value = packagePreview.value;
  const current = previewPage(value, previewPageIndex); previewPageIndex = current.page;
  const number = (value: number) => new Intl.NumberFormat(language).format(value);
  element('preview-package').textContent = packagePreview.input.replace(/^\\\\\?\\/, '');
  element('preview-summary').textContent = t('previewSummary', { version: value.format_version, files: number(value.file_count), folders: number(value.directory_count), size: bytes(value.content_bytes) });
  element('preview-limit').hidden = !value.truncated;
  element('preview-limit').textContent = value.truncated ? t('previewTruncated', { total: number(value.total_entries), shown: number(value.entries.length) }) : '';
  element('preview-empty').hidden = value.total_entries !== 0;
  element('preview-table').hidden = current.entries.length === 0;
  const body = element('preview-entries'); body.replaceChildren();
  for (const entry of current.entries) {
    const row = document.createElement('tr');
    for (const content of [entry.path ?? t('singleFileContent'), t(entry.directory ? 'previewFolder' : 'previewFile'), entry.directory ? '—' : bytes(entry.size)]) {
      const cell = document.createElement('td'); cell.textContent = content; row.append(cell);
    }
    body.append(row);
  }
  element('preview-page').textContent = t('pageNumber', { page: number(current.page + 1), pages: number(current.pages) });
  element<HTMLButtonElement>('preview-previous').disabled = current.page === 0;
  element<HTMLButtonElement>('preview-next').disabled = current.page + 1 === current.pages;
}
element('preview-previous').onclick = () => { previewPageIndex--; renderPreview(); };
element('preview-next').onclick = () => { previewPageIndex++; renderPreview(); };
element('close-preview').onclick = () => { clearPreview(); element(`${operation}-tab`).focus(); };
element('preview-restore').onclick = () => { if (packagePreview) refill({ input: packagePreview.input, output: null, operation: 'decrypt' }); };
function updateProgress(value: Progress): void {
  progress = value;
  element('progress-phase').textContent = cancelling ? t('cancelling') : t(value.phase as MessageKey);
  const bar = element<HTMLProgressElement>('progress-bar');
  if (value.total > 0) bar.value = Math.min(100, value.completed / value.total * 100); else bar.removeAttribute('value');
  element('progress-detail').textContent = value.total > 0 ? t('progress', { done: bytes(value.completed), total: bytes(value.total), percent: Math.min(100, value.completed / value.total * 100).toFixed(1) }) : t('preparing');
  bar.setAttribute('aria-label', element('progress-detail').textContent ?? '');
  const elapsed = (performance.now() - startedAt) / 1000;
  const speed = value.completed / Math.max(elapsed, 0.01);
  const eta = Math.max(0, (value.total - value.completed) / speed);
  element('progress-speed').textContent = elapsed > 0.5 && speed > 0 && value.total > value.completed ? t('speed', { speed: bytes(speed), eta: t(eta > 60 ? 'minutes' : 'seconds', { n: Math.ceil(eta > 60 ? eta / 60 : eta) }) }) : t('estimating');
  element('progress-current').textContent = `${activeTask?.input ?? ''}${value.current ? '\n' + value.current : ''}`;
  element<HTMLButtonElement>('cancel-job').disabled = cancelling || ['committing', 'completed'].includes(value.phase);
  element('activity').textContent = t(['committing', 'completed'].includes(value.phase) ? 'committingHelp' : 'working');
}
async function perform(task: Task, password: string): Promise<VaultReport> {
  clearPreview();
  activeTask = task; cancelling = false; startedAt = performance.now(); element('job-progress').hidden = false;
  element('job-progress').scrollIntoView({ block: 'nearest' });
  const onProgress = new Channel<Progress>(); onProgress.onmessage = value => { if (activeTask?.id === task.id) updateProgress(value); };
  updateProgress({ phase: 'preparing', completed: 0, total: 0, current: '' });
  try {
    const report = await invoke<VaultReport>('vault_operation', { input: task.input, output: task.output ?? '', password, operation: task.operation, onProgress });
    if (task.operation === 'verify' && report.preview) {
      packagePreview = { input: task.input, value: report.preview }; renderPreview();
      element('package-preview').scrollIntoView({ block: 'nearest' });
    }
    return report;
  }
  catch (error) { captureDiagnostic(error, task.operation); throw error; }
  finally { activeTask = null; progress = null; element('job-progress').hidden = true; }
}
element('start-queue').onclick = () => {
  if (!isTauri()) { showResult('desktopRequired', {}, true); return; }
  void queue.start(perform, failureCode).catch(showError).finally(() => { void refreshJobs().catch(showError); });
};
element('pause-queue').onclick = () => { queue.pause(); showResult('paused'); };
element('cancel-job').onclick = () => {
  element<HTMLButtonElement>('cancel-job').disabled = true;
  void invoke<boolean>('cancel_vault').then(accepted => { cancelling = accepted; if (progress) updateProgress(progress); }).catch(showError);
};
element('clear-history').onclick = () => { try { queue.clearHistory(); historyBroken = false; lastResult = null; renderResult(); } catch (error) { showError(error); } };

function button(key: MessageKey, action: () => void): HTMLButtonElement {
  const button = document.createElement('button'); button.type = 'button'; button.className = 'secondary'; button.textContent = t(key); button.onclick = action; return button;
}
function refill(plan: Plan): void {
  page = 'vault'; showPage(); setMode(plan.operation); setSources([plan.input]);
  input('output-path').value = plan.output?.replace(/[\\/][^\\/]+$/, '') ?? '';
  clearPasswords(); showResult('retryHelp'); input('password').focus();
}
function renderTasks(): void {
  const list = element('task-list'); list.replaceChildren();
  const history = element('history-list'); history.replaceChildren();
  const count = (status: string) => queue.tasks.filter(t => t.status === status).length;
  element('queue-summary').textContent = t('queueSummary', { waiting: count('waiting'), running: count('running'), done: count('succeeded'), failed: count('failed'), stopped: count('cancelled') + count('interrupted') });
  const latestBatch = [...queue.tasks].reverse().find(task => task.batchId)?.batchId;
  const batch = queue.tasks.filter(task => task.batchId === latestBatch && latestBatch !== undefined);
  element('batch-result').hidden = batch.length === 0;
  if (batch.length) element('batch-summary').textContent = t('batchSummary', batchSummary(batch));
  element<HTMLButtonElement>('retry-batch').disabled = uiBusy || !batch.some(task => task.status === 'failed');
  element('retry-batch').onclick = () => {
    const failed = batch.filter(task => task.status === 'failed');
    if (!failed.length) return;
    page = 'vault'; showPage(); setMode(failed[0].operation); setSources(mergeSourcePaths([], failed.map(task => task.input)));
    const parents = new Set(failed.map(task => task.output?.replace(/[\\/][^\\/]+$/, '') ?? ''));
    input('output-path').value = parents.size === 1 ? [...parents][0] : '';
    clearPasswords(); showResult('retryHelp'); input('password').focus();
  };
  const pending = queue.tasks.filter(t => ['waiting', 'running'].includes(t.status));
  const finished = queue.tasks.filter(t => !['waiting', 'running'].includes(t.status)).reverse();
  const latestFailure = finished.find(task => task.status === 'failed');
  if (latestFailure && latestFailure.id !== lastFailureId) {
    element<HTMLDetailsElement>('history-panel').open = true;
    lastFailureId = latestFailure.id;
  }
  element('history-summary').textContent = t('historyCount', { n: new Intl.NumberFormat(language).format(finished.length) });
  for (const [target, emptyKey] of [[list, 'emptyPending'], [history, 'emptyHistory']] as const) {
    if ((target === list ? pending : finished).length) continue;
    const empty = document.createElement('li'); empty.className = 'empty-records'; empty.textContent = t(emptyKey); target.append(empty);
  }
  const ordered = [...pending, ...finished];
  for (const task of ordered) {
    const item = document.createElement('li'); item.className = 'task-item';
    const detail = document.createElement('div'); const title = document.createElement('strong'); title.textContent = `${t(task.operation)} · ${task.input.replace(/^\\\\\?\\/, '')}`;
    const output = document.createElement('p'); output.textContent = task.output ? t('output', { path: task.output.replace(/^\\\\\?\\/, '') }) : t('noOutput');
    const status = document.createElement('p'); status.className = `task-status ${task.status}`; status.textContent = `${t(task.status)} · ${new Date(task.endedAt ?? task.createdAt).toLocaleString(language)}`;
    detail.append(title, output, status);
    if (task.code) { const error = document.createElement('p'); error.className = 'task-error'; error.textContent = errorText(task.code); detail.append(error); }
    item.append(detail);
    if (task.status === 'waiting') item.append(button('cancel', () => { try { queue.cancelWaiting(task.id); } catch (error) { showError(error); } }));
    else if (['failed', 'cancelled', 'interrupted'].includes(task.status)) item.append(button('retry', () => refill(task)));
    if (task.status === 'succeeded' && task.output) item.append(button('openOutput', () => { void runUi(async () => { await invoke('open_output_folder', { path: task.output }); }); }));
    if (task.status === 'succeeded' && task.operation === 'verify') item.append(button('inspectAgain', () => refill(task)));
    (['waiting', 'running'].includes(task.status) ? list : history).append(item);
  }
}

interface InterruptedJob { id: string; input: string; output: string; staging: string; decrypt: boolean }
interface RecoveryEntry { id: string; status: 'active' | 'cleanable' | 'invalid'; record: InterruptedJob | null; code: string | null }
async function refreshJobs(): Promise<void> { jobs = await invoke<RecoveryEntry[]>('recovery_entries'); renderJobs(); }
function renderJobs(): void {
  const list = element('job-list'); list.replaceChildren();
  const attention = jobs.filter(job => job.status !== 'active').length;
  element('recovery-alert').hidden = attention === 0;
  element('recovery-alert-text').textContent = t('recoveryAttention', { n: attention });
  if (!jobs.length) { const empty = document.createElement('li'); empty.className = 'empty-records'; empty.textContent = t('emptyJobs'); list.append(empty); }
  for (const job of jobs) {
    const item = document.createElement('li'); item.className = 'recovery-item'; const detail = document.createElement('div');
    const title = document.createElement('strong'); title.textContent = job.record ? `${t(job.record.decrypt ? 'decrypt' : 'encrypt')} · ${job.record.input}` : job.id;
    const status = document.createElement('p'); status.textContent = t(job.status === 'active' ? 'recoveryActive' : job.status === 'cleanable' ? 'recoveryCleanable' : 'recoveryInvalid');
    detail.append(title, status);
    if (job.record) {
      const output = document.createElement('p'); output.textContent = t('output', { path: job.record.output });
      const staging = document.createElement('p'); staging.textContent = t('staging', { path: job.record.staging }); detail.append(output, staging);
    }
    item.append(detail);
    if (job.status === 'cleanable') {
      const cleanup = button('cleanup', () => { void runUi(async () => {
        try {
          const cleaned = await invoke<InterruptedJob>('cleanup_job', { id: job.id });
          refill({ input: cleaned.input, output: cleaned.output, operation: cleaned.decrypt ? 'decrypt' : 'encrypt' });
          showResult('cleanupDone');
        } finally { await refreshJobs(); }
      }); });
      cleanup.disabled = uiBusy || queue.running; item.append(cleanup);
    }
    list.append(item);
  }
}
element('refresh-jobs').onclick = () => { void runUi(refreshJobs); };
element('review-recovery').onclick = () => { element('job-list').scrollIntoView({ block: 'center' }); element('refresh-jobs').focus(); };

interface Snapshot { entries: { path: string; mode: string }[]; state_path: string }
async function refreshRecords(): Promise<void> { snapshot = await invoke<Snapshot>('protection_status'); renderRecords(); }
function renderRecords(): void {
  const list = element('record-list'); list.replaceChildren(); if (!snapshot) return;
  if (!snapshot.entries.length) { const empty = document.createElement('li'); empty.className = 'empty-records'; empty.textContent = t('emptyRecords'); list.append(empty); }
  for (const entry of snapshot.entries.slice(0, 200)) {
    const item = document.createElement('li'); const record = document.createElement('button'); record.type = 'button'; record.className = 'record';
    const path = document.createElement('span'); path.textContent = entry.path.replace(/^\\\\\?\\/, '');
    const mode = document.createElement('small'); mode.textContent = t(entry.mode === 'hidden-readonly' ? 'hiddenReadonly' : entry.mode === 'hidden' ? 'hidden' : 'readonly');
    record.append(path, mode); record.onclick = () => { input('attribute-path').value = entry.path; input('attribute-path').focus(); }; item.append(record); list.append(item);
  }
  element('state-location').textContent = t('stateLocation', { path: snapshot.state_path });
}
for (const folder of [false, true]) element(folder ? 'pick-attribute-folder' : 'pick-attribute-file').onclick = () => { void runUi(async () => {
  try { const path = await open({ directory: folder, multiple: false, title: t(folder ? 'pickFolder' : 'pickFile') }); if (typeof path === 'string') input('attribute-path').value = path; }
  catch (error) { console.error(error); showResult('dialogFailed', {}, true); }
}); };
async function attributes(restore: boolean): Promise<void> {
  if (queue.running || !input('attribute-path').reportValidity()) return;
  await runUi(async () => { const report = await invoke<{ changed: number; missing: number }>('attribute_operation', { path: input('attribute-path').value.trim(), mode: element<HTMLSelectElement>('attribute-mode').value, restore }); showResult('attributeDone', { changed: report.changed, missing: report.missing }); await refreshRecords(); });
}
element<HTMLFormElement>('attribute-form').onsubmit = event => { event.preventDefault(); void attributes(false); };
element('restore-attributes').onclick = () => { void attributes(true); };
element('refresh-records').onclick = () => { void runUi(refreshRecords); };
function showPage(): void {
  for (const value of ['vault', 'attributes'] as const) {
    element(`${value}-panel`).hidden = value !== page; element(`nav-${value}`).classList.toggle('selected', value === page);
    if (value === page) element(`nav-${value}`).setAttribute('aria-current', 'page'); else element(`nav-${value}`).removeAttribute('aria-current');
  }
  element('page-title').textContent = t(page); element('page-description').textContent = t(page === 'vault' ? 'vaultDescription' : 'attributesDescription');
}
for (const value of ['vault', 'attributes'] as const) element(`nav-${value}`).onclick = () => { page = value; clearPasswords(); showPage(); if (value === 'attributes') void runUi(refreshRecords); };

translate();
if (startupError) showResult(startupError, {}, true);
if (isTauri()) {
  void getCurrentWindow().onCloseRequested(event => {
    queue.pause(); clearPasswords();
    if (queue.running) {
      event.preventDefault();
      void invoke<boolean>('cancel_vault').then(accepted => { cancelling = accepted; if (progress) updateProgress(progress); }).catch(showError);
    }
  }).catch(showError);
  void refreshJobs().catch(showError);
  void getCurrentWebview().onDragDropEvent(event => {
    const payload = event.payload;
    element('vault-form').classList.toggle('drag-over', payload.type === 'enter' || payload.type === 'over');
    if (payload.type === 'drop' && !uiBusy) { page = 'vault'; showPage(); appendSources(payload.paths); element('pick-file').focus(); }
  }).catch(error => { console.error(error); showResult('dialogFailed', {}, true); });
}
