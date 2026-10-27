import { t, type MessageKey } from './i18n.ts';

export const ATTRIBUTE_PAGE_SIZE = 50;

export interface ManagedRoot { path: string; entry_count: number }

export interface ManagedStatus {
  entry: { path: string; mode: 'hidden' | 'readonly' | 'hidden-readonly'; original_attributes: number; managed_attributes: number };
  current_attributes: number | null;
  expected_attributes: number;
  state: 'consistent' | 'changed' | 'missing' | 'unreadable';
  os_code?: number;
}

export function attributeKeys(value: number | null): MessageKey[] {
  if (value === null) return ['attributeUnavailable'];
  const flags = [[0x2, 'attributeHidden'], [0x4, 'attributeSystem'], [0x1, 'attributeReadonly'], [0x10, 'attributeDirectory'], [0x20, 'attributeArchive']] as const;
  const keys: MessageKey[] = flags.filter(([bit]) => (value & bit) !== 0).map(([, key]) => key);
  return keys.length ? keys : ['attributeNone'];
}

export const attributeStateKeys: Record<ManagedStatus['state'], MessageKey> = {
  consistent: 'attributeConsistent', changed: 'attributeChanged', missing: 'attributeMissing', unreadable: 'attributeUnreadable',
};

function attributeFlags(value: number | null): string {
  return attributeKeys(value).map(key => t(key)).join(' · ') + (value === null ? '' : ` (0x${value.toString(16).toUpperCase()})`);
}

export function createAttributeRecord(status: ManagedStatus, select: () => void): HTMLLIElement {
  const entry = status.entry;
  const item = document.createElement('li');
  const record = document.createElement('button'); record.type = 'button'; record.className = 'record';
  const path = document.createElement('span'); path.textContent = entry.path.replace(/^\\\\\?\\/, '');
  const details = document.createElement('span'); details.className = 'attribute-details';
  const add = (text: string, className = '') => { const line = document.createElement('small'); line.textContent = text; line.className = className; details.append(line); };
  add(t(entry.mode === 'hidden-readonly' ? 'hiddenReadonly' : entry.mode === 'hidden' ? 'hidden' : 'readonly'));
  add(t('attributeStatus', { state: t(attributeStateKeys[status.state]) }), `attribute-state ${status.state}`);
  add(t('attributeOriginal', { flags: attributeFlags(entry.original_attributes) }));
  add(t('attributeCurrent', { flags: attributeFlags(status.current_attributes) }));
  add(t('attributeExpected', { flags: attributeFlags(status.expected_attributes) }));
  if (entry.managed_attributes === 0x3 && entry.mode !== 'readonly') add(t('attributeLegacy'));
  if (status.os_code !== undefined) add(t('attributeOsCode', { code: status.os_code }));
  record.append(path, details); record.onclick = select; item.append(record);
  return item;
}

export function createRootRecord(root: ManagedRoot, select: () => void, restore: () => void): HTMLLIElement {
  const item = document.createElement('li'); item.className = 'root-record';
  const choose = document.createElement('button'); choose.type = 'button'; choose.className = 'root-path';
  choose.textContent = root.path.replace(/^\\\\\?\\/, '');
  choose.setAttribute('aria-label', t('selectManagedRoot', { path: choose.textContent })); choose.onclick = select;
  const count = document.createElement('span'); count.className = 'field-help'; count.textContent = t('managedRootCount', { n: root.entry_count });
  const action = document.createElement('button'); action.type = 'button'; action.className = 'secondary';
  action.textContent = t('restoreAttributes'); action.setAttribute('aria-label', t('restoreManagedRoot', { path: choose.textContent })); action.onclick = restore;
  item.append(choose, count, action);
  return item;
}
