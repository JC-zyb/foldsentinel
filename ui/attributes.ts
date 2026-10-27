import { t } from './i18n.ts';

export const ATTRIBUTE_PAGE_SIZE = 50;

export interface ManagedRoot { id: string; name: string }

export function createRootRecord(root: ManagedRoot, restore: () => void): HTMLLIElement {
  const item = document.createElement('li'); item.className = 'root-record';
  const name = document.createElement('span'); name.className = 'root-name'; name.textContent = root.name;
  const action = document.createElement('button'); action.type = 'button'; action.className = 'secondary';
  action.textContent = t('restoreAttributes'); action.setAttribute('aria-label', t('restoreManagedRoot', { name: root.name })); action.onclick = restore;
  item.append(name, action);
  return item;
}
