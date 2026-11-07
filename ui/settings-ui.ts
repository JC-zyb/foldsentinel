import { PROTECTION_MODES } from './attributes';
import { language, t, type MessageKey } from './i18n';
import type { Preferences } from './preferences';

export interface DataLocations { app_data: string; recovery: string; protection: string }
export type DataLocationKind = keyof DataLocations;
export interface WindowPreferences { close_to_tray: boolean; start_minimized: boolean; language: 'zh-CN' | 'en' }
export interface SettingsDependencies {
  read: () => Preferences;
  change: (patch: Partial<Preferences>) => Promise<void> | void;
  isDesktop: () => boolean;
  clearHistory: () => Promise<void> | void;
  historyCount: () => number;
  exportDiagnostic: () => Promise<boolean>;
  dataLocations: () => Promise<DataLocations>;
  openDataLocation: (kind: DataLocationKind) => Promise<void>;
  chooseOutput: () => Promise<string | null>;
  getWindowPreferences: () => Promise<WindowPreferences>;
  setWindowPreferences: (preferences: WindowPreferences) => Promise<void>;
}

type Group = 'notifications' | 'history' | 'diagnostics' | 'defaults' | 'window' | 'display';
type Confirmation = { kind: 'clear' } | { kind: 'paths' } | { kind: 'retention'; days: Preferences['historyRetentionDays'] };
const modeLabels = { none: 'noProtection', hidden: 'hidden', locked: 'locked', readonly: 'readonly', 'readonly-acl': 'readonlyAcl', 'hidden-locked': 'hiddenLocked' } as const;

export class SettingsUi {
  private readonly translations: (() => void)[] = [];
  private readonly groups = new Map<Group, HTMLElement>();
  private readonly statuses = new Map<Group, HTMLParagraphElement>();
  private readonly feedback = new Map<Group, { key: MessageKey; error: boolean }>();
  private readonly busy = new Set<Group>();
  private readonly paths = new Map<DataLocationKind, HTMLElement>();
  private readonly confirmPanel = document.createElement('div');
  private readonly confirmText = document.createElement('p');
  private readonly confirmButton = document.createElement('button');
  private confirmation: Confirmation | null = null;
  private locations: DataLocations | null = null;
  private windowPreferences: WindowPreferences | null = null;
  private loading = false;

  constructor(private readonly container: HTMLElement, private readonly dependencies: SettingsDependencies) {
    container.classList.add('settings-extensions');
    const notifications = this.group('notifications', 'settingsNotifications');
    const notification = this.select(notifications, 'settings-notification', 'settingsNotifyWhen', 'settingsNotifyHelp', [
      ['off', 'settingsNotifyOff'], ['failures', 'settingsNotifyFailures'], ['all', 'settingsNotifyAll']
    ]);
    notification.onchange = () => { void this.change('notifications', { notification: notification.value as Preferences['notification'] }); };

    const history = this.group('history', 'settingsHistory');
    const retention = this.select(history, 'settings-history-retention', 'settingsHistoryRetention', 'settingsHistoryRetentionHelp', [
      ['0', 'settingsHistoryForever'], ['7', 'settingsHistory7Days'], ['30', 'settingsHistory30Days'], ['90', 'settingsHistory90Days']
    ]);
    retention.onchange = () => {
      const days = Number(retention.value) as Preferences['historyRetentionDays'];
      if (days && this.dependencies.historyCount()) this.ask({ kind: 'retention', days });
      else void this.change('history', { historyRetentionDays: days });
    };
    const historyPaths = this.toggle(history, 'settings-history-paths', 'settingsHistoryPaths', 'settingsHistoryPathsHelp');
    historyPaths.onchange = () => {
      if (!historyPaths.checked) this.ask({ kind: 'paths' });
      else void this.change('history', { historyPaths: true });
    };
    const clearRow = this.row(history, 'settingsHistoryClear', 'settingsHistoryClearHelp');
    const clear = this.button('clearHistory'); clear.id = 'settings-clear-history';
    clear.onclick = () => this.ask({ kind: 'clear' }); clearRow.append(clear);
    const count = document.createElement('p'); count.className = 'field-help'; count.id = 'settings-history-count'; history.append(count);
    this.confirmPanel.className = 'settings-confirm'; this.confirmPanel.hidden = true;
    this.confirmPanel.setAttribute('role', 'group'); this.confirmPanel.setAttribute('aria-labelledby', 'settings-confirm-text');
    this.confirmText.id = 'settings-confirm-text'; this.confirmPanel.append(this.confirmText);
    const confirmActions = document.createElement('div'); confirmActions.className = 'selection-actions';
    this.confirmButton.type = 'button'; this.confirmButton.className = 'primary'; this.confirmButton.id = 'settings-confirm-history';
    this.confirmButton.onclick = () => { void this.confirm(); };
    const cancel = this.button('cancel'); cancel.id = 'settings-cancel-history'; cancel.onclick = () => this.dismissConfirmation();
    confirmActions.append(this.confirmButton, cancel); this.confirmPanel.append(confirmActions); history.append(this.confirmPanel);
    this.confirmPanel.onkeydown = event => { if (event.key === 'Escape' && !this.busy.has('history')) { event.preventDefault(); this.dismissConfirmation(); } };

    const diagnostics = this.group('diagnostics', 'settingsDiagnostics');
    const exportRow = this.row(diagnostics, 'settingsDiagnosticExport', 'settingsDiagnosticHelp');
    const exportButton = this.button('exportDetails', true); exportButton.id = 'settings-export-diagnostic';
    exportButton.onclick = () => { void this.perform('diagnostics', async () => { if (await this.dependencies.exportDiagnostic()) return 'settingsDiagnosticExported'; }, null); }; exportRow.append(exportButton);
    const locations = document.createElement('details'); locations.className = 'settings-data-locations';
    const locationSummary = document.createElement('summary'); this.text(locationSummary, 'settingsDataLocations'); locations.append(locationSummary);
    const locationHelp = document.createElement('p'); locationHelp.className = 'field-help'; this.text(locationHelp, 'settingsDataHelp'); locations.append(locationHelp);
    const locationKeys = { app_data: 'settingsDataApp', recovery: 'settingsDataRecovery', protection: 'settingsDataProtection' } as const;
    for (const kind of ['app_data', 'recovery', 'protection'] as const) {
      const row = document.createElement('div'); row.className = 'settings-location-row';
      const identity = document.createElement('div'); const label = document.createElement('strong'); this.text(label, locationKeys[kind]);
      const path = document.createElement('p'); path.className = 'field-help settings-location-path'; path.id = `settings-location-${kind}`; this.paths.set(kind, path); identity.append(label, path);
      const open = this.button('settingsOpenFolder', true); open.id = `settings-open-${kind}`; open.setAttribute('aria-describedby', path.id);
      this.translations.push(() => open.setAttribute('aria-label', `${t('settingsOpenFolder')}: ${t(locationKeys[kind])}`));
      open.onclick = () => { void this.perform('diagnostics', () => this.dependencies.openDataLocation(kind), 'settingsFolderOpened'); }; row.append(identity, open); locations.append(row);
    }
    const reload = this.button('settingsReloadDesktop', true); reload.id = 'settings-reload-desktop'; reload.onclick = () => { void this.loadDesktop(); }; locations.append(reload); diagnostics.append(locations);

    const defaults = this.group('defaults', 'settingsDefaults');
    const defaultsHelp = document.createElement('p'); defaultsHelp.className = 'field-help'; this.text(defaultsHelp, 'settingsDefaultsHelp'); defaults.append(defaultsHelp);
    const protection = this.select(defaults, 'settings-default-protection', 'settingsDefaultProtection', 'settingsDefaultProtectionHelp', PROTECTION_MODES.map(mode => [mode, modeLabels[mode]]));
    protection.onchange = () => { void this.change('defaults', { defaultProtectionMode: protection.value as Preferences['defaultProtectionMode'] }); };
    const minutes = this.select(defaults, 'settings-default-unlock', 'settingsDefaultUnlock', 'settingsDefaultUnlockHelp', [1, 5, 15, 30, 60].map(value => [String(value), 'managedMinutes', { n: value }]));
    minutes.onchange = () => { void this.change('defaults', { defaultUnlockMinutes: Number(minutes.value) as Preferences['defaultUnlockMinutes'] }); };
    const outputRow = this.row(defaults, 'settingsDefaultOutput', 'settingsDefaultOutputHelp', 'settings-default-output'); outputRow.classList.add('settings-row-stacked');
    const outputControls = document.createElement('div'); outputControls.className = 'settings-output-controls';
    const output = document.createElement('input'); output.id = 'settings-default-output'; output.type = 'text'; output.readOnly = true; output.setAttribute('aria-describedby', 'settings-default-output-help');
    this.translations.push(() => { output.placeholder = t('settingsOutputSource'); });
    const choose = this.button('settingsChooseFolder', true); choose.id = 'settings-choose-output'; choose.onclick = () => { void this.perform('defaults', async () => { const path = await this.dependencies.chooseOutput(); if (path !== null) await this.dependencies.change({ defaultOutputDirectory: path }); }, null); };
    const reset = this.button('settingsResetOutput'); reset.id = 'settings-reset-output'; reset.onclick = () => { void this.change('defaults', { defaultOutputDirectory: '' }); };
    outputControls.append(output, choose, reset); outputRow.append(outputControls);

    const window = this.group('window', 'settingsWindow');
    const closeToTray = this.toggle(window, 'settings-close-to-tray', 'settingsCloseToTray', 'settingsCloseToTrayHelp', true);
    closeToTray.onchange = () => { void this.changeWindow({ close_to_tray: closeToTray.checked }); };
    const minimized = this.toggle(window, 'settings-start-minimized', 'settingsStartMinimized', 'settingsStartMinimizedHelp', true);
    minimized.onchange = () => { void this.changeWindow({ start_minimized: minimized.checked }); };
    const windowHelp = document.createElement('p'); windowHelp.className = 'field-help'; this.text(windowHelp, 'settingsWindowServiceHelp'); window.append(windowHelp);
    const reloadWindow = this.button('settingsReloadDesktop', true); reloadWindow.id = 'settings-reload-window'; reloadWindow.onclick = () => { void this.loadDesktop(); }; window.append(reloadWindow);

    const display = this.group('display', 'settingsDisplay');
    const density = this.select(display, 'settings-density', 'settingsDensity', 'settingsDensityHelp', [['standard', 'settingsDensityStandard'], ['compact', 'settingsDensityCompact']]);
    density.onchange = () => { void this.change('display', { density: density.value as Preferences['density'] }); };
    const fontSize = this.select(display, 'settings-font-size', 'settingsFontSize', 'settingsFontSizeHelp', [['standard', 'settingsFontStandard'], ['large', 'settingsFontLarge']]);
    fontSize.onchange = () => { void this.change('display', { fontSize: fontSize.value as Preferences['fontSize'] }); };
    for (const [group, section] of this.groups) {
      const status = document.createElement('p'); status.className = 'settings-feedback'; status.id = `settings-${group}-feedback`; status.setAttribute('role', 'status'); status.setAttribute('aria-live', 'polite'); status.setAttribute('aria-atomic', 'true');
      this.statuses.set(group, status); section.append(status);
    }
    this.translate();
  }

  private text(element: HTMLElement, key: MessageKey): void { this.translations.push(() => { element.textContent = t(key); }); }
  private group(id: Group, title: MessageKey): HTMLElement {
    const group = document.createElement('section'); group.className = 'settings-group'; group.id = `settings-${id}-group`;
    const heading = document.createElement('h2'); heading.id = `settings-${id}-title`; this.text(heading, title); group.setAttribute('aria-labelledby', heading.id); group.append(heading);
    this.container.append(group); this.groups.set(id, group); return group;
  }
  private row(group: HTMLElement, title: MessageKey, help: MessageKey, id?: string): HTMLElement {
    const row = document.createElement('div'); row.className = 'settings-row';
    const copy = document.createElement('div'); const label = document.createElement(id ? 'label' : 'strong'); if (label instanceof HTMLLabelElement) label.htmlFor = id!;
    this.text(label, title); const description = document.createElement('p'); description.className = 'field-help'; if (id) description.id = `${id}-help`; this.text(description, help); copy.append(label, description); row.append(copy); group.append(row); return row;
  }
  private select(group: HTMLElement, id: string, title: MessageKey, help: MessageKey, values: [string, MessageKey, Record<string, string | number>?][]): HTMLSelectElement {
    const row = this.row(group, title, help, id); const select = document.createElement('select'); select.id = id; select.setAttribute('aria-describedby', `${id}-help`);
    for (const [value, key, params] of values) { const option = document.createElement('option'); option.value = value; this.translations.push(() => { option.textContent = t(key, params); }); select.append(option); }
    row.append(select); return select;
  }
  private toggle(group: HTMLElement, id: string, title: MessageKey, help: MessageKey, desktop = false): HTMLInputElement {
    const row = this.row(group, title, help, id); row.classList.add('settings-row-toggle');
    const target = document.createElement('label'); target.className = 'settings-switch-target'; target.htmlFor = id;
    const input = document.createElement('input'); input.id = id; input.type = 'checkbox'; input.className = 'setting-checkbox'; input.setAttribute('role', 'switch'); input.setAttribute('aria-describedby', `${id}-help`);
    if (desktop) input.dataset.desktopOnly = ''; target.append(input); row.append(target); return input;
  }
  private button(key: MessageKey, desktop = false): HTMLButtonElement {
    const button = document.createElement('button'); button.type = 'button'; button.className = 'secondary'; this.text(button, key); if (desktop) button.dataset.desktopOnly = ''; return button;
  }
  private control<T extends HTMLInputElement | HTMLSelectElement | HTMLButtonElement = HTMLInputElement>(id: string): T { return this.container.querySelector<T>(`#${id}`)!; }

  translate(): void { for (const translate of this.translations) translate(); this.refresh(); }
  refresh(): void {
    const preferences = this.dependencies.read();
    const bindings = { 'settings-notification': preferences.notification, 'settings-history-retention': String(preferences.historyRetentionDays), 'settings-default-protection': preferences.defaultProtectionMode, 'settings-default-unlock': String(preferences.defaultUnlockMinutes), 'settings-default-output': preferences.defaultOutputDirectory, 'settings-density': preferences.density, 'settings-font-size': preferences.fontSize };
    for (const [id, value] of Object.entries(bindings)) this.control(id).value = value;
    this.control('settings-history-paths').checked = preferences.historyPaths;
    this.control('settings-close-to-tray').checked = this.windowPreferences?.close_to_tray ?? false;
    this.control('settings-start-minimized').checked = this.windowPreferences?.start_minimized ?? false;
    const count = this.container.querySelector('#settings-history-count')!; count.textContent = t('settingsHistoryCount', { n: this.dependencies.historyCount() });
    for (const [group, section] of this.groups) {
      const busy = this.busy.has(group); section.setAttribute('aria-busy', String(busy));
      section.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>('input, select, button').forEach(control => {
        control.disabled = busy || ('desktopOnly' in control.dataset && !this.dependencies.isDesktop());
      });
      const message = this.feedback.get(group); const status = this.statuses.get(group)!;
      status.textContent = message ? t(message.key) : group === 'window' && !this.dependencies.isDesktop() ? t('settingsDesktopOnly') : '';
      status.classList.toggle('settings-error', message?.error ?? false); status.setAttribute('role', message?.error ? 'alert' : 'status');
    }
    if (!this.windowPreferences) { this.control('settings-close-to-tray').disabled = true; this.control('settings-start-minimized').disabled = true; }
    this.control<HTMLButtonElement>('settings-clear-history').disabled ||= this.dependencies.historyCount() === 0;
    this.control<HTMLButtonElement>('settings-reset-output').disabled ||= !preferences.defaultOutputDirectory;
    for (const [kind, element] of this.paths) element.textContent = this.locations?.[kind] ?? t(this.dependencies.isDesktop() ? 'settingsDataNotLoaded' : 'settingsDesktopOnly');
    if (this.confirmation) {
      this.confirmText.textContent = t(this.confirmation.kind === 'clear' ? 'settingsConfirmClearHistory' : this.confirmation.kind === 'paths' ? 'settingsConfirmHistoryPaths' : 'settingsConfirmRetention');
      this.confirmButton.textContent = t(this.confirmation.kind === 'clear' ? 'settingsConfirmClear' : 'settingsConfirmChange');
    }
  }

  private async perform(group: Group, action: () => Promise<void | MessageKey> | void, success: MessageKey | null = 'settingsSaved', pending: MessageKey = 'settingsWorking'): Promise<void> {
    if (this.busy.has(group)) return;
    const section = this.groups.get(group)!;
    const active = document.activeElement;
    const origin = active instanceof HTMLElement && section.contains(active) ? active : null;
    this.busy.add(group); this.feedback.set(group, { key: pending, error: false }); this.refresh();
    try { const result = await action(); if (result ?? success) this.feedback.set(group, { key: (result ?? success)!, error: false }); else this.feedback.delete(group); }
    catch (error) { console.error(`Settings action failed (${group})`, error); this.feedback.set(group, { key: group === 'diagnostics' ? 'settingsDiagnosticFailed' : 'settingsChangeFailed', error: true }); }
    finally {
      this.busy.delete(group); this.refresh();
      if (origin?.isConnected && origin.getClientRects().length && !origin.matches(':disabled') && document.activeElement === document.body) origin.focus({ preventScroll: true });
    }
  }
  private async change(group: Group, patch: Partial<Preferences>): Promise<void> { await this.perform(group, () => this.dependencies.change(patch), 'settingsSaved', 'settingsSaving'); }
  private ask(confirmation: Confirmation): void {
    this.confirmation = confirmation; this.confirmPanel.hidden = false; this.refresh(); this.container.querySelector<HTMLButtonElement>('#settings-cancel-history')!.focus();
  }
  private dismissConfirmation(): void {
    const origin = this.confirmation?.kind === 'clear' ? 'settings-clear-history' : this.confirmation?.kind === 'paths' ? 'settings-history-paths' : 'settings-history-retention';
    this.confirmation = null; this.confirmPanel.hidden = true; this.refresh(); this.control(origin).focus();
  }
  private async confirm(): Promise<void> {
    const confirmation = this.confirmation; if (!confirmation) return;
    await this.perform('history', async () => {
      if (confirmation.kind === 'clear') await this.dependencies.clearHistory();
      else await this.dependencies.change(confirmation.kind === 'paths' ? { historyPaths: false } : { historyRetentionDays: confirmation.days });
      this.confirmation = null; this.confirmPanel.hidden = true;
    });
    if (!this.confirmation && (document.activeElement === document.body || this.confirmPanel.contains(document.activeElement))) {
      const target = this.control(confirmation.kind === 'paths' ? 'settings-history-paths' : 'settings-history-retention');
      target.focus({ preventScroll: true });
    }
  }
  private async changeWindow(patch: Partial<WindowPreferences>): Promise<void> {
    if (!this.windowPreferences || !this.dependencies.isDesktop()) return;
    const next = { ...this.windowPreferences, ...patch, language };
    await this.perform('window', async () => { await this.dependencies.setWindowPreferences(next); this.windowPreferences = next; }, 'settingsSaved', 'settingsSaving');
  }
  requestClearHistory(): void { this.ask({ kind: 'clear' }); }
  async loadDesktop(): Promise<void> {
    if (!this.dependencies.isDesktop() || this.loading) return;
    this.loading = true;
    await Promise.all([
      this.perform('diagnostics', async () => { this.locations = await this.dependencies.dataLocations(); }, null, 'settingsReading'),
      this.perform('window', async () => { this.windowPreferences = await this.dependencies.getWindowPreferences(); }, null, 'settingsReading')
    ]);
    this.loading = false;
  }
}
