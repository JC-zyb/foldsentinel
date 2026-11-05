import { language, t, type MessageKey } from './i18n';

export interface UpdateCheck {
  current_version: string;
  latest_version: string | null;
  status: 'available' | 'up_to_date' | 'ahead' | 'no_release';
  release_url: string | null;
  published_at: string | null;
  notes: string | null;
  prerelease: boolean;
}
interface UpdateDependencies {
  isDesktop: () => boolean;
  check: () => Promise<UpdateCheck>;
  open: (url: string) => Promise<void>;
}
const errorMessages: Record<string, MessageKey> = {
  network: 'updateNetworkError', timeout: 'updateTimeout', rate_limited: 'updateRateLimited',
  invalid_response: 'updateInvalidResponse', unavailable: 'updateUnavailable', busy: 'updateBusy',
};

// 更新检查与文件任务相互独立；结果只保留在本次窗口，渲染不会移动用户焦点。
export class UpdatesUi {
  private busy = false;
  private opening = false;
  private result: UpdateCheck | null = null;
  private error: MessageKey | null = null;
  private checkedAt: Date | null = null;
  private openFailed = false;

  constructor(private container: HTMLElement, private dependencies: UpdateDependencies) {
    container.innerHTML = `<div class="settings-row settings-update-row"><div><strong id="update-label"></strong><p id="update-help" class="field-help"></p></div><button id="check-updates" type="button" class="secondary" aria-describedby="update-help"></button></div>
      <p id="update-status" class="settings-feedback" role="status" aria-live="polite" aria-atomic="true"></p>
      <p id="update-checked-at" class="field-help" hidden></p>
      <div id="update-release" hidden><p id="update-published-at" class="field-help" hidden></p><details id="update-notes" hidden><summary id="update-notes-label"></summary><p id="update-notes-text"></p></details><div class="selection-actions"><button id="open-release" type="button" class="secondary"></button></div><p id="update-open-error" class="field-help update-error" role="status" hidden></p></div>`;
    this.element<HTMLButtonElement>('check-updates').onclick = () => { void this.check(); };
    this.element<HTMLButtonElement>('open-release').onclick = () => { void this.open(); };
    this.translate();
  }

  private element<T extends HTMLElement = HTMLElement>(id: string): T {
    return this.container.querySelector<T>(`#${id}`)!;
  }

  translate(): void {
    this.element('update-label').textContent = t('softwareUpdates');
    this.element('update-help').textContent = t('updateHelp');
    const button = this.element<HTMLButtonElement>('check-updates');
    button.textContent = t(this.busy ? 'checkingUpdates' : 'checkUpdates');
    button.disabled = this.busy || !this.dependencies.isDesktop();
    button.setAttribute('aria-busy', String(this.busy));
    const status = this.element('update-status');
    const key = !this.dependencies.isDesktop() ? 'updateDesktopOnly' : this.busy ? 'checkingUpdates' : this.error ?? (this.result ? ({
      available: 'updateAvailable', up_to_date: 'updateCurrent', ahead: 'updateAhead', no_release: 'updateNoRelease',
    } as const)[this.result.status] : 'updateNotChecked');
    status.textContent = t(key, { version: this.result?.latest_version ?? '' });
    status.classList.toggle('update-error', !!this.error);
    const checkedAt = this.element('update-checked-at');
    checkedAt.hidden = this.checkedAt === null;
    checkedAt.textContent = this.checkedAt ? t('updateCheckedAt', { time: this.checkedAt.toLocaleString(language) }) : '';
    const release = this.result;
    this.element('update-release').hidden = !release?.release_url;
    const published = release?.published_at ? new Date(release.published_at) : null;
    const publication = this.element('update-published-at');
    publication.hidden = !published || Number.isNaN(published.getTime());
    publication.textContent = published && !Number.isNaN(published.getTime()) ? t(release?.prerelease ? 'updatePreviewPublished' : 'updatePublished', { date: published.toLocaleDateString(language) }) : '';
    this.element('update-notes').hidden = !release?.notes;
    this.element('update-notes-label').textContent = t('updateNotes');
    this.element('update-notes-text').textContent = release?.notes ?? '';
    const openButton = this.element<HTMLButtonElement>('open-release');
    openButton.textContent = t(this.opening ? 'updateOpening' : 'viewRelease');
    openButton.disabled = this.opening || this.busy;
    this.element('update-open-error').hidden = !this.openFailed;
    this.element('update-open-error').textContent = this.openFailed ? t('updateOpenFailed') : '';
  }

  private async check(): Promise<void> {
    if (this.busy || !this.dependencies.isDesktop()) return;
    this.busy = true; this.error = null; this.result = null; this.checkedAt = null; this.openFailed = false;
    this.element<HTMLDetailsElement>('update-notes').open = false;
    this.translate();
    try {
      this.result = await this.dependencies.check();
      this.checkedAt = new Date();
    } catch (error) {
      this.error = typeof error === 'string' ? errorMessages[error] ?? 'updateUnavailable' : 'updateUnavailable';
      console.error('Update check failed', error);
    } finally { this.busy = false; this.translate(); }
  }

  private async open(): Promise<void> {
    if (!this.result?.release_url || this.opening || this.busy) return;
    this.opening = true; this.openFailed = false; this.translate();
    try { await this.dependencies.open(this.result.release_url); }
    catch (error) { this.openFailed = true; console.error('Open release failed', error); }
    finally { this.opening = false; this.translate(); }
  }
}
