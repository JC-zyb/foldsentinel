import { language, t } from './i18n.ts';
import { validProtectionNote } from './protection-workflow.ts';

export const ATTRIBUTE_PAGE_SIZE = 50;

export const PROTECTION_MODES = ['none', 'hidden', 'locked', 'readonly', 'readonly-acl', 'hidden-locked'] as const;
export type ProtectionMode = typeof PROTECTION_MODES[number] | 'hidden-readonly';
const modeKeys = { none: 'noProtection', hidden: 'hidden', locked: 'locked', readonly: 'readonly', 'readonly-acl': 'readonlyAcl', 'hidden-locked': 'hiddenLocked', 'hidden-readonly': 'hiddenReadonly' } as const;
export interface ManagedRoot { id: string; name: string; mode?: ProtectionMode | null; note?: string; duplicate_name?: boolean }
export interface RootVerification { state: 'consistent' | 'changed' | 'unavailable'; checked_at: number }
export interface RootRecordOptions {
  aclRestoreConfirmation?: { confirm: () => void; cancel: () => void };
  verify?: () => void;
  saveNote?: (note: string) => void;
  noteDraft?: string;
  onNoteDraft?: (note: string | undefined) => void;
  feedback?: { text: string; error?: boolean };
  verification?: RootVerification;
  busy?: boolean;
  stale?: boolean;
  selected?: boolean;
  select?: (selected: boolean) => void;
  pendingMode?: ProtectionMode;
  onPendingMode?: (mode: ProtectionMode) => void;
  noteEditorOpen?: boolean;
  onNoteEditorOpen?: (open: boolean) => void;
}

const rootControlSync = new WeakMap<HTMLElement, (busy: boolean, stale: boolean) => void>();
const rootRecordIds = new WeakMap<HTMLElement, string>();
let nextNameId = 0;

export function findRootRecord(container: HTMLElement, id: string): HTMLElement | undefined {
  return Array.from(container.querySelectorAll<HTMLElement>('.root-record')).find(item => rootRecordIds.get(item) === id);
}

export function syncRootControls(container: HTMLElement, busy: boolean, stale: boolean): void {
  rootControlSync.get(container)?.(busy, stale);
  container.querySelectorAll<HTMLElement>('.root-record').forEach(item => rootControlSync.get(item)?.(busy, stale));
}

export function fillProtectionModes(select: HTMLSelectElement, mode: ProtectionMode | null): void {
  select.replaceChildren();
  if (!mode) {
    const mixed = document.createElement('option'); mixed.value = ''; mixed.textContent = t('mixedProtection'); mixed.disabled = true; select.append(mixed);
  }
  for (const value of [...PROTECTION_MODES, ...(mode === 'hidden-readonly' ? ['hidden-readonly' as const] : [])]) {
    const option = document.createElement('option'); option.value = value; option.textContent = t(modeKeys[value]); select.append(option);
  }
  select.value = mode ?? '';
}

export function createRootRecord(root: ManagedRoot, restore: () => void, apply?: (mode: ProtectionMode) => void, options: RootRecordOptions = {}): HTMLLIElement {
  const item = document.createElement('li'); item.className = options.select ? 'root-record selectable-root' : 'root-record'; rootRecordIds.set(item, root.id);
  let busy = options.busy ?? false; let stale = options.stale ?? false;
  const action = (kind: string, label: string, callback: () => void, className = 'secondary') => {
    const button = document.createElement('button'); button.type = 'button'; button.className = className;
    button.dataset.rootAction = kind; button.textContent = label; button.onclick = callback;
    return button;
  };
  const identity = document.createElement('div'); identity.className = 'root-identity';
  if (options.select) {
    const label = document.createElement('label'); label.className = 'checkbox root-selection';
    const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.checked = options.selected ?? false;
    checkbox.dataset.rootAction = 'select'; checkbox.setAttribute('aria-label', t('selectProtectionItem', { name: root.name, note: root.note || t('noProtectionNote') }));
    checkbox.onchange = () => options.select?.(checkbox.checked);
    const caption = document.createElement('span'); caption.className = 'sr-only'; caption.textContent = t('selectItem');
    label.append(checkbox, caption); identity.append(label);
  }
  const name = document.createElement('span'); name.className = 'root-name'; name.textContent = root.name;
  name.id = `root-name-${nextNameId++}`;
  const nameToggle = action('expand-name', '', () => {
    const expanded = nameToggle.getAttribute('aria-expanded') !== 'true';
    nameToggle.setAttribute('aria-expanded', String(expanded));
    nameToggle.setAttribute('aria-label', `${t(expanded ? 'collapseName' : 'expandName')}: ${root.name}`);
    nameToggle.title = t(expanded ? 'collapseName' : 'expandName');
  }, 'root-name-toggle');
  nameToggle.setAttribute('aria-expanded', 'false'); nameToggle.setAttribute('aria-controls', name.id);
  nameToggle.setAttribute('aria-label', `${t('expandName')}: ${root.name}`);
  nameToggle.title = t('expandName');
  const arrow = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  arrow.setAttribute('viewBox', '0 0 16 16'); arrow.setAttribute('aria-hidden', 'true');
  const arrowPath = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  arrowPath.setAttribute('d', 'm4 6 4 4 4-4'); arrowPath.setAttribute('fill', 'none');
  arrowPath.setAttribute('stroke', 'currentColor'); arrowPath.setAttribute('stroke-width', '1.5'); arrow.append(arrowPath);
  const nameLine = document.createElement('div'); nameLine.className = 'root-name-line';
  nameToggle.append(name, arrow); nameLine.append(nameToggle); identity.append(nameLine);

  let noteInput: HTMLInputElement | null = null; let noteSave: HTMLButtonElement | null = null;
  let noteEditor: HTMLDivElement | undefined;
  const savedNote = root.note ?? '';
  if (savedNote) { const note = document.createElement('p'); note.className = 'root-note'; note.textContent = savedNote; identity.append(note); }
  if (root.duplicate_name) { const duplicate = document.createElement('p'); duplicate.className = 'root-duplicate'; duplicate.textContent = t('duplicateRootName'); identity.append(duplicate); }
  if (options.saveNote) {
    const editor = document.createElement('div'); editor.className = 'root-note-editor';
    noteEditor = editor; editor.id = `root-note-editor-${nextNameId++}`;
    const label = document.createElement('label'); label.textContent = t('protectionNote');
    noteInput = document.createElement('input'); noteInput.type = 'text'; noteInput.value = options.noteDraft ?? savedNote; noteInput.maxLength = 160;
    noteInput.placeholder = t('notePlaceholder'); noteInput.setAttribute('aria-label', `${t('protectionNote')}: ${root.name}`);
    const noteHelp = document.createElement('p'); noteHelp.className = 'field-help'; noteHelp.textContent = t('notePlaceholder');
    noteHelp.id = `root-note-help-${nextNameId++}`; noteInput.setAttribute('aria-describedby', noteHelp.id);
    label.append(noteInput); editor.append(label, noteHelp);
    noteSave = action('save-note', t('saveNote'), () => { if (noteInput && !noteSave?.disabled) options.saveNote?.(noteInput.value.trim()); });
    const edit = action('edit-note', t('editNote'), () => { setNoteEditorOpen(true); options.onNoteDraft?.(noteInput?.value ?? savedNote); noteInput?.focus(); }, 'text-button root-note-edit');
    edit.setAttribute('aria-controls', editor.id);
    function setNoteEditorOpen(open: boolean): void {
      editor.hidden = !open; edit.hidden = open; edit.setAttribute('aria-expanded', String(open)); options.onNoteEditorOpen?.(open);
    }
    const open = options.noteEditorOpen ?? options.noteDraft !== undefined;
    editor.hidden = !open; edit.hidden = open; edit.setAttribute('aria-expanded', String(open));
    const cancel = action('cancel-note', t('cancelNote'), () => { setNoteEditorOpen(false); if (noteInput) noteInput.value = savedNote; options.onNoteDraft?.(undefined); edit.focus(); updateControls(); }, 'text-button');
    const buttons = document.createElement('div'); buttons.className = 'root-note-actions'; buttons.append(noteSave, cancel); editor.append(buttons);
    noteInput.oninput = () => { options.onNoteDraft?.(noteInput?.value); updateControls(); };
    noteInput.dataset.rootAction = 'note-draft';
    noteInput.onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); if (!noteSave?.disabled) noteSave?.click(); } };
    editor.onkeydown = event => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setNoteEditorOpen(false); edit.focus(); }
    };
    nameLine.append(edit);
  }
  item.append(identity);

  const field = document.createElement('div'); field.className = 'root-policy';
  const modeLabel = document.createElement('label'); modeLabel.className = 'root-field-label'; modeLabel.textContent = t('registeredMode'); field.append(modeLabel);
  let select: HTMLSelectElement | null = null; let changed: HTMLParagraphElement | null = null;
  if (apply) {
    select = document.createElement('select'); select.setAttribute('aria-label', t('rootProtection', { name: root.name }));
    select.id = `root-mode-${nextNameId++}`; modeLabel.htmlFor = select.id;
    fillProtectionModes(select, root.mode ?? null);
    select.dataset.rootAction = 'mode';
    if (options.pendingMode) select.value = options.pendingMode;
    field.append(select);
    changed = document.createElement('p'); changed.className = 'root-mode-pending'; changed.setAttribute('role', 'status');
    select.onchange = () => {
      if (!select?.value || busy || stale) return;
      const mode = select.value as ProtectionMode;
      options.onPendingMode?.(mode); updateControls(); apply(mode);
    };
    field.append(changed);
  } else {
    const modeValue = document.createElement('p'); modeValue.className = 'root-mode-value'; modeValue.textContent = t(root.mode ? modeKeys[root.mode] : 'mixedProtection'); field.append(modeValue);
  }
  item.append(field);

  const status = document.createElement('div'); status.className = 'root-status';
  const statusLabel = document.createElement('span'); statusLabel.className = 'root-field-label'; statusLabel.textContent = t('verificationStatus'); status.append(statusLabel);
  const verification = document.createElement('p'); verification.className = `root-verification ${options.verification?.state ?? 'unchecked'}`;
  const stateKeys = { consistent: 'verificationConsistent', changed: 'verificationChanged', unavailable: 'verificationUnavailable' } as const;
  verification.textContent = t(options.verification ? stateKeys[options.verification.state] : 'verificationUnknown');
  status.append(verification);
  if (options.verification) {
    const time = document.createElement('p'); time.className = 'root-verification-time';
    time.textContent = t('verificationTime', { time: new Date(options.verification.checked_at).toLocaleString(language) }); status.append(time);
  }
  const actions = document.createElement('div'); actions.className = 'root-actions';
  if (options.verify) actions.append(action('verify', t('verifyProtection'), options.verify, 'text-button'));
  const restoreButton = action('restore', t('restoreAttributes'), restore);
  restoreButton.setAttribute('aria-label', t('restoreManagedRoot', { name: root.name })); actions.append(restoreButton);
  item.append(status, actions);
  if (noteEditor) item.append(noteEditor);
  if (options.feedback) {
    const feedback = document.createElement('p'); feedback.className = `root-feedback${options.feedback.error ? ' is-error' : ''}`;
    feedback.setAttribute('role', options.feedback.error ? 'alert' : 'status'); feedback.textContent = options.feedback.text; item.append(feedback);
  }
  if (options.aclRestoreConfirmation) {
    const panel = document.createElement('section'); panel.className = 'protection-confirm root-acl-confirm';
    panel.setAttribute('aria-label', t('aclRestoreTitle'));
    const heading = document.createElement('h3'); heading.textContent = t('aclRestoreTitle');
    const help = document.createElement('p'); help.textContent = t('aclRestoreHelp');
    const controls = document.createElement('div'); controls.className = 'selection-actions';
    controls.append(action('confirm-acl-restore', t('confirmAclRestore'), options.aclRestoreConfirmation.confirm, 'primary'),
      action('cancel-acl-restore', t('cancel'), options.aclRestoreConfirmation.cancel));
    panel.append(heading, help, controls); item.append(panel);
  }

  function updateControls(): void {
    const blocked = busy || stale;
    item.setAttribute('aria-busy', String(busy));
    item.querySelectorAll<HTMLButtonElement | HTMLSelectElement | HTMLInputElement>('button, select, input').forEach(control => { control.disabled = blocked; });
    // 名称展开只影响显示，刷新失败后仍可阅读完整名称。
    nameToggle.disabled = busy;
    if (select && changed) {
      const pending = !!select.value && select.value !== root.mode;
      changed.textContent = pending ? t('pendingMode') : '';
      changed.hidden = !pending;
    }
    if (noteInput && noteSave) {
      const value = noteInput.value.trim();
      const valid = validProtectionNote(value);
      noteInput.setCustomValidity(valid ? '' : t('notePlaceholder'));
      noteInput.setAttribute('aria-invalid', String(!valid));
      noteSave.disabled = blocked || !valid || value === savedNote;
    }
  }
  rootControlSync.set(item, (nextBusy, nextStale) => { busy = nextBusy; stale = nextStale; updateControls(); });
  updateControls();
  return item;
}
