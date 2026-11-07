import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/plugin-dialog';
import { detectLanguage, errorText, failureCode, language, setLanguage, t, type Language, type MessageKey } from './i18n';
import { batchSummary, canRetryTask, filterHistory, HISTORY_KEY, MAX_SOURCES, inspectSources, loadHistory, mergeSourcePaths, outputParent, serializeHistory, sourceKey, TaskQueue, type Operation, type Plan, type Progress, type Task, type VaultReport } from './tasks';
import { previewPage, type PackagePreview } from './preview';
import { ATTRIBUTE_PAGE_SIZE, createRootRecord, fillProtectionModes, syncRootControls, type ManagedRoot, type ProtectionMode, type RootVerification } from './attributes';
import { ProtectionRecords, runProtectionAction, runProtectionBatch, validProtectionNote, type ProtectionBatchResult } from './protection-workflow';
import { ManagedProtectionUi } from './managed';
import { UpdatesUi, type UpdateCheck } from './updates';
import { DEFAULT_PREFERENCES, loadPreferences, PREFERENCES_KEY, serializePreferences, type Preferences } from './preferences';
import { SettingsUi, type WindowPreferences, type DataLocations } from './settings-ui';
import './style.css';

const LANGUAGE_KEY = 'foldsentinel.language';
const THEME_KEY = 'foldsentinel.theme';
type Theme = 'system' | 'light' | 'dark';
let startupError: MessageKey | null = null;
let theme: Theme = 'system';
let preferences: Preferences = { ...DEFAULT_PREFERENCES };
let preferencesBroken = false;
let settingsUi: SettingsUi | null = null;
let windowPreferences: WindowPreferences = { close_to_tray: false, start_minimized: false, language: 'en' };
let desktopBusy: boolean | null = null;
const outputDrafts: Partial<Record<Operation, string>> = {};
try {
  setLanguage(detectLanguage(localStorage.getItem(LANGUAGE_KEY), navigator.languages));
  const savedTheme = localStorage.getItem(THEME_KEY);
  if (savedTheme === 'light' || savedTheme === 'dark') theme = savedTheme;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme === 'system' ? 'light dark' : theme;
}
catch { setLanguage(detectLanguage(null, navigator.languages)); startupError = 'storageFailed'; }
try { preferences = loadPreferences(localStorage.getItem(PREFERENCES_KEY)); }
catch { preferencesBroken = true; startupError = 'settingsPreferencesInvalid'; }
document.documentElement.dataset.density = preferences.density;
document.documentElement.dataset.fontSize = preferences.fontSize;
const text = (key: MessageKey) => `data-i18n="${key}"`;
const shield = '<svg viewBox="0 0 32 32" fill="none" aria-hidden="true"><path d="M16 3 27 7v8c0 7-7 12-11 14C12 27 5 22 5 15V7l11-4Z" stroke="currentColor" stroke-width="2"/><path d="m11 16 3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>';
document.querySelector<HTMLDivElement>('#app')!.innerHTML = `
  <aside class="sidebar">
    <div class="brand"><span class="brand-icon">${shield}</span><div>FoldSentinel<small ${text('brand')}></small></div></div>
    <p id="nav-label" class="nav-label" ${text('local')}></p>
    <nav aria-labelledby="nav-label"><button id="nav-vault" class="nav-item selected" aria-current="page" ${text('vault')}></button><button id="nav-attributes" class="nav-item" ${text('attributes')}></button><button id="nav-settings" class="nav-item" ${text('settings')}></button></nav>
    <div class="sidebar-footer"><span class="local-dot"></span><span ${text('localOnly')}></span><small ${text('license')}></small></div>
  </aside>
  <main aria-labelledby="page-title">
    <header><div><p class="eyebrow" ${text('eyebrow')}></p><h1 id="page-title" tabindex="-1"></h1><p id="page-description"></p></div></header>
    <div id="result" role="status" aria-live="polite" tabindex="-1" hidden></div>
    <section id="vault-panel">
      <div id="recovery-alert" class="notice recovery-alert" role="status" hidden><p id="recovery-alert-text"></p><button id="review-recovery" type="button" class="secondary" ${text('reviewRecovery')}></button></div>
      <div id="attribute-alert" class="notice recovery-alert" role="status" hidden><p id="attribute-alert-text"></p><button id="review-attributes" type="button" class="secondary" ${text('reviewAttributes')}></button></div>
      <div class="segmented" role="group" aria-labelledby="operation-label"><span id="operation-label" class="sr-only" ${text('operationType')}></span><button id="encrypt-tab" aria-pressed="true" ${text('encrypt')}></button><button id="decrypt-tab" aria-pressed="false" ${text('decrypt')}></button><button id="verify-tab" aria-pressed="false" ${text('verify')}></button></div>
      <div id="queue-overview" class="queue-overview" hidden><span id="queue-overview-text" role="status"></span><button id="review-queue" type="button" class="text-button" ${text('reviewQueue')}></button></div>
      <form id="vault-form" class="panel"><fieldset id="vault-fields"><legend class="sr-only" ${text('vault')}></legend>
        <div class="vault-columns"><section class="vault-source" aria-labelledby="selection-title">
        <div class="section-heading"><span class="step">1</span><div><h2 id="selection-title" ${text('selection')}></h2><p id="source-help"></p></div></div>
        <div class="source-picker"><p id="drop-help"></p><div class="selection-actions"><button id="pick-file" type="button" class="secondary" ${text('pickFiles')}></button><button id="pick-folder" type="button" class="secondary" ${text('pickFolders')}></button></div></div>
        <details id="manual-sources" class="manual-sources"><summary ${text('manualSources')}></summary><label for="input-path" ${text('source')}></label><textarea id="input-path" rows="3" spellcheck="false" autocomplete="off"></textarea></details>
        <section id="draft" class="draft" hidden><div class="records-heading"><strong id="draft-count" role="status"></strong><button id="clear-sources" type="button" class="text-button" ${text('clearSources')}></button></div><ul id="source-list"></ul><p id="source-warning" class="source-warning" role="status"></p></section>
        <div id="output-fields"><label for="output-path" ${text('outputDirectory')}></label><div class="path-row"><input id="output-path" spellcheck="false" autocomplete="off" aria-describedby="output-help"><button id="pick-output" type="button" class="secondary" ${text('chooseDirectory')}></button></div><p id="output-help" class="field-help"></p></div>
        </section><section class="vault-password" aria-labelledby="password-title"><div class="section-heading"><span class="step">2</span><div><h2 id="password-title" ${text('passwordTitle')}></h2><p id="password-help"></p></div></div>
        <div class="password-grid"><div><label for="password" ${text('password')}></label><input id="password" type="password" required maxlength="256" autocomplete="new-password" aria-describedby="password-help"></div><div id="confirm-field"><label for="confirm-password" ${text('confirmPassword')}></label><input id="confirm-password" type="password" required maxlength="256" autocomplete="new-password"></div></div>
        <label class="checkbox"><input id="show-password" type="checkbox"><span ${text('showPassword')}></span></label><div class="notice" id="vault-notice"></div>
        </section></div>
        <div class="form-footer"><div class="section-heading"><span class="step">3</span><div><h2 ${text('reviewTitle')}></h2><p id="review-help"></p></div></div><button id="add-to-queue" class="primary" type="submit" ${text('add')}></button></div>
        <details class="usage-details"><summary ${text('usageDetails')}></summary><p class="field-help" ${text('formatLimits')}></p><p class="field-help" ${text('passwordMemory')}></p></details>
      </fieldset></form>
      <section id="package-preview" class="panel preview-panel" hidden aria-labelledby="preview-title"><div class="records-heading"><h2 id="preview-title" ${text('previewTitle')}></h2><button id="close-preview" type="button" class="text-button" ${text('closePreview')}></button></div><p id="preview-package" class="preview-package"></p><p class="notice" ${text('previewAuthenticated')}></p><p id="preview-summary" class="preview-summary"></p><p id="preview-limit" class="notice" hidden></p><p id="preview-empty" class="field-help" hidden ${text('emptyPackage')}></p><div class="preview-table-scroll" tabindex="0" role="region" aria-labelledby="preview-title"><table id="preview-table"><caption class="sr-only" ${text('previewTitle')}></caption><thead><tr><th scope="col" ${text('previewPath')}></th><th scope="col" ${text('previewType')}></th><th scope="col" ${text('previewSize')}></th></tr></thead><tbody id="preview-entries"></tbody></table></div><div class="preview-pagination"><span id="preview-page" role="status"></span><div class="selection-actions"><button id="preview-previous" type="button" class="secondary" ${text('previousPage')}></button><button id="preview-next" type="button" class="secondary" ${text('nextPage')}></button><button id="preview-restore" type="button" class="primary" ${text('prepareRestore')}></button></div></div><p class="field-help" ${text('previewPrivacy')}></p></section>
      <section class="records queue" aria-labelledby="queue-title">
        <div class="records-heading queue-heading"><h2 id="queue-title" tabindex="-1" ${text('queue')}></h2><div id="queue-controls" class="queue-controls"><button id="start-queue" class="primary" ${text('start')}></button><button id="pause-queue" class="secondary" ${text('pause')}></button></div><p id="queue-pause-status" class="notice" role="status" tabindex="-1" hidden></p></div>
        <div id="job-progress" class="job-progress" hidden><div class="records-heading"><strong id="progress-phase" role="status"></strong><button id="cancel-job" class="secondary" ${text('cancel')}></button></div><progress id="progress-bar" max="100"></progress><p id="progress-detail"></p><p id="progress-speed"></p><p id="progress-current" class="field-help"></p></div><div id="batch-result" class="batch-result" hidden><p id="batch-summary" role="status"></p><button id="retry-batch" type="button" class="secondary" ${text('retryBatch')}></button></div><p id="queue-summary" role="status"></p><ul id="task-list"></ul><details id="history-panel" class="history-panel"><summary id="history-summary"></summary><div class="records-heading"><p class="field-help" ${text('queueHelp')}></p><button id="clear-history" type="button" class="text-button" ${text('clearHistory')}></button></div><ul id="history-list"></ul></details>
      </section>
      <section class="records recovery"><div class="records-heading"><h2 ${text('jobs')}></h2><button id="refresh-jobs" class="text-button" ${text('refreshJobs')}></button></div><p class="field-help" ${text('jobsHelp')}></p><ul id="job-list"></ul></section>
    </section>
    <section id="attributes-panel" hidden>
      <div class="protection-tabs" role="tablist" data-i18n-aria-label="protectionAreas">
        <button id="protection-local-tab" type="button" role="tab" aria-selected="true" aria-controls="protection-local-panel" ${text('protectionLocal')}></button>
        <button id="protection-service-tab" type="button" role="tab" aria-selected="false" aria-controls="managed-service-panel" tabindex="-1" ${text('protectionService')}></button>
      </div>
      <section id="protection-local-panel" role="tabpanel" aria-labelledby="protection-local-tab">
      <p class="protection-local-help" ${text('protectionLocalHelp')}></p>
      <div class="protection-toolbar"><div class="selection-actions"><button id="pick-attribute-file" type="button" class="primary" ${text('addProtectionFiles')}></button><button id="pick-attribute-folder" type="button" class="secondary" ${text('addProtectionFolders')}></button><button id="open-attribute-form" type="button" class="text-button" aria-expanded="false" aria-controls="attribute-form" ${text('manualProtection')}></button></div></div>
      <form id="attribute-form" class="panel protection-add" hidden><fieldset id="attribute-fields"><legend class="sr-only" ${text('attributes')}></legend><div class="protection-add-heading"><h2 ${text('manageAttributes')}></h2><button id="cancel-attribute-add" type="button" class="text-button" ${text('cancelAdd')}></button></div>
        <div class="protection-add-grid"><div class="protection-add-source"><p class="field-help" ${text('protectionDropHelp')}></p><ul id="protection-draft-list" class="protection-draft-list"></ul><details id="attribute-manual"><summary ${text('manualProtection')}></summary><label for="attribute-path" ${text('attributePath')}></label><input id="attribute-path" spellcheck="false" autocomplete="off"><button id="append-protection-path" type="button" class="text-button" ${text('appendProtectionPath')}></button></details></div>
        <div class="protection-add-policy"><label for="attribute-mode" ${text('policy')}></label><select id="attribute-mode"></select><p class="field-help" ${text('protectionApplyHelp')}></p><div id="protection-add-feedback" class="protection-feedback" role="status" aria-live="polite" hidden></div><ul id="protection-add-results" class="protection-item-results"></ul><div class="attribute-actions"><button id="apply-protection-draft" class="primary" type="submit" ${text('applyAttributes')}></button><button id="retry-protection-add" type="button" class="secondary" hidden ${text('retryFailedProtection')}></button></div></div></div>
      </fieldset></form>
      <section id="protection-records" class="records saved-roots protection-records" aria-labelledby="roots-title">
        <div class="records-heading"><h2 id="roots-title" tabindex="-1" ${text('managedRoots')}></h2><div class="protection-record-tools"><form id="protection-search-form" class="protection-search" role="search"><label for="protection-search" class="sr-only" ${text('searchProtection')}></label><input id="protection-search" type="search" maxlength="160" autocomplete="off"><button id="search-protection" type="submit" class="secondary" ${text('search')}></button></form><button id="refresh-records" class="secondary" type="button" ${text('refresh')}></button></div></div>
        <p id="root-summary" role="status" class="field-help"></p><div id="records-feedback" class="protection-feedback" role="status" aria-live="polite" hidden></div>
        <p id="protection-verification-help" class="field-help" ${text('verificationScope')}></p>
        <div id="protection-bulk-toolbar" class="protection-bulk-toolbar"><label class="checkbox"><input id="select-protection-page" type="checkbox"><span ${text('selectProtectionPage')}></span></label><span id="selected-protection-count" role="status"></span><button id="verify-selected-protection" type="button" class="secondary" ${text('verifySelectedProtection')}></button><button id="restore-selected-protection" type="button" class="secondary" aria-controls="protection-restore-confirm" ${text('restoreSelectedProtection')}></button></div>
        <section id="protection-restore-confirm" class="protection-confirm" hidden aria-labelledby="protection-restore-title"><h3 id="protection-restore-title" ${text('confirmSelectedRestore')}></h3><p id="protection-restore-count"></p><ul id="protection-restore-items"></ul><div class="selection-actions"><button id="confirm-selected-restore" type="button" class="primary" ${text('confirmRestore')}></button><button id="cancel-selected-restore" type="button" class="secondary" ${text('cancel')}></button></div></section>
        <p id="protection-batch-feedback" class="protection-feedback" role="status" hidden></p><ul id="protection-batch-results" class="protection-item-results"></ul>
        <div id="protection-columns" class="protection-columns" aria-hidden="true"><span ${text('rootName')}></span><span ${text('registeredMode')}></span><span ${text('verificationStatus')}></span><span ${text('protectionRowActions')}></span></div><ul id="root-list"></ul>
        <div id="roots-pagination" class="selection-actions"><button id="roots-previous" class="secondary" type="button" ${text('previousPage')}></button><button id="roots-next" class="secondary" type="button" ${text('nextPage')}></button></div>
      </section>
      <div class="protection-description">
        <details class="protection-backup"><summary ${text('protectionBackup')}></summary><p class="field-help" ${text('protectionBackupHelp')}></p><div class="selection-actions"><button id="export-protection-backup" type="button" class="secondary" ${text('exportProtectionBackup')}></button><button id="import-protection-backup" type="button" class="secondary" ${text('importProtectionBackup')}></button></div><p id="protection-backup-feedback" class="protection-feedback" role="status" hidden></p><section id="backup-import-confirm" class="protection-confirm" hidden><p ${text('confirmProtectionImport')}></p><div class="selection-actions"><button id="confirm-protection-import" type="button" class="primary" ${text('confirmImport')}></button><button id="cancel-protection-import" type="button" class="secondary" ${text('cancel')}></button></div></section></details>
        <details><summary ${text('protectionDetails')}></summary><p class="notice attribute-notice" ${text('attributeNotice')}></p><p class="field-help" ${text('managedRootsHelp')}></p><p class="field-help" ${text('attributeHelp')}></p><p class="field-help" ${text('reapplyHelp')}></p></details>
      </div>
      </section>
      <section id="managed-service-panel" role="tabpanel" aria-labelledby="protection-service-tab" hidden></section>
    </section>
    <section id="settings-panel" class="settings-page" hidden aria-labelledby="settings-general-title">
      <section class="settings-group">
        <h2 id="settings-general-title" ${text('settingsGeneral')}></h2>
        <div class="settings-row">
          <div><label for="language" ${text('language')}></label><p class="field-help" ${text('settingsLanguageHelp')}></p></div>
          <select id="language"><option value="zh-CN">简体中文</option><option value="en">English</option></select>
        </div>
        <div class="settings-row">
          <div><label for="theme" ${text('settingsTheme')}></label><p class="field-help" ${text('settingsThemeHelp')}></p></div>
          <select id="theme"><option value="system" ${text('settingsThemeSystem')}></option><option value="light" ${text('settingsThemeLight')}></option><option value="dark" ${text('settingsThemeDark')}></option></select>
        </div>
      </section>
      <div id="settings-features"></div>
      <section class="settings-group">
        <h2 ${text('settingsStartup')}></h2>
        <div class="settings-row settings-row-toggle">
          <div><label for="autostart" ${text('settingsAutostart')}></label><p id="autostart-help" class="field-help" ${text('settingsAutostartHelp')}></p></div>
          <input id="autostart" class="setting-checkbox" type="checkbox" role="switch" aria-describedby="autostart-help settings-feedback">
        </div>
        <p id="settings-feedback" class="settings-feedback" role="status" aria-live="polite"></p>
      </section>
      <section class="settings-group settings-about">
        <h2 ${text('settingsAbout')}></h2>
        <div class="settings-row"><span ${text('settingsVersion')}></span><span id="settings-app-version">v0.9.2</span></div>
        <div id="update-settings"></div>
      </section>
    </section>
    <section id="diagnostic" class="diagnostic" hidden><details><summary ${text('errorDetails')}></summary><p class="field-help" ${text('diagnosticPrivacy')}></p><pre id="diagnostic-text"></pre><div class="selection-actions"><button id="copy-diagnostic" type="button" class="secondary" ${text('copyDetails')}></button><button id="export-diagnostic" type="button" class="secondary" ${text('exportDetails')}></button></div><p id="diagnostic-status" role="status"></p></details></section><footer class="main-footer"><span id="activity"></span><span ${text('keepSource')}></span></footer>
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
let page: 'vault' | 'attributes' | 'settings' = 'vault';
let protectionArea: 'local' | 'service' = 'local';
let autostartEnabled: boolean | null = null;
let autostartBusy = false;
let settingsFeedback: MessageKey | null = null;
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
const taskRowIds = new WeakMap<HTMLElement, string>();
const pendingRootModes = new Map<string, ProtectionMode>();
const rootNoteDrafts = new Map<string, string>();
const rootNoteEditorStates = new Map<string, boolean>();
let addFeedback: ProtectionFeedback | null = null;
let recordNotice: ProtectionFeedback | null = null;
let initialTasks: Task[] = [];
try {
  const raw = localStorage.getItem(HISTORY_KEY);
  initialTasks = preferencesBroken ? loadHistory(raw) : filterHistory(loadHistory(raw), preferences);
  if (raw !== null && !preferencesBroken) localStorage.setItem(HISTORY_KEY, serializeHistory(initialTasks, preferences));
}
catch { historyBroken = true; startupError = 'historyInvalid'; }
let lastFailureId = [...initialTasks].reverse().find(task => task.status === 'failed')?.id;
const queue = new TaskQueue(initialTasks, tasks => {
  if (preferencesBroken) throw new Error('preferences_invalid');
  try { localStorage.setItem(HISTORY_KEY, serializeHistory(tasks)); }
  catch { throw new Error('storage_failed'); }
}, () => { renderTasks(); syncControls(); settingsUi?.refresh(); }, () => preferences);

function showResult(key: MessageKey, values: Record<string, string | number> = {}, error = false): void {
  lastResult = { key, values, error }; renderResult();
  if (error) element('result').focus();
}
function showError(error: unknown): void {
  console.error('FoldSentinel operation failed', error);
  if (error instanceof Error && error.message === 'preferences_invalid') { showResult('settingsPreferencesInvalid', {}, true); return; }
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
  const busy = uiBusy || queue.running;
  if (isTauri() && desktopBusy !== busy) {
    desktopBusy = busy;
    void invoke('set_desktop_busy', { busy }).catch(error => { desktopBusy = null; showError(error); });
  }
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
  element<HTMLButtonElement>('apply-protection-draft').disabled = uiBusy || queue.running || !protectionDraft.length || protectionPreviewBusy || protectionPreviewFailed || protectionDraft.some(path => !validProtectionNote(protectionDraftNotes.get(sourceKey(path)) ?? ''));
  element<HTMLButtonElement>('retry-protection-add').disabled = uiBusy || queue.running || protectionPreviewBusy || protectionPreviewFailed || !protectionAddResults.some(result => !result.succeeded);
  document.querySelectorAll<HTMLButtonElement>('#job-list button').forEach(button => { button.disabled = uiBusy || queue.running; });
  element<HTMLButtonElement>('start-queue').disabled = preferencesBroken || uiBusy || queue.running || !queue.tasks.some(t => t.status === 'waiting');
  element<HTMLButtonElement>('pause-queue').disabled = !queue.running || queue.pauseRequested;
  element<HTMLButtonElement>('start-queue').hidden = queue.running;
  element<HTMLButtonElement>('pause-queue').hidden = !queue.running;
  element('queue-controls').hidden = !queue.running && !queue.tasks.some(task => task.status === 'waiting');
  element<HTMLButtonElement>('retry-batch').disabled = uiBusy || !queue.tasks.some(task => task.batchId === [...queue.tasks].reverse().find(item => item.batchId)?.batchId && task.status === 'failed' && canRetryTask(task));
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
  element('selection-title').textContent = t(operation === 'encrypt' ? 'selection' : 'packageSelection');
  element('source-help').textContent = t(operation === 'encrypt' ? 'sourceHelp' : 'packageHelp');
  element('drop-help').textContent = t(operation === 'encrypt' ? 'dropHelp' : 'packageDropHelp');
  element('pick-file').textContent = t(operation === 'encrypt' ? 'pickFiles' : 'pickPackages');
  element<HTMLTextAreaElement>('input-path').placeholder = t(operation === 'encrypt' ? 'sourcePlaceholder' : 'packagePlaceholder');
  input('output-path').placeholder = t(operation === 'decrypt' ? 'restoreOutputPlaceholder' : 'outputPlaceholder');
  element('output-help').textContent = t(operation === 'decrypt' ? 'restoreOutputHelp' : 'outputHelp');
  element('password-help').textContent = t(operation === 'encrypt' ? 'passwordHelp' : 'packagePasswordHelp');
  element('review-help').textContent = t(operation === 'verify' ? 'verifyReviewHelp' : 'reviewHelp');
  element('pick-folder').hidden = operation !== 'encrypt'; element('output-fields').hidden = operation === 'verify';
  element('confirm-field').hidden = operation !== 'encrypt'; input('confirm-password').required = operation === 'encrypt';
  input('password').autocomplete = operation === 'encrypt' ? 'new-password' : 'current-password';
  element('vault-notice').textContent = t(operation === 'verify' ? 'verifyNotice' : operation === 'decrypt' ? 'restoreNotice' : 'notice');
}
function setMode(mode: Operation): void {
  if (operation !== mode) {
    outputDrafts[operation] = input('output-path').value;
    input('output-path').value = outputDrafts[mode] ?? (mode === 'encrypt' ? preferences.defaultOutputDirectory : '');
    operation = mode; clearPasswords(); clearPreview();
  }
  modeLabels();
}
function translate(): void {
  managedUi?.translate();
  updatesUi?.translate();
  settingsUi?.translate();
  const policy = element<HTMLSelectElement>('attribute-mode');
  fillProtectionModes(policy, (policy.value || preferences.defaultProtectionMode) as ProtectionMode);
  document.querySelectorAll<HTMLElement>('[data-i18n]').forEach(el => { el.textContent = t(el.dataset.i18n as MessageKey); });
  document.querySelectorAll<HTMLElement>('[data-i18n-aria-label]').forEach(el => { el.setAttribute('aria-label', t(el.dataset.i18nAriaLabel as MessageKey)); });
  renderSettings();
  input('protection-search').placeholder = t('searchProtection');
  element('page-title').textContent = t(page); element('page-description').textContent = t(pageDescription());
  modeLabels(); renderSources(); renderTasks(); renderJobs(); renderRecords(); renderProtectionDraft(); renderProtectionBatchResults(); renderRestoreSelection(); renderProtectionFeedback('protection-backup-feedback', backupFeedback); renderResult(); renderDiagnostic(); renderPreview(); syncControls(); if (progress) updateProgress(progress);
}
function pageDescription(): MessageKey {
  return page === 'vault' ? 'vaultDescription' : page === 'attributes' ? 'attributesDescription' : 'settingsDescription';
}
function renderSettings(): void {
  element<HTMLSelectElement>('language').value = language;
  element<HTMLSelectElement>('theme').value = theme;
  const autostart = input('autostart');
  autostart.checked = autostartEnabled === true;
  autostart.disabled = !isTauri() || autostartBusy || autostartEnabled === null;
  const status = element('settings-feedback');
  const key = settingsFeedback ?? (!isTauri() ? 'settingsAutostartDesktopOnly' : autostartBusy ? 'settingsSaving' : autostartEnabled === null ? 'settingsAutostartLoading' : autostartEnabled ? 'settingsAutostartOn' : 'settingsAutostartOff');
  status.textContent = t(key);
}
async function loadAutostart(): Promise<void> {
  if (!isTauri() || autostartBusy || autostartEnabled !== null) return;
  autostartBusy = true;
  settingsFeedback = null;
  renderSettings();
  try { autostartEnabled = await invoke<boolean>('autostart_enabled'); }
  catch (error) { console.error(error); settingsFeedback = 'settingsAutostartUnavailable'; }
  finally { autostartBusy = false; renderSettings(); }
}
element<HTMLSelectElement>('language').onchange = () => {
  const next = element<HTMLSelectElement>('language').value as Language;
  try {
    localStorage.setItem(LANGUAGE_KEY, next); setLanguage(next); translate();
    if (isTauri()) void syncDesktopLanguage().catch(showError);
  }
  catch { element<HTMLSelectElement>('language').value = language; showResult('storageFailed', {}, true); }
};
async function syncDesktopLanguage(): Promise<void> {
  const current = await invoke<WindowPreferences>('get_window_preferences');
  windowPreferences = current.language === language ? current : await invoke<WindowPreferences>('set_window_preferences', { preferences: { ...current, language } });
}
element<HTMLSelectElement>('theme').onchange = () => {
  const next = element<HTMLSelectElement>('theme').value as Theme;
  try {
    localStorage.setItem(THEME_KEY, next);
    theme = next;
    document.documentElement.dataset.theme = next;
    document.documentElement.style.colorScheme = next === 'system' ? 'light dark' : next;
    settingsFeedback = null;
    renderSettings();
  } catch { element<HTMLSelectElement>('theme').value = theme; settingsFeedback = 'storageFailed'; renderSettings(); }
};
input('autostart').onchange = () => {
  if (autostartEnabled === null || autostartBusy || !isTauri()) return;
  const previous = autostartEnabled;
  const next = input('autostart').checked;
  autostartEnabled = next;
  autostartBusy = true;
  settingsFeedback = null;
  renderSettings();
  void invoke<boolean>('set_autostart_enabled', { enabled: next }).then(enabled => {
    autostartEnabled = enabled;
    settingsFeedback = null;
  }).catch(error => {
    console.error(error);
    autostartEnabled = previous;
    settingsFeedback = 'settingsAutostartError';
  }).finally(() => {
    autostartBusy = false;
    renderSettings();
  });
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
    try { const paths = await open({ directory: folder, multiple: true, title: t(folder ? 'pickFolders' : operation === 'encrypt' ? 'pickFiles' : 'pickPackages') }); if (paths) appendSources(typeof paths === 'string' ? [paths] : paths); }
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
  element('preview-summary').textContent = t('previewSummary', { files: number(value.file_count), folders: number(value.directory_count), size: bytes(value.content_bytes) });
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
    void notifyTask(false);
    return report;
  }
  catch (error) { captureDiagnostic(error, task.operation); if (failureCode(error) !== 'cancelled') void notifyTask(true); throw error; }
  finally { activeTask = null; progress = null; element('job-progress').hidden = true; }
}
async function notifyTask(failed: boolean): Promise<void> {
  if (!isTauri() || preferences.notification === 'off' || (!failed && preferences.notification !== 'all')) return;
  try { await invoke('notify_task', { failed, language }); }
  catch (error) { console.error('Task notification failed', error); showResult('settingsNotificationFailed', {}, true); }
}
element('start-queue').onclick = () => {
  if (!isTauri()) { showResult('desktopRequired', {}, true); return; }
  void queue.start(perform, failureCode).catch(showError).finally(() => { void refreshJobs().catch(showError); });
};
element('pause-queue').onclick = () => { queue.pause(); element('queue-pause-status').focus(); };
element('cancel-job').onclick = () => {
  element<HTMLButtonElement>('cancel-job').disabled = true;
  void invoke<boolean>('cancel_vault').then(accepted => { cancelling = accepted; if (progress) updateProgress(progress); }).catch(showError);
};
element('clear-history').onclick = () => {
  page = 'settings'; clearPasswords(); lastResult = null; renderResult(); showPage();
  settingsUi?.requestClearHistory();
};

function button(key: MessageKey, action: () => void): HTMLButtonElement {
  const button = document.createElement('button'); button.type = 'button'; button.className = 'secondary'; button.textContent = t(key); button.onclick = action; return button;
}
function refill(plan: Plan): void {
  page = 'vault'; showPage(); setMode(plan.operation); setSources([plan.input]);
  input('output-path').value = outputParent(plan.output);
  clearPasswords(); showResult('retryHelp'); input('password').focus();
}
function renderTasks(): void {
  const list = element('task-list'); const history = element('history-list');
  const active = document.activeElement;
  const activeRow = active instanceof HTMLElement ? active.closest<HTMLElement>('.task-item') : null;
  const focus = active instanceof HTMLElement && (list.contains(active) || history.contains(active))
    ? { id: activeRow ? taskRowIds.get(activeRow) : undefined, action: active.dataset.taskAction } : null;
  list.replaceChildren(); history.replaceChildren();
  const count = (status: string) => queue.tasks.filter(t => t.status === status).length;
  const queueText = t('queueSummary', { waiting: count('waiting'), running: count('running'), done: count('succeeded'), failed: count('failed'), stopped: count('cancelled') + count('interrupted') });
  element('queue-summary').textContent = queueText;
  element('queue-pause-status').hidden = !queue.pauseRequested;
  element('queue-pause-status').textContent = queue.pauseRequested ? t('paused') : '';
  element('queue-overview-text').textContent = count('waiting') && !queue.running ? t('queueNotStarted', { n: count('waiting') }) : t('queueRunningOverview');
  element('queue-overview').hidden = !queue.running && count('waiting') === 0;
  const latestBatch = [...queue.tasks].reverse().find(task => task.batchId)?.batchId;
  const batch = queue.tasks.filter(task => task.batchId === latestBatch && latestBatch !== undefined);
  element('batch-result').hidden = batch.length === 0;
  if (batch.length) element('batch-summary').textContent = t('batchSummary', batchSummary(batch));
  element<HTMLButtonElement>('retry-batch').disabled = uiBusy || !batch.some(task => task.status === 'failed' && canRetryTask(task));
  element('retry-batch').onclick = () => {
    const failed = batch.filter(task => task.status === 'failed' && canRetryTask(task));
    if (!failed.length) return;
    page = 'vault'; showPage(); setMode(failed[0].operation); setSources(mergeSourcePaths([], failed.map(task => task.input)));
    const parents = new Set(failed.map(task => outputParent(task.output)));
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
    const item = document.createElement('li'); item.className = 'task-item'; taskRowIds.set(item, task.id);
    const action = (key: MessageKey, callback: () => void) => {
      const control = button(key, callback); control.dataset.taskAction = key; item.append(control);
    };
    const detail = document.createElement('div'); const title = document.createElement('strong'); title.textContent = `${t(task.operation)} · ${task.pathsHidden ? t('historyPathHidden') : task.input.replace(/^\\\\\?\\/, '')}`;
    const output = document.createElement('p'); output.textContent = task.output ? t('output', { path: task.output.replace(/^\\\\\?\\/, '') }) : t('noOutput');
    const status = document.createElement('p'); status.className = `task-status ${task.status}`; status.textContent = `${t(task.status)} · ${new Date(task.endedAt ?? task.createdAt).toLocaleString(language)}`;
    detail.append(title, output, status);
    if (task.code) { const error = document.createElement('p'); error.className = 'task-error'; error.textContent = errorText(task.code); detail.append(error); }
    item.append(detail);
    if (task.status === 'waiting') action('cancel', () => { try { queue.cancelWaiting(task.id); } catch (error) { showError(error); } });
    else if (['failed', 'cancelled', 'interrupted'].includes(task.status) && canRetryTask(task)) action('retry', () => refill(task));
    if (task.status === 'succeeded' && task.output) action('openOutput', () => { void runUi(async () => { await invoke('open_output_folder', { path: task.output }); }); });
    if (task.status === 'succeeded' && task.operation === 'verify' && canRetryTask(task)) action('inspectAgain', () => refill(task));
    (['waiting', 'running'].includes(task.status) ? list : history).append(item);
  }
  if (focus) {
    const row = Array.from(document.querySelectorAll<HTMLElement>('#task-list .task-item, #history-list .task-item')).find(item => taskRowIds.get(item) === focus.id);
    const target = row && Array.from(row.querySelectorAll<HTMLButtonElement>('[data-task-action]')).find(control => control.dataset.taskAction === focus.action);
    (target && !target.disabled ? target : element('queue-title')).focus({ preventScroll: true });
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
    : recordState.stale && recordNotice?.key !== 'protectionSucceededRefreshFailed' && recordNotice?.key !== 'protectionBatchSummaryRefreshFailed' ? { key: recordState.snapshot ? 'recordsRefreshFailed' : 'recordsLoadFailed', error: true } : recordNotice;
  renderProtectionFeedback('records-feedback', feedback);
  element('root-list').setAttribute('aria-busy', String(recordState.loading));
  syncRecordsPresentation();
  syncControls();
}
function syncRecordsPresentation(): void {
  const snapshot = recordState.snapshot;
  const hasRows = !!snapshot?.roots.length;
  const empty = !!snapshot && !recordState.loading && !recordState.stale && snapshot.overall_total === 0 && !recordsQuery;
  // 保留未提交的搜索输入；刷新失败或尚未读取时，不能把未知状态说成“没有项目”。
  element('protection-search-form').hidden = empty && !input('protection-search').value.trim();
  element('root-summary').hidden = !snapshot?.total;
  for (const id of ['protection-bulk-toolbar', 'protection-columns', 'protection-verification-help']) element(id).hidden = !hasRows;
  if (hasRows) return;
  const roots = element('root-list'); roots.replaceChildren();
  if (recordState.loading || recordState.stale) return;
  if (!snapshot && isTauri()) return;
  const item = document.createElement('li'); item.className = 'empty-records';
  if (empty) {
    item.classList.add('protection-empty');
    const title = document.createElement('strong'); title.textContent = t('emptyRoots');
    const help = document.createElement('p'); help.textContent = t('protectionEmptyHelp'); item.append(title, help);
  } else item.textContent = t(!snapshot ? 'protectionPreview' : recordsQuery ? 'noProtectionMatches' : 'emptyRoots');
  roots.append(item);
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
  if (!snapshot) {
    element('roots-pagination').hidden = true;
    renderRecordsFeedback(); return;
  }
  const visibleIds = new Set(snapshot.roots.map(root => root.id));
  for (const id of selectedProtection) if (!visibleIds.has(id)) selectedProtection.delete(id);
  element('attribute-alert-text').textContent = t('managedRootAttention', { n: snapshot.overall_total });
  element('root-summary').textContent = t('rootSummary', { total: snapshot.total, from: snapshot.total ? snapshot.offset + 1 : 0, to: snapshot.offset + snapshot.roots.length });
  for (const root of snapshot.roots) {
    const row = createRootRecord(root,
    () => { void changeProtection(root.id); }, mode => { void changeProtection(root.id, mode); }, {
      verify: () => { void verifyRoot(root.id); },
      selected: selectedProtection.has(root.id),
      select: selected => { if (selected) selectedProtection.add(root.id); else selectedProtection.delete(root.id); closeRestoreSelection(); syncProtectionSelection(); },
      saveNote: note => { void saveRootNote(root.id, note); },
      noteDraft: rootNoteDrafts.get(root.id),
      onNoteDraft: note => { if (note === undefined) rootNoteDrafts.delete(root.id); else rootNoteDrafts.set(root.id, note); },
      pendingMode: pendingRootModes.get(root.id),
      onPendingMode: mode => { pendingRootModes.set(root.id, mode); },
      noteEditorOpen: rootNoteEditorStates.get(root.id),
      onNoteEditorOpen: open => { rootNoteEditorStates.set(root.id, open); },
      verification: rootVerification.get(root.id),
      aclRestoreConfirmation: aclRestoreReviews.has(root.id) ? {
        confirm: () => { void confirmAclRestore(root.id); },
        cancel: () => { aclRestoreReviews.delete(root.id); renderRecords(); restoreRecordFocus({ id: root.id, action: 'restore', index: 0 }, false); },
      } : undefined,
      feedback: rootFeedback.has(root.id) ? { text: protectionText(rootFeedback.get(root.id)!), error: rootFeedback.get(root.id)!.error } : undefined,
      busy: uiBusy || queue.running, stale: recordState.stale,
    });
    rootRowIds.set(row, root.id);
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
function clearProtectionOperationFeedback(): void {
  recordNotice = null; rootFeedback.clear(); protectionBatchFeedback = null; protectionBatchResults = [];
  renderProtectionBatchResults(); renderRecords();
}
function forgetRestoredRoot(id: string): void {
  rootFeedback.delete(id); rootVerification.delete(id); pendingRootModes.delete(id);
  rootNoteDrafts.delete(id); rootNoteEditorStates.delete(id); selectedProtection.delete(id); aclRestoreReviews.delete(id);
}
async function runSelectedProtection(kind: 'verify' | 'restore', selected = recordState.snapshot?.roots.filter(root => selectedProtection.has(root.id)) ?? []): Promise<void> {
  if (uiBusy || queue.running || recordState.stale || !selected.length) return;
  const origin = document.activeElement;
  clearProtectionOperationFeedback();
  protectionBatchKind = kind; closeRestoreSelection();
  protectionBusyKey = kind === 'verify' ? 'checkingProtection' : 'restoringProtection';
  protectionBatchFeedback = { key: protectionBusyKey }; protectionBatchResults = []; renderProtectionBatchResults();
  await runUi(async () => {
    protectionBatchResults = await runProtectionBatch(selected, async root => {
      if (kind === 'verify') rootVerification.set(root.id, await invoke<RootVerification>('verify_protection', { id: root.id }));
      else { await invoke('restore_attributes', { id: root.id }); forgetRestoredRoot(root.id); }
    }, result => {
      if (!(result.succeeded && kind === 'restore')) rootFeedback.set(result.item.id, result.succeeded ? { key: 'verificationDone' } : { code: failureCode(result.error), error: true });
    });
    const failed = protectionBatchResults.filter(result => !result.succeeded); let refreshed = true;
    try { await refreshRecords(); } catch (error) { refreshed = false; captureDiagnostic(error); }
    protectionBatchFeedback = { key: refreshed ? 'protectionBatchSummary' : 'protectionBatchSummaryRefreshFailed', values: { success: selected.length - failed.length, failed: failed.length }, error: !!failed.length || !refreshed };
    if (failed.length) captureDiagnostic(failed[0].error);
    renderProtectionBatchResults();
  });
  const active = document.activeElement;
  if (page === 'attributes' && protectionArea === 'local' && (active === origin || active === document.body || active?.closest('[hidden]'))) element('roots-title').focus();
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
  return row && id ? { id, action: active?.dataset.rootAction ?? 'mode', index: Array.from(element('root-list').children).indexOf(row) } : null;
}
function restoreRecordFocus(focus: ReturnType<typeof recordFocus>, preventScroll = true): void {
  if (!focus || page !== 'attributes' || protectionArea !== 'local') return;
  const rows = Array.from(element('root-list').querySelectorAll<HTMLElement>('.root-record'));
  const row = rows.find(row => rootRowIds.get(row) === focus.id) ?? rows[Math.min(focus.index, rows.length - 1)];
  const original = row?.querySelector<HTMLElement>(`[data-root-action="${CSS.escape(focus.action)}"]:not(:disabled)`);
  const target = original?.getClientRects().length ? original : row?.querySelector<HTMLElement>('[data-root-action="mode"]:not(:disabled)') ?? row?.querySelector<HTMLElement>('button:not(:disabled)');
  (target ?? element('roots-title')).focus({ preventScroll });
}
async function runRootAction(id: string, busyKey: MessageKey, action: () => Promise<void>, successKey: MessageKey, invalidate = false, reviewAclOnConflict = false): Promise<void> {
  if (uiBusy || queue.running || recordState.stale) return;
  const focus = recordFocus();
  if (!isTauri()) { rootFeedback.set(id, { key: 'desktopRequired', error: true }); renderRecords(); restoreRecordFocus(focus); return; }
  aclRestoreReviews.delete(id);
  protectionBusyKey = busyKey;
  clearProtectionOperationFeedback();
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
    if (result.operationSucceeded && successKey === 'attributeRestored') forgetRestoredRoot(id);
    else rootFeedback.set(id, feedback);
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
  if (open && element('attribute-form').hidden && !protectionDraft.length) fillProtectionModes(element<HTMLSelectElement>('attribute-mode'), preferences.defaultProtectionMode);
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
    const label = document.createElement('label'); label.textContent = t('protectionDraftNote');
    const note = document.createElement('input'); note.type = 'text'; note.maxLength = 160; note.autocomplete = 'off'; note.value = protectionDraftNotes.get(sourceKey(path)) ?? '';
    note.placeholder = t('notePlaceholder'); note.setAttribute('aria-label', `${t('protectionDraftNote')}: ${names[index]}`); note.setCustomValidity(validProtectionNote(note.value) ? '' : t('notePlaceholder'));
    note.oninput = () => { protectionDraftNotes.set(sourceKey(path), note.value); note.setCustomValidity(validProtectionNote(note.value) ? '' : t('notePlaceholder')); note.setAttribute('aria-invalid', String(!validProtectionNote(note.value))); syncControls(); };
    label.append(note); identity.append(label);
    const remove = button('removeSource', () => {
      protectionDraft = protectionDraft.filter(value => value !== path); protectionDraftNotes.delete(sourceKey(path));
      protectionAddResults = protectionAddResults.filter(result => sourceKey(result.item.path) !== sourceKey(path));
      addFeedback = protectionDraft.length ? { key: 'protectionDraftReady', values: { n: protectionDraft.length } } : null;
      void previewProtectionDraft(); element('pick-attribute-file').focus();
    });
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
  protectionAddResults = []; clearProtectionOperationFeedback();
  addFeedback = { key: 'protectionDraftReady', values: { n: protectionDraft.length } };
  showProtectionAdd(); void previewProtectionDraft();
}
for (const folder of [false, true]) element(folder ? 'pick-attribute-folder' : 'pick-attribute-file').onclick = () => { void runUi(async () => {
  try { const paths = await open({ directory: folder, multiple: true, title: t(folder ? 'pickFolders' : 'pickFiles') }); if (paths) { appendProtectionDraft(typeof paths === 'string' ? [paths] : paths); element('attribute-mode').focus(); } }
  catch (error) { console.error(error); addFeedback = { key: 'dialogFailed', error: true }; showProtectionAdd(); renderProtectionDraft(); }
}); };
element('open-attribute-form').onclick = () => { if (!protectionDraft.length) { protectionAddResults = []; addFeedback = null; clearProtectionOperationFeedback(); renderProtectionDraft(); } showProtectionAdd(); element<HTMLDetailsElement>('attribute-manual').open = true; input('attribute-path').focus(); };
element('cancel-attribute-add').onclick = () => { protectionDraft = []; protectionDraftNotes.clear(); protectionAddResults = []; protectionPreview = []; protectionPreviewGeneration++; protectionPreviewBusy = false; protectionPreviewFailed = false; input('attribute-path').value = ''; element<HTMLDetailsElement>('attribute-manual').open = false; addFeedback = null; showProtectionAdd(false); renderProtectionDraft(); element('pick-attribute-file').focus(); };
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
  const paths = retry ? protectionAddResults.filter(result => !result.succeeded && protectionDraft.some(path => sourceKey(path) === sourceKey(result.item.path))).map(result => result.item.path) : [...protectionDraft];
  const draft = inspectSources(paths.join('\n'));
  if (!paths.length || paths.length > MAX_SOURCES || draft.overlaps.length) {
    addFeedback = { key: !paths.length ? 'sourcesRequired' : paths.length > MAX_SOURCES ? 'queueLimit' : 'protectionOverlap', error: true }; renderProtectionDraft(); return;
  }
  if (!isTauri()) { addFeedback = { key: 'desktopRequired', error: true }; renderProtectionDraft(); return; }
  if (paths.some(path => !validProtectionNote(protectionDraftNotes.get(sourceKey(path)) ?? ''))) { addFeedback = { key: 'notePlaceholder', error: true }; renderProtectionDraft(); return; }
  const mode = retry ? protectionRetryMode : element<HTMLSelectElement>('attribute-mode').value as ProtectionMode;
  protectionRetryMode = mode;
  const items = paths.map(path => ({ path, name: protectionOuterName(path), note: (protectionDraftNotes.get(sourceKey(path)) ?? '').trim() }));
  clearProtectionOperationFeedback();
  protectionBusyKey = 'applyingProtection'; addFeedback = { key: 'applyingProtection' }; renderProtectionDraft();
  const origin = document.activeElement;
  let completed = false;
  await runUi(async () => {
    const previous = retry ? protectionAddResults.filter(result => result.succeeded) : [];
    const results = await runProtectionBatch(items, item => invoke('attribute_operation', { path: item.path, mode, restore: false, note: mode === 'none' || !item.note ? undefined : item.note }));
    protectionAddResults = [...previous, ...results];
    const succeeded = results.filter(result => result.succeeded); const failed = results.filter(result => !result.succeeded);
    protectionDraft = protectionDraft.filter(path => !succeeded.some(result => sourceKey(result.item.path) === sourceKey(path)));
    succeeded.forEach(result => protectionDraftNotes.delete(sourceKey(result.item.path)));
    // 添加接口不返回记录 ID，失败也可能已有部分变更；本批执行后须重新核验。
    rootVerification.clear();
    let refreshed = true;
    try { await refreshRecords(0); } catch (error) { refreshed = false; captureDiagnostic(error); }
    addFeedback = { key: refreshed ? 'protectionBatchSummary' : 'protectionBatchSummaryRefreshFailed', values: { success: succeeded.length, failed: failed.length }, error: !!failed.length || !refreshed };
    if (failed.length) captureDiagnostic(failed[0].error);
    if (!failed.length && !protectionDraft.length) {
      recordNotice = { ...addFeedback, values: { success: protectionAddResults.filter(result => result.succeeded).length, failed: 0 } };
      protectionAddResults = []; addFeedback = null; input('attribute-path').value = '';
      element<HTMLDetailsElement>('attribute-manual').open = false; showProtectionAdd(false); completed = true;
      renderRecordsFeedback();
    }
    await previewProtectionDraft();
  });
  const active = document.activeElement;
  if (completed && page === 'attributes' && protectionArea === 'local' && (active === origin || active === document.body || active?.closest('[hidden]'))) element('roots-title').focus();
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
    rootVerification.clear(); rootFeedback.clear(); pendingRootModes.clear(); rootNoteDrafts.clear(); rootNoteEditorStates.clear(); selectedProtection.clear();
    backupFeedback = { key: 'protectionBackupImported', values: { roots: report.roots, entries: report.entries } };
    try { await refreshRecords(0); } catch (error) { backupFeedback = { key: 'protectionBackupImportedRefreshFailed', error: true }; captureDiagnostic(error); }
  } catch (error) { backupFeedback = { code: failureCode(error), error: true }; captureDiagnostic(error); }
  renderProtectionFeedback('protection-backup-feedback', backupFeedback);
}); };
element('refresh-records').onclick = () => { void refreshProtectionPage(); };
element('roots-previous').onclick = () => { void refreshProtectionPage(Math.max(0, (recordState.snapshot?.offset ?? 0) - ATTRIBUTE_PAGE_SIZE)); };
element('roots-next').onclick = () => { void refreshProtectionPage((recordState.snapshot?.offset ?? 0) + ATTRIBUTE_PAGE_SIZE); };
element<HTMLFormElement>('protection-search-form').onsubmit = event => { event.preventDefault(); if (uiBusy || queue.running) return; recordsQuery = input('protection-search').value.trim(); void refreshProtectionPage(0); };
element('review-queue').onclick = () => { element('queue-title').scrollIntoView({ block: 'start' }); element('queue-title').focus({ preventScroll: true }); };
function showProtectionArea(): void {
  for (const area of ['local', 'service'] as const) {
    const selected = area === protectionArea;
    const tab = element(`protection-${area}-tab`);
    tab.setAttribute('aria-selected', String(selected)); tab.tabIndex = selected ? 0 : -1;
    element(area === 'local' ? 'protection-local-panel' : 'managed-service-panel').hidden = !selected;
  }
  managedUi?.setVisible(page === 'attributes' && protectionArea === 'service');
}
function selectProtectionArea(area: 'local' | 'service'): void {
  if (protectionArea === area) return;
  protectionArea = area; clearPasswords(); showProtectionArea();
}
const protectionAreas = ['local', 'service'] as const;
for (const [index, area] of protectionAreas.entries()) {
  const tab = element(`protection-${area}-tab`);
  tab.onclick = () => selectProtectionArea(area);
  tab.onkeydown = event => {
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? protectionAreas.length - 1
      : event.key === 'ArrowRight' ? (index + 1) % protectionAreas.length
      : event.key === 'ArrowLeft' ? (index + protectionAreas.length - 1) % protectionAreas.length : null;
    if (next === null) return;
    event.preventDefault(); selectProtectionArea(protectionAreas[next]); element(`protection-${protectionAreas[next]}-tab`).focus();
  };
}
function showPage(): void {
  showProtectionArea();
  for (const value of ['vault', 'attributes', 'settings'] as const) {
    element(`${value}-panel`).hidden = value !== page; element(`nav-${value}`).classList.toggle('selected', value === page);
    if (value === page) element(`nav-${value}`).setAttribute('aria-current', 'page'); else element(`nav-${value}`).removeAttribute('aria-current');
  }
  element('page-title').textContent = t(page); element('page-description').textContent = t(pageDescription());
}
for (const value of ['vault', 'attributes', 'settings'] as const) element(`nav-${value}`).onclick = () => {
  page = value; clearPasswords(); lastResult = null; renderResult(); showPage(); element('page-title').focus();
  if (value === 'attributes' && protectionArea === 'local' && isTauri() && !uiBusy && !queue.running) void refreshProtectionPage();
  if (value === 'settings') { void loadAutostart(); settingsUi?.refresh(); void settingsUi?.loadDesktop(); }
};
element('review-attributes').onclick = () => { page = 'attributes'; protectionArea = 'local'; clearPasswords(); lastResult = null; renderResult(); showPage(); element('roots-title').focus(); if (isTauri()) void refreshProtectionPage(); };

function changePreferences(patch: Partial<Preferences>): void {
  const previous = preferences;
  const wasBroken = preferencesBroken;
  const next = loadPreferences(serializePreferences({ ...preferences, ...patch }));
  const saved = localStorage.getItem(PREFERENCES_KEY);
  localStorage.setItem(PREFERENCES_KEY, serializePreferences(next));
  preferencesBroken = false;
  try {
    if (next.historyPaths !== previous.historyPaths || next.historyRetentionDays !== previous.historyRetentionDays) queue.applyHistoryPreferences(next);
  } catch (error) {
    preferencesBroken = wasBroken;
    try { if (saved === null) localStorage.removeItem(PREFERENCES_KEY); else localStorage.setItem(PREFERENCES_KEY, saved); }
    catch (rollbackError) { console.error('Could not restore preferences after history storage failure', rollbackError); showResult('storageFailed', {}, true); }
    throw error;
  }
  preferences = next;
  document.documentElement.dataset.density = next.density;
  document.documentElement.dataset.fontSize = next.fontSize;
  if (!inspectSources(element<HTMLTextAreaElement>('input-path').value).paths.length && operation === 'encrypt' && (!input('output-path').value || input('output-path').value === previous.defaultOutputDirectory)) input('output-path').value = next.defaultOutputDirectory;
  if (!protectionDraft.length && element('attribute-form').hidden) fillProtectionModes(element<HTMLSelectElement>('attribute-mode'), next.defaultProtectionMode);
  settingsUi?.refresh();
  syncControls();
}
settingsUi = new SettingsUi(element('settings-features'), {
  read: () => preferences, change: changePreferences, isDesktop: isTauri,
  historyCount: () => queue.tasks.filter(task => !['waiting', 'running'].includes(task.status)).length,
  clearHistory: () => { queue.clearHistory(); historyBroken = false; },
  exportDiagnostic: async () => {
    const path = await save({ title: t('exportDetails'), defaultPath: 'FoldSentinel-diagnostic.json', filters: [{ name: 'JSON', extensions: ['json'] }] });
    if (!path) return false;
    if (diagnostic) await invoke('export_diagnostic', { path, request: diagnostic });
    else await invoke('export_system_diagnostic', { path });
    return true;
  },
  dataLocations: () => invoke<DataLocations>('data_locations'),
  openDataLocation: kind => invoke('open_data_location', { kind }),
  chooseOutput: async () => {
    const chosen = await open({ directory: true, multiple: false, title: t('chooseDirectory') });
    return typeof chosen === 'string' ? chosen : null;
  },
  getWindowPreferences: async () => { windowPreferences = await invoke<WindowPreferences>('get_window_preferences'); return windowPreferences; },
  setWindowPreferences: async value => { windowPreferences = await invoke<WindowPreferences>('set_window_preferences', { preferences: value }); },
});
input('output-path').value = preferences.defaultOutputDirectory;
const updatesUi = new UpdatesUi(element('update-settings'), {
  isDesktop: isTauri,
  check: () => invoke<UpdateCheck>('check_for_updates'),
  open: url => invoke<void>('open_release_page', { url }),
});
managedUi = new ManagedProtectionUi(element('managed-service-panel'), {
  request: command => invoke('managed_protection', { command }),
  installationStatus: () => invoke('service_installation_status'),
  installService: () => invoke('service_install'),
  run: action => { protectionBusyKey = 'workingServiceProtection'; return runUi(action); }, isDesktop: isTauri, isBlocked: () => uiBusy || queue.running,
  defaultUnlockMinutes: () => preferences.defaultUnlockMinutes,
  choose: async directory => {
    const path = await open({ directory, multiple: false, title: t(directory ? 'addProtectionFolders' : 'addProtectionFiles') });
    return typeof path === 'string' ? path : null;
  },
});
translate();
if (startupError) showResult(startupError, {}, true);
void loadAutostart();
if (isTauri()) {
  void syncDesktopLanguage().catch(showError);
  void listen('window-hidden', clearPasswords).catch(showError);
  void listen('exit-blocked', () => { clearPasswords(); showResult('settingsExitBlocked', {}, true); }).catch(showError);
  void listen('desktop-action-error', () => { showResult('settingsWindowActionFailed', {}, true); }).catch(showError);
  void invoke<string | null>('take_desktop_action_error').then(error => { if (error) showResult('settingsWindowActionFailed', {}, true); }).catch(showError);
  void getCurrentWindow().onCloseRequested(event => {
    clearPasswords();
    // 阻止 Tauri JS 监听器默认 destroy，交由原生 CloseRequested 隐藏到托盘。
    if (windowPreferences.close_to_tray) { event.preventDefault(); return; }
    queue.pause();
    if (queue.running) {
      event.preventDefault();
      void invoke<boolean>('cancel_vault').then(accepted => { cancelling = accepted; if (progress) updateProgress(progress); }).catch(showError);
    } else if (uiBusy) {
      event.preventDefault();
      showResult('settingsExitBlocked', {}, true);
    }
  }).catch(showError);
  void refreshJobs().catch(showError);
  void refreshProtectionPage();
  void getCurrentWebview().onDragDropEvent(event => {
    const payload = event.payload;
    for (const id of ['attributes-panel', 'vault-form']) element(id).classList.remove('drag-over');
    if (page === 'settings' || (page === 'attributes' && protectionArea === 'service')) return;
    const target = element(page === 'attributes' ? 'attributes-panel' : 'vault-form');
    target.classList.toggle('drag-over', payload.type === 'enter' || payload.type === 'over');
    if (payload.type === 'drop' && !uiBusy && !(page === 'attributes' && queue.running)) {
      if (page === 'attributes') { appendProtectionDraft(payload.paths); element('attribute-mode').focus(); }
      else { appendSources(payload.paths); element('pick-file').focus(); }
    }
  }).catch(error => { console.error(error); showResult('dialogFailed', {}, true); });
}
