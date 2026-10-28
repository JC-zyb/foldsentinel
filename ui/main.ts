import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open, save } from '@tauri-apps/plugin-dialog';
import { detectLanguage, errorText, failureCode, language, setLanguage, t, type Language, type MessageKey } from './i18n';
import { batchSummary, HISTORY_KEY, MAX_SOURCES, inspectSources, loadHistory, mergeSourcePaths, serializeHistory, sourceKey, TaskQueue, type Operation, type Plan, type Progress, type Task, type VaultReport } from './tasks';
import { previewPage, type PackagePreview } from './preview';
import { ATTRIBUTE_PAGE_SIZE, createRootRecord, fillProtectionModes, syncRootControls, type ManagedRoot, type ProtectionMode, type RootVerification } from './attributes';
import { ProtectionRecords, runProtectionAction, runProtectionBatch, validProtectionNote, type ProtectionBatchResult } from './protection-workflow';
import { ManagedProtectionUi } from './managed';
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
      <div id="attribute-alert" class="notice recovery-alert" role="status" hidden><p id="attribute-alert-text"></p><button id="review-attributes" type="button" class="secondary" ${text('reviewAttributes')}></button></div>
      <div class="segmented" role="group"><button id="encrypt-tab" aria-pressed="true" ${text('encrypt')}></button><button id="decrypt-tab" aria-pressed="false" ${text('decrypt')}></button><button id="verify-tab" aria-pressed="false" ${text('verify')}></button></div>
      <div id="queue-overview" class="queue-overview" hidden><span id="queue-overview-text" role="status"></span><button id="review-queue" type="button" class="text-button" ${text('reviewQueue')}></button></div>
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
      <section class="records queue"><h2 ${text('queue')}></h2><div id="job-progress" class="job-progress" hidden><div class="records-heading"><strong id="progress-phase" role="status"></strong><button id="cancel-job" class="secondary" ${text('cancel')}></button></div><progress id="progress-bar" max="100"></progress><p id="progress-detail"></p><p id="progress-speed"></p><p id="progress-current" class="field-help"></p></div><div id="queue-controls" class="queue-controls"><button id="start-queue" class="primary" ${text('start')}></button><button id="pause-queue" class="secondary" ${text('pause')}></button></div><div id="batch-result" class="batch-result" hidden><p id="batch-summary" role="status"></p><button id="retry-batch" type="button" class="secondary" ${text('retryBatch')}></button></div><p id="queue-summary" role="status"></p><ul id="task-list"></ul><details id="history-panel" class="history-panel"><summary id="history-summary"></summary><div class="records-heading"><p class="field-help" ${text('queueHelp')}></p><button id="clear-history" type="button" class="text-button" ${text('clearHistory')}></button></div><ul id="history-list"></ul></details></section>
      <section class="records recovery"><div class="records-heading"><h2 ${text('jobs')}></h2><button id="refresh-jobs" class="text-button" ${text('refreshJobs')}></button></div><p class="field-help" ${text('jobsHelp')}></p><ul id="job-list"></ul></section>
    </section>
    <section id="attributes-panel" hidden>
      <div class="protection-toolbar"><div class="selection-actions"><button id="pick-attribute-file" type="button" class="primary" ${text('addProtectionFiles')}></button><button id="pick-attribute-folder" type="button" class="secondary" ${text('addProtectionFolders')}></button><button id="open-attribute-form" type="button" class="text-button" aria-expanded="false" aria-controls="attribute-form" ${text('manualProtection')}></button><button id="review-managed-service" type="button" class="text-button" ${text('managedTitle')}></button></div><button id="refresh-records" class="secondary" type="button" ${text('refresh')}></button></div>
      <div class="protection-description"><p class="field-help" ${text('managedRootsHelp')}></p><details><summary ${text('protectionDetails')}></summary><p class="notice attribute-notice" ${text('attributeNotice')}></p><p class="field-help" ${text('attributeHelp')}></p><p class="field-help" ${text('reapplyHelp')}></p></details><details class="protection-backup"><summary ${text('protectionBackup')}></summary><p class="field-help" ${text('protectionBackupHelp')}></p><div class="selection-actions"><button id="export-protection-backup" type="button" class="secondary" ${text('exportProtectionBackup')}></button><button id="import-protection-backup" type="button" class="secondary" ${text('importProtectionBackup')}></button></div><p id="protection-backup-feedback" class="protection-feedback" role="status" hidden></p><section id="backup-import-confirm" class="protection-confirm" hidden><p ${text('confirmProtectionImport')}></p><div class="selection-actions"><button id="confirm-protection-import" type="button" class="primary" ${text('confirmImport')}></button><button id="cancel-protection-import" type="button" class="secondary" ${text('cancel')}></button></div></section></details></div>
      <form id="attribute-form" class="panel protection-add" hidden><fieldset id="attribute-fields"><legend class="sr-only" ${text('attributes')}></legend><div class="protection-add-heading"><h2 ${text('manageAttributes')}></h2><button id="cancel-attribute-add" type="button" class="text-button" ${text('cancelAdd')}></button></div><p class="field-help" ${text('protectionDropHelp')}></p><ul id="protection-draft-list" class="protection-draft-list"></ul><details id="attribute-manual"><summary ${text('manualProtection')}></summary><label for="attribute-path" ${text('attributePath')}></label><input id="attribute-path" spellcheck="false" autocomplete="off"><button id="append-protection-path" type="button" class="text-button" ${text('appendProtectionPath')}></button></details>
        <label for="attribute-mode" ${text('policy')}></label><select id="attribute-mode"></select><p class="field-help" ${text('protectionApplyHelp')}></p><div id="protection-add-feedback" class="protection-feedback" role="status" aria-live="polite" hidden></div><ul id="protection-add-results" class="protection-item-results"></ul><div class="attribute-actions"><button id="apply-protection-draft" class="primary" type="submit" ${text('applyAttributes')}></button><button id="retry-protection-add" type="button" class="secondary" hidden ${text('retryFailedProtection')}></button></div>
      </fieldset></form>
      <section class="records saved-roots protection-records" aria-labelledby="roots-title"><div class="records-heading"><h2 id="roots-title" tabindex="-1" ${text('managedRoots')}></h2><form id="protection-search-form" class="protection-search" role="search"><label for="protection-search" class="sr-only" ${text('searchProtection')}></label><input id="protection-search" type="search" maxlength="160" autocomplete="off"><button id="search-protection" type="submit" class="secondary" ${text('search')}></button></form></div><p id="root-summary" role="status" class="field-help"></p><div id="records-feedback" class="protection-feedback" role="status" aria-live="polite" hidden></div><p class="field-help" ${text('verificationScope')}></p><div class="protection-bulk-toolbar"><label class="checkbox"><input id="select-protection-page" type="checkbox"><span ${text('selectProtectionPage')}></span></label><span id="selected-protection-count" role="status"></span><button id="verify-selected-protection" type="button" class="secondary" ${text('verifySelectedProtection')}></button><button id="restore-selected-protection" type="button" class="secondary" aria-controls="protection-restore-confirm" ${text('restoreSelectedProtection')}></button></div><section id="protection-restore-confirm" class="protection-confirm" hidden aria-labelledby="protection-restore-title"><h3 id="protection-restore-title" ${text('confirmSelectedRestore')}></h3><p id="protection-restore-count"></p><ul id="protection-restore-items"></ul><div class="selection-actions"><button id="confirm-selected-restore" type="button" class="primary" ${text('confirmRestore')}></button><button id="cancel-selected-restore" type="button" class="secondary" ${text('cancel')}></button></div></section><p id="protection-batch-feedback" class="protection-feedback" role="status" hidden></p><ul id="protection-batch-results" class="protection-item-results"></ul><div class="protection-columns" aria-hidden="true"><span ${text('rootName')}></span><span ${text('registeredMode')}></span><span ${text('verificationActions')}></span></div><ul id="root-list"></ul><div id="roots-pagination" class="selection-actions"><button id="roots-previous" class="secondary" type="button" ${text('previousPage')}></button><button id="roots-next" class="secondary" type="button" ${text('nextPage')}></button></div></section>
      <section id="managed-service-panel" aria-labelledby="managed-title"></section>
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
const recordState = new ProtectionRecords<Snapshot>();
let recordsQuery = '';
let protectionDraft: string[] = [];
const protectionDraftNotes = new Map<string, string>();
interface ProtectionAddItem { path: string; name: string; note: string }
let protectionAddResults: ProtectionBatchResult<ProtectionAddItem>[] = [];
let protectionRetryMode: ProtectionMode = 'hidden-locked';
let protectionPreview: { index: number; name: string; duplicate_name: boolean }[] = [];
let protectionPreviewBusy = false;
let protectionPreviewFailed = false;
let protectionPreviewGeneration = 0;
const selectedProtection = new Set<string>();
let restoreSelection: ManagedRoot[] = [];
let protectionBatchResults: ProtectionBatchResult<ManagedRoot>[] = [];
let protectionBatchKind: 'verify' | 'restore' = 'verify';
let protectionBatchFeedback: ProtectionFeedback | null = null;
let pendingBackupImport: string | null = null;
let backupFeedback: ProtectionFeedback | null = null;
let managedUi: ManagedProtectionUi | null = null;
let protectionBusyKey: MessageKey = 'workingProtection';
interface ProtectionFeedback { key?: MessageKey; values?: Record<string, string | number>; code?: string; error?: boolean }
const rootFeedback = new Map<string, ProtectionFeedback>();
const rootVerification = new Map<string, RootVerification>();
const aclRestoreReviews = new Map<string, string>();
const rootRowIds = new WeakMap<HTMLElement, string>();
const pendingRootModes = new Map<string, ProtectionMode>();
const rootNoteDrafts = new Map<string, string>();
let addFeedback: ProtectionFeedback | null = null;
let recordNotice: ProtectionFeedback | null = null;
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
  managedUi?.sync();
  element<HTMLFieldSetElement>('vault-fields').disabled = uiBusy;
  element<HTMLFieldSetElement>('attribute-fields').disabled = uiBusy || queue.running;
  for (const id of ['refresh-jobs', 'refresh-records', 'pick-attribute-file', 'pick-attribute-folder', 'open-attribute-form', 'search-protection', 'export-protection-backup', 'import-protection-backup', 'confirm-protection-import', 'cancel-protection-import']) element<HTMLButtonElement>(id).disabled = uiBusy || queue.running;
  input('protection-search').disabled = uiBusy || queue.running;
  const snapshot = recordState.snapshot;
  element<HTMLButtonElement>('roots-previous').disabled = uiBusy || queue.running || recordState.stale || !snapshot || snapshot.offset === 0;
  element<HTMLButtonElement>('roots-next').disabled = uiBusy || queue.running || recordState.stale || !snapshot || snapshot.offset + snapshot.roots.length >= snapshot.total;
  syncRootControls(element('root-list'), uiBusy || queue.running, recordState.stale);
  syncProtectionSelection();
  element<HTMLButtonElement>('apply-protection-draft').disabled = uiBusy || queue.running || protectionPreviewBusy || protectionPreviewFailed || protectionDraft.some(path => !validProtectionNote(protectionDraftNotes.get(sourceKey(path)) ?? ''));
  element<HTMLButtonElement>('retry-protection-add').disabled = uiBusy || queue.running || protectionPreviewBusy || protectionPreviewFailed || !protectionAddResults.some(result => !result.succeeded);
  document.querySelectorAll<HTMLButtonElement>('#job-list button').forEach(button => { button.disabled = uiBusy || queue.running; });
  element<HTMLButtonElement>('start-queue').disabled = uiBusy || queue.running || !queue.tasks.some(t => t.status === 'waiting');
  element<HTMLButtonElement>('pause-queue').disabled = !queue.running;
  element<HTMLButtonElement>('start-queue').hidden = queue.running;
  element<HTMLButtonElement>('pause-queue').hidden = !queue.running;
  element('queue-controls').hidden = !queue.running && !queue.tasks.some(task => task.status === 'waiting');
  element<HTMLButtonElement>('retry-batch').disabled = uiBusy || !queue.tasks.some(task => task.batchId === [...queue.tasks].reverse().find(item => item.batchId)?.batchId && task.status === 'failed');
  element<HTMLButtonElement>('preview-restore').disabled = uiBusy || queue.running;
  for (const mode of ['encrypt', 'decrypt', 'verify']) element<HTMLButtonElement>(`${mode}-tab`).disabled = uiBusy;
  element('activity').textContent = !isTauri() ? t('preview') : queue.running ? t('working') : uiBusy ? t(page === 'attributes' ? protectionBusyKey : 'workingGeneral') : t('ready');
}
async function runUi(action: () => Promise<void>): Promise<void> {
  if (uiBusy) return;
  if (!isTauri()) { showResult('desktopRequired', {}, true); return; }
  uiBusy = true; syncControls();
  try { await action(); } catch (error) { showError(error); } finally { uiBusy = false; syncControls(); }
}
function clearPasswords(): void {
  managedUi?.clearPasswords();
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
  managedUi?.translate();
  const policy = element<HTMLSelectElement>('attribute-mode');
  fillProtectionModes(policy, (policy.value || 'hidden-locked') as ProtectionMode);
  document.querySelectorAll<HTMLElement>('[data-i18n]').forEach(el => { el.textContent = t(el.dataset.i18n as MessageKey); });
  element<HTMLSelectElement>('language').value = language;
  element<HTMLTextAreaElement>('input-path').placeholder = t('sourcePlaceholder'); input('output-path').placeholder = t('outputPlaceholder');
  input('protection-search').placeholder = t('searchProtection');
  element('page-title').textContent = t(page); element('page-description').textContent = t(page === 'vault' ? 'vaultDescription' : 'attributesDescription');
  modeLabels(); renderSources(); renderTasks(); renderJobs(); renderRecords(); renderProtectionDraft(); renderProtectionBatchResults(); renderRestoreSelection(); renderProtectionFeedback('protection-backup-feedback', backupFeedback); renderResult(); renderDiagnostic(); renderPreview(); syncControls(); if (progress) updateProgress(progress);
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
  const queueText = t('queueSummary', { waiting: count('waiting'), running: count('running'), done: count('succeeded'), failed: count('failed'), stopped: count('cancelled') + count('interrupted') });
  element('queue-summary').textContent = queueText;
  element('queue-overview-text').textContent = count('waiting') && !queue.running ? t('queueNotStarted', { n: count('waiting') }) : t('queueRunningOverview');
  element('queue-overview').hidden = !queue.running && count('waiting') === 0;
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
  element('queue-summary').hidden = queue.tasks.length === 0;
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

interface Snapshot { roots: ManagedRoot[]; total: number; overall_total: number; offset: number }
function protectionText(value: ProtectionFeedback): string { return value.code ? errorText(value.code) : t(value.key!, value.values); }
function renderProtectionFeedback(id: string, value: ProtectionFeedback | null): void {
  const box = element(id); box.hidden = !value;
  box.textContent = value ? protectionText(value) : '';
  box.classList.toggle('error', !!value?.error);
}
function renderRecordsFeedback(): void {
  const feedback: ProtectionFeedback | null = recordState.loading ? { key: 'refreshingProtection' }
    : recordState.stale && recordNotice?.key !== 'protectionSucceededRefreshFailed' ? { key: recordState.snapshot ? 'recordsRefreshFailed' : 'recordsLoadFailed', error: true } : recordNotice;
  renderProtectionFeedback('records-feedback', feedback);
  element('root-list').setAttribute('aria-busy', String(recordState.loading));
  syncControls();
}
async function refreshRecords(offset = recordState.snapshot?.offset ?? 0): Promise<void> {
  aclRestoreReviews.clear();
  closeRestoreSelection();
  const loading = recordState.refresh(() => invoke<Snapshot>('protection_status', { offset, query: recordsQuery }));
  renderRecordsFeedback();
  try { await loading; renderRecords(); }
  finally { renderRecordsFeedback(); }
}
function renderRecords(): void {
  const snapshot = recordState.snapshot;
  const roots = element('root-list'); roots.replaceChildren(); element('root-summary').textContent = '';
  element('attribute-alert').hidden = !snapshot?.overall_total;
  if (!snapshot) { renderRecordsFeedback(); return; }
  const visibleIds = new Set(snapshot.roots.map(root => root.id));
  for (const id of selectedProtection) if (!visibleIds.has(id)) selectedProtection.delete(id);
  element('attribute-alert-text').textContent = t('managedRootAttention', { n: snapshot.overall_total });
  element('root-summary').textContent = t('rootSummary', { total: snapshot.total, from: snapshot.total ? snapshot.offset + 1 : 0, to: snapshot.offset + snapshot.roots.length });
  if (!snapshot.roots.length) { const empty = document.createElement('li'); empty.className = 'empty-records'; empty.textContent = t(recordsQuery ? 'noProtectionMatches' : 'emptyRoots'); roots.append(empty); }
  for (const root of snapshot.roots) {
    const row = createRootRecord(root,
    () => { void changeProtection(root.id); }, mode => { void changeProtection(root.id, mode); }, {
      verify: () => { void verifyRoot(root.id); },
      selected: selectedProtection.has(root.id),
      select: selected => { if (selected) selectedProtection.add(root.id); else selectedProtection.delete(root.id); closeRestoreSelection(); syncProtectionSelection(); },
      saveNote: note => { void saveRootNote(root.id, note); },
      noteDraft: rootNoteDrafts.get(root.id),
      onNoteDraft: note => { if (note === undefined) rootNoteDrafts.delete(root.id); else rootNoteDrafts.set(root.id, note); },
      verification: rootVerification.get(root.id),
      aclRestoreConfirmation: aclRestoreReviews.has(root.id) ? {
        confirm: () => { void confirmAclRestore(root.id); },
        cancel: () => { aclRestoreReviews.delete(root.id); renderRecords(); restoreRecordFocus({ id: root.id, action: 'restore', index: 0 }, false); },
      } : undefined,
      feedback: rootFeedback.has(root.id) ? { text: protectionText(rootFeedback.get(root.id)!), error: rootFeedback.get(root.id)!.error } : undefined,
      busy: uiBusy || queue.running, stale: recordState.stale,
    });
    rootRowIds.set(row, root.id);
    const select = row.querySelector('select');
    const pending = pendingRootModes.get(root.id);
    if (select && pending && pending !== root.mode) { select.value = pending; select.dispatchEvent(new Event('change')); }
    roots.append(row);
  }
  element('roots-pagination').hidden = snapshot.total <= ATTRIBUTE_PAGE_SIZE;
  renderRecordsFeedback();
}
function syncProtectionSelection(): void {
  const blocked = uiBusy || queue.running || recordState.stale || recordState.loading;
  const roots = recordState.snapshot?.roots ?? [];
  const count = roots.filter(root => selectedProtection.has(root.id)).length;
  const select = input('select-protection-page');
  select.checked = !!roots.length && count === roots.length; select.indeterminate = count > 0 && count < roots.length;
  select.disabled = blocked || !roots.length;
  const selectedCount = element('selected-protection-count'); selectedCount.textContent = t('selectedProtectionCount', { n: count }); selectedCount.hidden = count === 0;
  for (const id of ['verify-selected-protection', 'restore-selected-protection']) {
    const button = element<HTMLButtonElement>(id); button.hidden = count === 0; button.disabled = blocked;
  }
  element<HTMLButtonElement>('confirm-selected-restore').disabled = blocked || !count;
  element<HTMLButtonElement>('cancel-selected-restore').disabled = uiBusy || queue.running;
}
function closeRestoreSelection(): void { restoreSelection = []; element('protection-restore-confirm').hidden = true; }
function renderRestoreSelection(): void {
  const list = element('protection-restore-items'); list.replaceChildren();
  for (const root of restoreSelection) { const item = document.createElement('li'); item.textContent = `${root.name}${root.note ? ` · ${root.note}` : ''}`; list.append(item); }
  element('protection-restore-count').textContent = t('confirmRestoreCount', { n: restoreSelection.length });
  element('protection-restore-confirm').hidden = !restoreSelection.length;
}
function renderProtectionBatchResults(): void {
  renderProtectionFeedback('protection-batch-feedback', protectionBatchFeedback);
  const list = element('protection-batch-results'); list.replaceChildren();
  for (const result of protectionBatchResults) {
    const row = document.createElement('li'); row.className = result.succeeded ? 'protection-item-success' : 'protection-item-failed';
    const status = result.succeeded ? t(protectionBatchKind === 'restore' ? 'protectionItemRestored' : 'protectionItemChecked') : errorText(failureCode(result.error));
    row.textContent = `${result.item.name}${result.item.note ? ` · ${result.item.note}` : ''} — ${status}`; list.append(row);
  }
}
async function runSelectedProtection(kind: 'verify' | 'restore', selected = recordState.snapshot?.roots.filter(root => selectedProtection.has(root.id)) ?? []): Promise<void> {
  if (uiBusy || queue.running || recordState.stale || !selected.length) return;
  protectionBatchKind = kind; closeRestoreSelection();
  protectionBusyKey = kind === 'verify' ? 'checkingProtection' : 'restoringProtection';
  protectionBatchFeedback = { key: protectionBusyKey }; protectionBatchResults = []; renderProtectionBatchResults();
  await runUi(async () => {
    protectionBatchResults = await runProtectionBatch(selected, async root => {
      if (kind === 'verify') rootVerification.set(root.id, await invoke<RootVerification>('verify_protection', { id: root.id }));
      else { await invoke('restore_attributes', { id: root.id }); selectedProtection.delete(root.id); rootVerification.delete(root.id); pendingRootModes.delete(root.id); rootNoteDrafts.delete(root.id); }
    }, result => {
      rootFeedback.set(result.item.id, result.succeeded ? { key: kind === 'verify' ? 'verificationDone' : 'attributeRestored' } : { code: failureCode(result.error), error: true });
    });
    const failed = protectionBatchResults.filter(result => !result.succeeded); let refreshed = true;
    try { await refreshRecords(); } catch (error) { refreshed = false; captureDiagnostic(error); }
    protectionBatchFeedback = { key: refreshed ? 'protectionBatchSummary' : 'protectionBatchSummaryRefreshFailed', values: { success: selected.length - failed.length, failed: failed.length }, error: !!failed.length || !refreshed };
    if (failed.length) captureDiagnostic(failed[0].error);
    renderProtectionBatchResults();
  });
}
input('select-protection-page').onchange = () => { for (const root of recordState.snapshot?.roots ?? []) { if (input('select-protection-page').checked) selectedProtection.add(root.id); else selectedProtection.delete(root.id); } closeRestoreSelection(); renderRecords(); };
element('verify-selected-protection').onclick = () => { void runSelectedProtection('verify'); };
element('restore-selected-protection').onclick = () => {
  restoreSelection = recordState.snapshot?.roots.filter(root => selectedProtection.has(root.id)) ?? [];
  if (!restoreSelection.length || uiBusy || queue.running || recordState.stale) return;
  renderRestoreSelection(); element('confirm-selected-restore').focus();
};
element('cancel-selected-restore').onclick = () => { closeRestoreSelection(); element('restore-selected-protection').focus(); };
element('confirm-selected-restore').onclick = () => { const selected = [...restoreSelection]; void runSelectedProtection('restore', selected); };
function recordFocus(): { id: string; action: string; index: number } | null {
  const active = document.activeElement as HTMLElement | null;
  const row = active?.closest<HTMLElement>('.root-record');
  const id = row && rootRowIds.get(row);
  return row && id ? { id, action: active?.dataset.rootAction ?? 'apply', index: Array.from(element('root-list').children).indexOf(row) } : null;
}
function restoreRecordFocus(focus: ReturnType<typeof recordFocus>, preventScroll = true): void {
  if (!focus || page !== 'attributes') return;
  const rows = Array.from(element('root-list').querySelectorAll<HTMLElement>('.root-record'));
  const row = rows.find(row => rootRowIds.get(row) === focus.id) ?? rows[Math.min(focus.index, rows.length - 1)];
  const target = row?.querySelector<HTMLElement>(`[data-root-action="${CSS.escape(focus.action)}"]:not(:disabled)`) ?? row?.querySelector<HTMLElement>('button:not(:disabled)');
  (target ?? element('roots-title')).focus({ preventScroll });
}
async function runRootAction(id: string, busyKey: MessageKey, action: () => Promise<void>, successKey: MessageKey, invalidate = false, reviewAclOnConflict = false): Promise<void> {
  if (uiBusy || queue.running || recordState.stale) return;
  if (!isTauri()) { rootFeedback.set(id, { key: 'desktopRequired', error: true }); renderRecords(); return; }
  aclRestoreReviews.delete(id);
  const focus = recordFocus(); protectionBusyKey = busyKey;
  rootFeedback.set(id, { key: busyKey }); renderRecords();
  let awaitingAclConfirmation = false;
  await runUi(async () => {
    const result = await runProtectionAction(action, () => refreshRecords());
    if (invalidate) { rootVerification.delete(id); if (result.operationSucceeded) pendingRootModes.delete(id); }
    if (!result.operationSucceeded && result.refreshSucceeded && reviewAclOnConflict
      && failureCode(result.operationError) === 'protection_conflict'
      && recordState.snapshot?.roots.some(root => root.id === id)) {
      try {
        const review = await invoke<{ token: string }>('review_restore_attributes', { id });
        if (!review || typeof review.token !== 'string' || !/^[a-f0-9]{64}$/.test(review.token)) throw { code: 'internal' };
        aclRestoreReviews.set(id, review.token);
        rootFeedback.delete(id);
        awaitingAclConfirmation = true;
      } catch (error) {
        result.operationError = error;
      }
    }
    if (awaitingAclConfirmation) {
      recordNotice = null;
      renderRecords();
      return;
    }
    const feedback: ProtectionFeedback = result.operationSucceeded ? { key: successKey }
      : { code: failureCode(result.operationError), error: true };
    rootFeedback.set(id, feedback);
    if (!result.operationSucceeded) { console.error('Protection operation failed', result.operationError); captureDiagnostic(result.operationError); }
    // 恢复成功后该行会消失，列表上方继续报告操作结果。
    recordNotice = result.operationSucceeded && !result.refreshSucceeded ? { key: 'protectionSucceededRefreshFailed', error: true }
      : recordState.snapshot?.roots.some(root => root.id === id) ? null : feedback;
    renderRecords();
  });
  restoreRecordFocus(awaitingAclConfirmation
    ? { id, action: 'cancel-acl-restore', index: focus?.index ?? 0 } : focus, !awaitingAclConfirmation);
}
async function changeProtection(id: string, mode?: ProtectionMode): Promise<void> {
  const restoring = !mode || mode === 'none';
  await runRootAction(id, restoring ? 'restoringProtection' : 'applyingProtection',
    async () => { if (mode) await invoke('set_protection_mode', { id, mode }); else await invoke('restore_attributes', { id }); },
    restoring ? 'attributeRestored' : 'attributeApplied', true, restoring);
}
async function confirmAclRestore(id: string): Promise<void> {
  const token = aclRestoreReviews.get(id);
  if (!token) return;
  await runRootAction(id, 'restoringProtection', async () => {
    await invoke('restore_attributes_after_review', { id, token });
    selectedProtection.delete(id); rootNoteDrafts.delete(id);
  }, 'attributeRestored', true);
}
async function verifyRoot(id: string): Promise<void> {
  await runRootAction(id, 'checkingProtection', async () => { rootVerification.set(id, await invoke<RootVerification>('verify_protection', { id })); }, 'verificationDone');
}
async function saveRootNote(id: string, note: string): Promise<void> {
  rootNoteDrafts.set(id, note);
  await runRootAction(id, 'savingProtectionNote', async () => { await invoke('set_protection_note', { id, note }); rootNoteDrafts.delete(id); }, 'protectionNoteSaved');
}
function showProtectionAdd(open = true): void {
  element('attribute-form').hidden = !open;
  element('open-attribute-form').setAttribute('aria-expanded', String(open));
}
function renderProtectionDraft(): void {
  const list = element('protection-draft-list');
  const active = document.activeElement;
  const activeNote = active instanceof HTMLInputElement && list.contains(active) ? { index: Array.from(list.querySelectorAll('input')).indexOf(active), start: active.selectionStart, end: active.selectionEnd } : null;
  list.replaceChildren();
  const names = protectionDraft.map(protectionOuterName);
  for (const [index, path] of protectionDraft.entries()) {
    const item = document.createElement('li'); item.className = 'protection-draft-item';
    const identity = document.createElement('div'); const name = document.createElement('strong'); name.textContent = names[index]; identity.append(name);
    if (protectionPreview.some(value => value.index === index && value.duplicate_name)) {
      const warning = document.createElement('p'); warning.className = 'root-duplicate'; warning.textContent = t('duplicateRootName'); identity.append(warning);
    }
    const label = document.createElement('label'); label.textContent = t('protectionNote');
    const note = document.createElement('input'); note.type = 'text'; note.maxLength = 160; note.autocomplete = 'off'; note.value = protectionDraftNotes.get(sourceKey(path)) ?? '';
    note.placeholder = t('notePlaceholder'); note.setAttribute('aria-label', `${t('protectionNote')}: ${names[index]}`); note.setCustomValidity(validProtectionNote(note.value) ? '' : t('notePlaceholder'));
    note.oninput = () => { protectionDraftNotes.set(sourceKey(path), note.value); note.setCustomValidity(validProtectionNote(note.value) ? '' : t('notePlaceholder')); note.setAttribute('aria-invalid', String(!validProtectionNote(note.value))); syncControls(); };
    label.append(note); identity.append(label);
    const remove = button('removeSource', () => { protectionDraft = protectionDraft.filter(value => value !== path); protectionDraftNotes.delete(sourceKey(path)); void previewProtectionDraft(); element('pick-attribute-file').focus(); });
    remove.setAttribute('aria-label', t('removeProtectionDraft', { name: names[index] }));
    item.append(identity, remove); list.append(item);
  }
  list.hidden = !protectionDraft.length;
  list.setAttribute('aria-busy', String(protectionPreviewBusy));
  if (protectionPreviewBusy || protectionPreviewFailed) {
    const status = document.createElement('li'); status.className = 'field-help'; status.textContent = t(protectionPreviewBusy ? 'checkingProtectionNames' : 'protectionNamesFailed');
    if (protectionPreviewFailed) { const retry = button('retryProtectionNames', () => { void previewProtectionDraft(); }); status.append(retry); }
    list.append(status);
  }
  renderProtectionFeedback('protection-add-feedback', addFeedback);
  const results = element('protection-add-results'); results.replaceChildren();
  for (const result of protectionAddResults) {
    const row = document.createElement('li'); row.className = result.succeeded ? 'protection-item-success' : 'protection-item-failed';
    row.textContent = `${result.item.name}${result.item.note ? ` · ${result.item.note}` : ''} — ${result.succeeded ? t(protectionRetryMode === 'none' ? 'protectionItemRestored' : 'protectionItemSucceeded') : errorText(failureCode(result.error))}`; results.append(row);
  }
  element('retry-protection-add').hidden = !protectionAddResults.some(result => !result.succeeded);
  syncControls();
  if (activeNote) { const note = list.querySelectorAll('input')[activeNote.index]; if (note && !note.disabled) { note.focus({ preventScroll: true }); note.setSelectionRange(activeNote.start, activeNote.end); } }
}
function protectionOuterName(path: string): string { return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || t('unnamedProtectionItem'); }
async function previewProtectionDraft(): Promise<void> {
  const generation = ++protectionPreviewGeneration;
  protectionPreview = []; protectionPreviewFailed = false;
  if (protectionDraft.length > MAX_SOURCES) { protectionPreviewBusy = false; protectionPreviewFailed = true; addFeedback = { key: 'queueLimit', error: true }; renderProtectionDraft(); return; }
  if (!protectionDraft.length || !isTauri()) { protectionPreviewBusy = false; renderProtectionDraft(); return; }
  protectionPreviewBusy = true; renderProtectionDraft();
  try {
    const preview = await invoke<typeof protectionPreview>('preview_protection_add', { paths: protectionDraft });
    if (generation === protectionPreviewGeneration) protectionPreview = preview;
  } catch (error) { if (generation === protectionPreviewGeneration) { protectionPreviewFailed = true; captureDiagnostic(error); } }
  finally { if (generation === protectionPreviewGeneration) { protectionPreviewBusy = false; renderProtectionDraft(); } }
}
function appendProtectionDraft(paths: string[]): void {
  protectionDraft = mergeSourcePaths(protectionDraft, paths);
  addFeedback = { key: 'protectionDraftReady', values: { n: protectionDraft.length } };
  showProtectionAdd(); void previewProtectionDraft();
}
for (const folder of [false, true]) element(folder ? 'pick-attribute-folder' : 'pick-attribute-file').onclick = () => { void runUi(async () => {
  try { const paths = await open({ directory: folder, multiple: true, title: t(folder ? 'pickFolders' : 'pickFiles') }); if (paths) { appendProtectionDraft(typeof paths === 'string' ? [paths] : paths); element('attribute-mode').focus(); } }
  catch (error) { console.error(error); addFeedback = { key: 'dialogFailed', error: true }; showProtectionAdd(); renderProtectionDraft(); }
}); };
element('open-attribute-form').onclick = () => { showProtectionAdd(); element<HTMLDetailsElement>('attribute-manual').open = true; input('attribute-path').focus(); };
element('cancel-attribute-add').onclick = () => { protectionDraft = []; protectionDraftNotes.clear(); protectionAddResults = []; protectionPreviewGeneration++; protectionPreviewBusy = false; protectionPreviewFailed = false; input('attribute-path').value = ''; addFeedback = null; showProtectionAdd(false); renderProtectionDraft(); element('pick-attribute-file').focus(); };
function appendManualProtectionPath(): boolean {
  const path = input('attribute-path').value.trim();
  if (!path) return false;
  appendProtectionDraft([path]); input('attribute-path').value = ''; return true;
}
element('append-protection-path').onclick = () => { if (!uiBusy && !queue.running) appendManualProtectionPath(); };
input('attribute-path').onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); appendManualProtectionPath(); } };
element<HTMLFormElement>('attribute-form').onsubmit = event => {
  event.preventDefault();
  if (uiBusy || queue.running) return;
  if (appendManualProtectionPath()) { addFeedback = { key: 'reviewProtectionNotes' }; renderProtectionDraft(); return; }
  void applyProtectionDraft();
};
element('retry-protection-add').onclick = () => { void applyProtectionDraft(true); };
async function applyProtectionDraft(retry = false): Promise<void> {
  if (uiBusy || queue.running) return;
  if (protectionPreviewBusy || protectionPreviewFailed) { await previewProtectionDraft(); return; }
  const paths = retry ? protectionAddResults.filter(result => !result.succeeded).map(result => result.item.path) : [...protectionDraft];
  const draft = inspectSources(paths.join('\n'));
  if (!paths.length || paths.length > MAX_SOURCES || draft.overlaps.length) {
    addFeedback = { key: !paths.length ? 'sourcesRequired' : paths.length > MAX_SOURCES ? 'queueLimit' : 'protectionOverlap', error: true }; renderProtectionDraft(); return;
  }
  if (!isTauri()) { addFeedback = { key: 'desktopRequired', error: true }; renderProtectionDraft(); return; }
  if (paths.some(path => !validProtectionNote(protectionDraftNotes.get(sourceKey(path)) ?? ''))) { addFeedback = { key: 'notePlaceholder', error: true }; renderProtectionDraft(); return; }
  const mode = retry ? protectionRetryMode : element<HTMLSelectElement>('attribute-mode').value as ProtectionMode;
  protectionRetryMode = mode;
  const items = paths.map(path => ({ path, name: protectionOuterName(path), note: (protectionDraftNotes.get(sourceKey(path)) ?? '').trim() }));
  protectionBusyKey = 'applyingProtection'; addFeedback = { key: 'applyingProtection' }; renderProtectionDraft();
  await runUi(async () => {
    const previous = retry ? protectionAddResults.filter(result => result.succeeded) : [];
    const results = await runProtectionBatch(items, item => invoke('attribute_operation', { path: item.path, mode, restore: false, note: mode === 'none' || !item.note ? undefined : item.note }));
    protectionAddResults = [...previous, ...results];
    const succeeded = results.filter(result => result.succeeded); const failed = results.filter(result => !result.succeeded);
    protectionDraft = protectionDraft.filter(path => !succeeded.some(result => sourceKey(result.item.path) === sourceKey(path)));
    succeeded.forEach(result => protectionDraftNotes.delete(sourceKey(result.item.path)));
    let refreshed = true;
    try { await refreshRecords(0); } catch (error) { refreshed = false; captureDiagnostic(error); }
    addFeedback = { key: refreshed ? 'protectionBatchSummary' : 'protectionBatchSummaryRefreshFailed', values: { success: succeeded.length, failed: failed.length }, error: !!failed.length || !refreshed };
    if (failed.length) captureDiagnostic(failed[0].error);
    await previewProtectionDraft();
  });
}
async function refreshProtectionPage(offset?: number): Promise<void> {
  protectionBusyKey = 'refreshingProtection'; recordNotice = null;
  await runUi(async () => { try { await refreshRecords(offset); } catch (error) { console.error('Protection list refresh failed', error); captureDiagnostic(error); } });
}

interface ProtectionBackupReport { roots: number; entries: number }
element('export-protection-backup').onclick = () => { void runUi(async () => {
  try {
    const path = await save({ title: t('exportProtectionBackup'), defaultPath: 'FoldSentinel-recovery.fsbackup.json', filters: [{ name: 'JSON', extensions: ['json'] }] });
    if (!path) return;
    const report = await invoke<ProtectionBackupReport>('export_protection_backup', { path });
    backupFeedback = { key: 'protectionBackupExported', values: { roots: report.roots, entries: report.entries } };
  } catch (error) { backupFeedback = { code: failureCode(error), error: true }; captureDiagnostic(error); }
  renderProtectionFeedback('protection-backup-feedback', backupFeedback);
}); };
element('import-protection-backup').onclick = () => { void runUi(async () => {
  try {
    const path = await open({ title: t('importProtectionBackup'), multiple: false, directory: false, filters: [{ name: 'JSON', extensions: ['json'] }] });
    if (!path || Array.isArray(path)) return;
    pendingBackupImport = path; element('backup-import-confirm').hidden = false;
  } catch (error) { backupFeedback = { code: failureCode(error), error: true }; captureDiagnostic(error); renderProtectionFeedback('protection-backup-feedback', backupFeedback); }
}).then(() => { if (pendingBackupImport) element('confirm-protection-import').focus(); }); };
element('cancel-protection-import').onclick = () => { pendingBackupImport = null; element('backup-import-confirm').hidden = true; element('import-protection-backup').focus(); };
element('confirm-protection-import').onclick = () => { void runUi(async () => {
  const path = pendingBackupImport; if (!path) return;
  try {
    const report = await invoke<ProtectionBackupReport>('import_protection_backup', { path });
    pendingBackupImport = null; element('backup-import-confirm').hidden = true;
    rootVerification.clear(); rootFeedback.clear(); pendingRootModes.clear(); rootNoteDrafts.clear(); selectedProtection.clear();
    backupFeedback = { key: 'protectionBackupImported', values: { roots: report.roots, entries: report.entries } };
    try { await refreshRecords(0); } catch (error) { backupFeedback = { key: 'protectionBackupImportedRefreshFailed', error: true }; captureDiagnostic(error); }
  } catch (error) { backupFeedback = { code: failureCode(error), error: true }; captureDiagnostic(error); }
  renderProtectionFeedback('protection-backup-feedback', backupFeedback);
}); };
element('refresh-records').onclick = () => { void refreshProtectionPage(); };
element('roots-previous').onclick = () => { void refreshProtectionPage(Math.max(0, (recordState.snapshot?.offset ?? 0) - ATTRIBUTE_PAGE_SIZE)); };
element('roots-next').onclick = () => { void refreshProtectionPage((recordState.snapshot?.offset ?? 0) + ATTRIBUTE_PAGE_SIZE); };
element<HTMLFormElement>('protection-search-form').onsubmit = event => { event.preventDefault(); if (uiBusy || queue.running) return; recordsQuery = input('protection-search').value.trim(); void refreshProtectionPage(0); };
element('root-list').addEventListener('change', event => {
  const target = event.target;
  if (!(target instanceof HTMLSelectElement) || !target.value) return;
  const row = target.closest<HTMLElement>('.root-record'); const id = row && rootRowIds.get(row);
  if (id) pendingRootModes.set(id, target.value as ProtectionMode);
});
element('review-queue').onclick = () => { element('task-list').scrollIntoView({ block: 'start' }); element('start-queue').focus({ preventScroll: true }); };
function showPage(): void {
  managedUi?.setVisible(page === 'attributes');
  for (const value of ['vault', 'attributes'] as const) {
    element(`${value}-panel`).hidden = value !== page; element(`nav-${value}`).classList.toggle('selected', value === page);
    if (value === page) element(`nav-${value}`).setAttribute('aria-current', 'page'); else element(`nav-${value}`).removeAttribute('aria-current');
  }
  element('page-title').textContent = t(page); element('page-description').textContent = t(page === 'vault' ? 'vaultDescription' : 'attributesDescription');
}
for (const value of ['vault', 'attributes'] as const) element(`nav-${value}`).onclick = () => { page = value; clearPasswords(); showPage(); if (value === 'attributes' && !uiBusy && !queue.running) void refreshProtectionPage(); };
element('review-attributes').onclick = () => { page = 'attributes'; clearPasswords(); showPage(); element('roots-title').focus(); void refreshProtectionPage(); };
element('review-managed-service').onclick = () => { managedUi?.clearPasswords(); element('managed-service-panel').scrollIntoView({ block: 'start' }); element('managed-title').focus({ preventScroll: true }); };

managedUi = new ManagedProtectionUi(element('managed-service-panel'), {
  request: command => invoke('managed_protection', { command }),
  run: action => { protectionBusyKey = 'workingServiceProtection'; return runUi(action); }, isDesktop: isTauri, isBlocked: () => uiBusy || queue.running,
  choose: async directory => {
    const path = await open({ directory, multiple: false, title: t(directory ? 'addProtectionFolders' : 'addProtectionFiles') });
    return typeof path === 'string' ? path : null;
  },
});
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
  void refreshProtectionPage();
  void getCurrentWebview().onDragDropEvent(event => {
    const payload = event.payload;
    const target = element(page === 'attributes' ? 'attributes-panel' : 'vault-form');
    for (const id of ['attributes-panel', 'vault-form']) element(id).classList.remove('drag-over');
    target.classList.toggle('drag-over', payload.type === 'enter' || payload.type === 'over');
    if (payload.type === 'drop' && !uiBusy && !(page === 'attributes' && queue.running)) {
      if (page === 'attributes') { appendProtectionDraft(payload.paths); element('attribute-mode').focus(); }
      else { appendSources(payload.paths); element('pick-file').focus(); }
    }
  }).catch(error => { console.error(error); showResult('dialogFailed', {}, true); });
}
