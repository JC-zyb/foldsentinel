import { errorText, failureCode, language, t, type MessageKey } from './i18n';
import { validProtectionNote } from './protection-workflow';
import { clearManagedCommand, managedStatus, requestManaged, validManagedPassword, type ManagedCommand, type ManagedMode, type ServiceRoot, type ServiceStatus, type ServiceTransport } from './managed-workflow';

interface ManagedOptions {
  request: (command: ManagedCommand) => Promise<unknown>;
  run: (action: () => Promise<void>) => Promise<void>;
  isDesktop: () => boolean;
  isBlocked: () => boolean;
  choose: (directory: boolean) => Promise<string | null>;
}
type EditorKind = 'enroll' | 'unlock' | 'password' | 'remove' | 'continuous';
interface Feedback { key?: MessageKey; code?: string; values?: Record<string, string | number>; error?: boolean }
const modeKeys = { locked: 'locked', 'hidden-locked': 'hiddenLocked', 'readonly-acl': 'readonlyAcl' } as const;
const nameOnly = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? '';

export class ManagedProtectionUi {
  private snapshot: ServiceStatus | null = null;
  private transport: ServiceTransport | null = null;
  private stale = false;
  private loading = false;
  private available = false;
  private visible = false;
  private editor: { kind: EditorKind; root?: ServiceRoot } | null = null;
  private editorOrigin: { id: string | null; action: string } | null = null;
  private feedback: Feedback | null = null;
  private rowFeedback = new Map<string, Feedback>();
  private rowIds = new WeakMap<HTMLElement, string>();
  private offset = 0;
  private query = '';
  private poll: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly container: HTMLElement, private readonly options: ManagedOptions) { this.render(); }

  private element<T extends HTMLElement = HTMLElement>(id: string): T { return this.container.querySelector(`#${id}`) as T; }
  private button(key: MessageKey, action: () => void, primary = false): HTMLButtonElement {
    const button = document.createElement('button'); button.type = 'button'; button.className = primary ? 'primary' : 'secondary'; button.textContent = t(key); button.onclick = action; return button;
  }
  private paragraph(text: string, className = 'field-help'): HTMLParagraphElement { const p = document.createElement('p'); p.className = className; p.textContent = text; return p; }
  private feedbackText(feedback: Feedback): string { return feedback.code === 'authentication' ? t('managedAuthenticationFailed') : feedback.code ? errorText(feedback.code) : t(feedback.key!, feedback.values); }

  clearPasswords(): void { this.container.querySelectorAll<HTMLInputElement>('input[type=password]').forEach(input => { input.value = ''; }); }
  translate(): void { this.clearPasswords(); this.editor = null; this.editorOrigin = null; this.render(); }
  setVisible(visible: boolean): void {
    this.visible = visible; this.clearPasswords(); this.editor = null; this.editorOrigin = null; this.render();
    if (!visible) { if (this.poll) clearTimeout(this.poll); this.poll = null; }
    else if (this.options.isDesktop()) { void this.refresh(); }
  }
  sync(): void {
    const blocked = this.options.isBlocked() || this.loading;
    if (this.options.isBlocked()) this.clearPasswords();
    const actionsBlocked = blocked || !this.available || this.stale;
    this.container.querySelectorAll<HTMLButtonElement>('button').forEach(button => { button.disabled = blocked; });
    this.container.querySelectorAll<HTMLButtonElement>('[data-managed-action]').forEach(button => { button.disabled = actionsBlocked; });
    const fieldset = this.container.querySelector<HTMLFieldSetElement>('fieldset'); if (fieldset) fieldset.disabled = actionsBlocked;
    const search = this.container.querySelector<HTMLInputElement>('#managed-search'); if (search) search.disabled = blocked;
    const refresh = this.container.querySelector<HTMLButtonElement>('#managed-refresh'); if (refresh) refresh.disabled = blocked || !this.options.isDesktop();
    if (this.loading) this.element('managed-service-status').textContent = t('managedConnecting');
    const previous = this.container.querySelector<HTMLButtonElement>('#managed-previous'); if (previous) previous.disabled = blocked || this.offset === 0;
    const next = this.container.querySelector<HTMLButtonElement>('#managed-next'); if (next) next.disabled = blocked || this.offset + 50 >= this.filtered().length;
    this.container.setAttribute('aria-busy', String(this.loading));
  }

  private schedulePoll(): void {
    if (this.poll) clearTimeout(this.poll);
    this.poll = null;
    if (!this.visible || !this.options.isDesktop()) return;
    this.poll = setTimeout(() => {
      this.poll = null;
      // 编辑密码时不做自动重绘；手动刷新会明确结束编辑并清空密码。
      const editingSearch = document.activeElement instanceof HTMLInputElement && this.container.contains(document.activeElement);
      if (this.editor || editingSearch || this.options.isBlocked()) this.schedulePoll(); else void this.refresh(true);
    }, 5000);
  }
  async refresh(automatic = false): Promise<void> {
    if (this.loading || this.options.isBlocked() || !this.options.isDesktop()) return;
    const focus = this.activeFocus();
    const focusedElement = document.activeElement;
    if (!automatic) { this.clearPasswords(); this.editor = null; this.editorOrigin = null; }
    this.loading = true; this.sync();
    try {
      const response = await requestManaged({ action: 'status' }, this.options.request);
      this.snapshot = managedStatus(response.data); this.transport = response.transport;
      this.available = true; this.stale = false;
      if (this.feedback?.key === 'managedRefreshFailed' || this.feedback?.code === 'service_unavailable') this.feedback = null;
      this.offset = Math.min(this.offset, Math.max(0, Math.ceil(this.filtered().length / 50) - 1) * 50);
    } catch (error) {
      this.available = false; this.stale = !!this.snapshot;
      if (!this.snapshot) this.transport = null;
      this.feedback = this.snapshot ? { key: 'managedRefreshFailed', error: true } : { code: failureCode(error), error: true };
    } finally {
      this.loading = false;
      // 禁用按钮会让浏览器清除焦点；用户已转到其他控件时不再拉回。
      const restoreDisabledFocus = focus && (document.activeElement === document.body || document.activeElement === focusedElement);
      this.render();
      if (restoreDisabledFocus) this.restoreFocus(focus);
      this.schedulePoll();
    }
  }
  private filtered(): ServiceRoot[] {
    const query = this.query.toLowerCase();
    return (this.snapshot?.roots ?? []).filter(root => root.name.toLowerCase().includes(query) || root.note.toLowerCase().includes(query));
  }
  private open(kind: EditorKind, root?: ServiceRoot): void {
    if (!this.available || this.stale || this.options.isBlocked()) return;
    this.editorOrigin = this.activeFocus();
    this.clearPasswords(); this.editor = { kind, root }; this.render();
    const editor = this.element('managed-editor');
    const focus = Array.from(editor.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>('input:not([type=checkbox]), select, button'))
      .find(control => !control.disabled && control.getClientRects().length > 0);
    (focus ?? editor.querySelector<HTMLElement>('legend'))?.focus();
  }
  private close(): void {
    const focus = this.editorOrigin; this.editorOrigin = null;
    this.clearPasswords(); this.editor = null; this.render();
    if (focus) this.restoreFocus(focus, false); else this.element('managed-add').focus();
    this.schedulePoll();
  }
  private activeFocus(): { id: string | null; action: string } | null {
    const active = document.activeElement as HTMLElement | null; const row = active?.closest<HTMLElement>('.managed-root');
    const action = active?.dataset.managedAction ?? active?.dataset.focusAction;
    if (!action) return null;
    return { id: row ? this.rowIds.get(row) ?? null : null, action };
  }
  private restoreFocus(focus: ReturnType<ManagedProtectionUi['activeFocus']>, preventScroll = true): void {
    if (!focus) return;
    if (focus.id === null) {
      const target = this.container.querySelector<HTMLElement>(`[data-managed-action="${focus.action}"], [data-focus-action="${focus.action}"]`);
      (target && !(target instanceof HTMLButtonElement && target.disabled) ? target : this.element('managed-title')).focus({ preventScroll });
      return;
    }
    const row = Array.from(this.container.querySelectorAll<HTMLElement>('.managed-root')).find(row => this.rowIds.get(row) === focus.id);
    const button = row?.querySelector<HTMLButtonElement>(`[data-managed-action="${focus.action}"]`);
    (button && !button.disabled ? button : this.element('managed-title')).focus({ preventScroll });
  }
  private async execute(command: ManagedCommand, success: MessageKey, root?: ServiceRoot): Promise<void> {
    if (this.options.isBlocked() || !this.available || this.stale) { clearManagedCommand(command); return; }
    const focus = this.editorOrigin ?? this.activeFocus(); this.editorOrigin = null;
    this.clearPasswords(); this.editor = null;
    try { await this.options.run(async () => {
      let changed = false;
      try {
        const response = await requestManaged(command, this.options.request); this.transport = response.transport; changed = true;
        this.feedback = root && command.action !== 'remove' ? null : { key: success };
        if (root) this.rowFeedback.set(root.id, { key: success });
      } catch (error) {
        const feedback = { code: failureCode(error), error: true }; this.feedback = root ? null : feedback; if (root) this.rowFeedback.set(root.id, feedback);
      }
      // 变更失败也读取服务实际状态；不能用刷新错误覆盖原操作错误。
      try {
        const response = await requestManaged({ action: 'status' }, this.options.request);
        this.snapshot = managedStatus(response.data); this.transport = response.transport; this.available = true; this.stale = false;
      } catch {
        this.stale = true; this.available = false;
        if (changed) this.feedback = { key: 'managedSucceededRefreshFailed', error: true };
      }
      this.render();
    }); } finally { clearManagedCommand(command); }
    this.sync(); this.restoreFocus(focus); this.schedulePoll();
  }

  private render(): void {
    const focus = this.activeFocus();
    const helpOpen = this.container.querySelector<HTMLDetailsElement>('.managed-service-help')?.open ?? false;
    this.container.replaceChildren();
    this.container.className = 'records managed-protection';
    const header = document.createElement('div'); header.className = 'records-heading';
    const heading = document.createElement('h2'); heading.id = 'managed-title'; heading.tabIndex = -1; heading.textContent = t('managedTitle');
    const refresh = this.button('refreshServiceStatus', () => { void this.refresh(); }); refresh.id = 'managed-refresh'; refresh.dataset.focusAction = 'refresh'; header.append(heading, refresh); this.container.append(header);
    const status = this.paragraph(this.options.isDesktop() ? !this.available ? t(this.snapshot ? 'managedServiceLost' : 'managedServiceUnavailable') : t(this.transport?.healthy ? 'managedServiceConnected' : 'managedServiceDegraded') : t('managedServicePreview'), 'managed-service-status');
    status.id = 'managed-service-status'; status.setAttribute('role', 'status'); this.container.append(status);
    if (this.transport?.last_tick_error) this.container.append(this.paragraph(errorText(this.transport.last_tick_error), 'managed-service-error'));
    const details = document.createElement('details'); details.className = 'managed-service-help'; details.open = helpOpen;
    const summary = document.createElement('summary'); summary.dataset.focusAction = 'service-help'; summary.textContent = t('managedServiceDetails'); details.append(summary, this.paragraph(t('managedServiceInstallHelp')), this.paragraph(t('managedServiceScope')), this.paragraph(t('managedServicePasswordBoundary'))); this.container.append(details);
    if (this.feedback) { const feedback = this.paragraph(this.feedbackText(this.feedback), `protection-feedback${this.feedback.error ? ' error' : ''}`); feedback.id = 'managed-feedback'; feedback.setAttribute('role', this.feedback.error ? 'alert' : 'status'); this.container.append(feedback); }
    const tools = document.createElement('div'); tools.className = 'managed-tools';
    const add = this.button('managedAdd', () => this.open('enroll'), true); add.id = 'managed-add'; add.dataset.managedAction = 'enroll'; tools.append(add);
    const search = document.createElement('form'); search.className = 'protection-search'; search.setAttribute('role', 'search');
    const label = document.createElement('label'); label.className = 'sr-only'; label.htmlFor = 'managed-search'; label.textContent = t('searchManaged');
    const input = document.createElement('input'); input.id = 'managed-search'; input.type = 'search'; input.maxLength = 160; input.value = this.query; input.placeholder = t('searchManaged');
    const find = this.button('search', () => {}); find.type = 'submit'; search.append(label, input, find);
    search.onsubmit = event => { event.preventDefault(); this.clearPasswords(); this.editor = null; this.query = input.value.trim(); this.offset = 0; this.render(); this.element('managed-search').focus({ preventScroll: true }); };
    tools.append(search); this.container.append(tools);
    if (this.editor) this.renderEditor();
    const roots = this.filtered(); const list = document.createElement('ul'); list.id = 'managed-root-list';
    if (!roots.length) list.append(this.paragraph(t(this.snapshot?.registered ? 'noProtectionMatches' : 'managedEmpty'), 'empty-records'));
    for (const root of roots.slice(this.offset, this.offset + 50)) list.append(this.renderRoot(root));
    this.container.append(list);
    if (roots.length) {
      const footer = document.createElement('div'); footer.className = 'managed-pagination';
      footer.append(this.paragraph(t('rootSummary', { total: roots.length, from: this.offset + 1, to: Math.min(roots.length, this.offset + 50) })));
      const previous = this.button('previousPage', () => { this.clearPasswords(); this.editor = null; this.offset = Math.max(0, this.offset - 50); this.render(); }); previous.id = 'managed-previous';
      const next = this.button('nextPage', () => { this.clearPasswords(); this.editor = null; this.offset += 50; this.render(); }); next.id = 'managed-next';
      if (roots.length > 50) footer.append(previous, next); this.container.append(footer);
    }
    this.sync(); this.restoreFocus(focus);
  }

  private renderRoot(root: ServiceRoot): HTMLLIElement {
    const row = document.createElement('li'); row.className = 'managed-root'; this.rowIds.set(row, root.id);
    const identity = document.createElement('div'); identity.className = 'managed-identity';
    const name = document.createElement('strong'); name.textContent = root.name; identity.append(name);
    if (root.note) identity.append(this.paragraph(root.note, 'root-note'));
    identity.append(this.paragraph(t(modeKeys[root.mode])));
    identity.append(this.paragraph(t(root.password_required ? 'managedPasswordSet' : 'managedNoPassword')));
    const state = document.createElement('div'); state.className = 'managed-state';
    state.append(this.paragraph(t(root.monitoring ? 'managedMonitoringOn' : 'managedMonitoringOff'), 'managed-monitoring'));
    state.append(this.paragraph(root.restoring ? t('managedRestoring') : root.unlocked_until !== null ? t('managedUnlockedUntil', { time: new Date(root.unlocked_until * 1000).toLocaleString(language) }) : t('managedPolicyApplied')));
    if (root.last_error) state.append(this.paragraph(errorText(root.last_error), 'managed-service-error'));
    const actions = document.createElement('div'); actions.className = 'managed-root-actions';
    const append = (key: MessageKey, action: string, callback: () => void) => { const button = this.button(key, callback); button.dataset.managedAction = action; button.setAttribute('aria-label', `${t(key)}: ${root.name}${root.note ? ` · ${root.note}` : ''}`); actions.append(button); };
    if (root.password_required && root.unlocked_until === null && !root.restoring) append('managedUnlock', 'unlock', () => this.open('unlock', root));
    if (root.unlocked_until !== null && !root.restoring) append('managedRelock', 'relock', () => { void this.execute({ action: 'relock', id: root.id }, 'managedRelocked', root); });
    if (!root.restoring) {
      append('managedPasswordSettings', 'password', () => this.open('password', root));
      append(root.monitoring ? 'managedStopMonitoring' : 'managedStartMonitoring', 'continuous', () => this.open('continuous', root));
    }
    append('restoreAttributes', 'remove', () => this.open('remove', root));
    row.append(identity, state, actions);
    const feedback = this.rowFeedback.get(root.id); if (feedback) { const p = this.paragraph(this.feedbackText(feedback), `protection-feedback managed-row-feedback${feedback.error ? ' error' : ''}`); p.setAttribute('role', feedback.error ? 'alert' : 'status'); row.append(p); }
    return row;
  }

  private renderEditor(): void {
    const editor = this.editor!; const root = editor.root;
    const form = document.createElement('form'); form.id = 'managed-editor'; form.className = 'managed-editor'; form.autocomplete = 'off'; form.noValidate = true;
    const fieldset = document.createElement('fieldset'); const legend = document.createElement('legend');
    const titles = { enroll: 'managedAdd', unlock: 'managedUnlock', password: 'managedPasswordSettings', remove: 'confirmSelectedRestore', continuous: root?.monitoring ? 'managedStopMonitoring' : 'managedStartMonitoring' } as const;
    legend.textContent = t(titles[editor.kind]); legend.tabIndex = -1; fieldset.append(legend);
    if (root) { const name = document.createElement('strong'); name.textContent = `${root.name}${root.note ? ` · ${root.note}` : ''}`; fieldset.append(name); }
    const field = (key: MessageKey, id: string, type = 'text', required = false): HTMLInputElement => {
      const label = document.createElement('label'); label.htmlFor = id; label.textContent = t(key); const input = document.createElement('input'); input.id = id; input.type = type; input.required = required; input.autocomplete = type === 'password' ? 'off' : 'off';
      if (type === 'password') { input.maxLength = 1024; input.spellcheck = false; input.setAttribute('autocapitalize', 'off'); }
      label.append(input); fieldset.append(label); return input;
    };
    let path: HTMLInputElement | null = null; let note: HTMLInputElement | null = null; let mode: HTMLSelectElement | null = null;
    let password: HTMLInputElement | null = null; let newPassword: HTMLInputElement | null = null; let confirmation: HTMLInputElement | null = null; let setPassword: HTMLInputElement | null = null; let removePassword: HTMLInputElement | null = null;
    let duration: HTMLSelectElement | null = null;
    if (editor.kind === 'enroll') {
      fieldset.append(this.paragraph(t('managedEnrollHelp')));
      path = field('attributePath', 'managed-path', 'text', true); path.spellcheck = false;
      const selected = this.paragraph('', 'managed-selected-name'); selected.id = 'managed-selected-name'; path.oninput = () => { this.clearPasswords(); selected.textContent = nameOnly(path!.value); };
      fieldset.append(selected);
      const picks = document.createElement('div'); picks.className = 'selection-actions';
      for (const folder of [false, true]) picks.append(this.button(folder ? 'addProtectionFolders' : 'addProtectionFiles', () => {
        this.clearPasswords(); void this.options.run(async () => {
          try { const chosen = await this.options.choose(folder); if (chosen && path) { path.value = chosen; selected.textContent = nameOnly(chosen); } }
          catch { this.feedback = { key: 'dialogFailed', error: true }; this.clearPasswords(); this.render(); }
        });
      }));
      fieldset.append(picks); note = field('protectionNote', 'managed-note'); note.maxLength = 160; note.placeholder = t('notePlaceholder'); note.oninput = () => { this.clearPasswords(); note!.setCustomValidity(validProtectionNote(note!.value) ? '' : t('notePlaceholder')); };
      const label = document.createElement('label'); label.textContent = t('policy'); mode = document.createElement('select'); mode.id = 'managed-mode';
      for (const value of ['hidden-locked', 'locked', 'readonly-acl'] as const) { const option = document.createElement('option'); option.value = value; option.textContent = t(modeKeys[value]); mode.append(option); }
      mode.onchange = () => this.clearPasswords(); label.append(mode); fieldset.append(label);
      const checkbox = document.createElement('label'); checkbox.className = 'checkbox'; setPassword = document.createElement('input'); setPassword.id = 'managed-use-password'; setPassword.type = 'checkbox'; setPassword.checked = true;
      const caption = document.createElement('span'); caption.textContent = t('managedRequirePassword'); checkbox.append(setPassword, caption); fieldset.append(checkbox);
      newPassword = field('managedNewPassword', 'managed-new-password', 'password', true); confirmation = field('confirmPassword', 'managed-confirm-password', 'password', true);
      setPassword.onchange = () => { this.clearPasswords(); newPassword!.required = confirmation!.required = setPassword!.checked; newPassword!.disabled = confirmation!.disabled = !setPassword!.checked; };
      fieldset.append(this.paragraph(t('managedPasswordRules')));
    } else {
      if (editor.kind === 'unlock') fieldset.append(this.paragraph(t('managedUnlockHelp')));
      if (editor.kind === 'remove') fieldset.append(this.paragraph(t('managedRemoveHelp'), 'notice'));
      if (editor.kind === 'continuous') fieldset.append(this.paragraph(t(root?.monitoring ? 'managedStopHelp' : 'managedStartHelp'), 'notice'));
      if (root?.password_required || editor.kind === 'password') password = field('managedCurrentPassword', 'managed-current-password', 'password', !!root?.password_required);
      if (editor.kind === 'unlock') {
        const label = document.createElement('label'); label.textContent = t('managedAccessDuration'); duration = document.createElement('select'); duration.id = 'managed-duration';
        for (const minutes of [1, 5, 15, 30, 60]) { const option = document.createElement('option'); option.value = String(minutes * 60); option.textContent = t('managedMinutes', { n: minutes }); duration.append(option); }
        duration.value = '300'; duration.onchange = () => this.clearPasswords(); label.append(duration); fieldset.append(label);
      }
      if (editor.kind === 'password') {
        fieldset.append(this.paragraph(t('managedPasswordRules')));
        if (root?.password_required) {
          const label = document.createElement('label'); label.className = 'checkbox'; removePassword = document.createElement('input'); removePassword.id = 'managed-remove-password'; removePassword.type = 'checkbox'; const caption = document.createElement('span'); caption.textContent = t('managedClearPassword'); label.append(removePassword, caption); fieldset.append(label);
        }
        newPassword = field('managedNewPassword', 'managed-new-password', 'password', true); confirmation = field('confirmPassword', 'managed-confirm-password', 'password', true);
        if (removePassword) removePassword.onchange = () => { this.clearPasswords(); newPassword!.required = confirmation!.required = !removePassword!.checked; newPassword!.disabled = confirmation!.disabled = removePassword!.checked; };
      }
    }
    const error = this.paragraph('', 'managed-editor-error'); error.id = 'managed-editor-error'; error.setAttribute('role', 'alert'); error.hidden = true; fieldset.append(error);
    const actions = document.createElement('div'); actions.className = 'selection-actions'; const submit = this.button('managedConfirm', () => {}, true); submit.type = 'submit'; submit.dataset.managedAction = 'submit'; actions.append(submit, this.button('cancel', () => this.close())); fieldset.append(actions);
    form.append(fieldset); this.container.append(form);
    form.onsubmit = event => {
      event.preventDefault(); if (this.options.isBlocked() || !this.available || this.stale) { this.clearPasswords(); return; }
      let command: ManagedCommand;
      const invalid = (key: MessageKey) => { this.clearPasswords(); error.textContent = t(key); error.hidden = false; };
      if (editor.kind === 'enroll' && !path?.value.trim()) { invalid('requiredField'); return; }
      if (root?.password_required && password && !password.value) { invalid('requiredField'); return; }
      const hasNewPassword = editor.kind === 'enroll' ? !!setPassword?.checked : editor.kind === 'password' && !removePassword?.checked;
      if (hasNewPassword && !validManagedPassword(newPassword?.value ?? '')) { invalid('managedPasswordRules'); return; }
      if (hasNewPassword && newPassword?.value !== confirmation?.value) { invalid('passwordMismatch'); return; }
      if (note && !validProtectionNote(note.value)) { invalid('notePlaceholder'); return; }
      if (editor.kind === 'enroll') command = { action: 'enroll', path: path!.value.trim(), note: note!.value.trim(), mode: mode!.value as ManagedMode, password: hasNewPassword ? newPassword!.value : null };
      else if (!root) { invalid('error_internal'); return; }
      else if (editor.kind === 'unlock') command = { action: 'unlock', id: root.id, password: password!.value, duration_seconds: Number(duration!.value) };
      else if (editor.kind === 'password') command = { action: 'set_password', id: root.id, password: password?.value ?? '', new_password: hasNewPassword ? newPassword!.value : null };
      else if (editor.kind === 'continuous') command = { action: 'set_continuous', id: root.id, password: root.password_required ? password!.value : null, enabled: !root.monitoring };
      else command = { action: 'remove', id: root.id, password: root.password_required ? password!.value : null };
      this.clearPasswords();
      const successes = { enroll: 'managedEnrolled', unlock: 'managedUnlocked', password: 'managedPasswordChanged', remove: 'managedRemoved', continuous: 'managedMonitoringChanged' } as const;
      void this.execute(command, successes[editor.kind], root);
    };
  }
}
