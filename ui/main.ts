import { invoke, isTauri } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import './style.css';

const shield = '<svg viewBox="0 0 32 32" fill="none" aria-hidden="true"><path d="M16 3 27 7v8c0 7-7 12-11 14C12 27 5 22 5 15V7l11-4Z" stroke="currentColor" stroke-width="2"/><path d="m11 16 3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>';
document.querySelector<HTMLDivElement>('#app')!.innerHTML = `
  <aside class="sidebar">
    <div class="brand"><span class="brand-icon">${shield}</span><div>FoldSentinel<small>文件夹哨兵</small></div></div>
    <p class="nav-label">本地保护</p>
    <nav aria-label="功能导航"><button id="nav-vault" class="nav-item selected" aria-current="page">加密与恢复 <span>01</span></button><button id="nav-attributes" class="nav-item">隐藏与只读 <span>02</span></button></nav>
    <div class="sidebar-footer"><span class="local-dot"></span> 数据仅在本机处理<small>开源 · MIT · v0.1.0</small></div>
  </aside>
  <main>
    <header><div><p class="eyebrow">YOUR FILES. YOUR CONTROL.</p><h1 id="page-title">加密与恢复</h1><p id="page-description">为文件和文件夹生成独立的密码加密包。</p></div><span class="version">Windows · 0.1</span></header>
    <section id="vault-panel" aria-label="加密与恢复">
      <div class="segmented" role="group" aria-label="操作类型"><button id="encrypt-tab" class="active" aria-pressed="true">创建加密包</button><button id="decrypt-tab" aria-pressed="false">恢复加密包</button></div>
      <form id="vault-form" class="panel">
        <fieldset id="vault-fields"><legend class="sr-only">加密参数</legend>
          <div class="section-heading"><span class="step">1</span><div><h2 id="source-title">选择要保护的内容</h2><p id="source-help">支持普通文件或文件夹，归档上限 64 MiB / 10,000 项。</p></div></div>
          <label for="input-path">源路径</label><div class="path-row"><input id="input-path" required spellcheck="false" autocomplete="off" placeholder="输入完整路径，或使用右侧按钮"><button id="pick-file" type="button" class="secondary">选文件</button><button id="pick-folder" type="button" class="secondary">选文件夹</button></div>
          <label for="output-path" id="output-label">加密包保存位置</label><div class="path-row"><input id="output-path" required spellcheck="false" autocomplete="off" placeholder="选择一个尚不存在的输出名称"><button id="pick-output" type="button" class="secondary">选择位置</button></div>
          <p class="field-help" id="output-help">建议使用 .fse 后缀；加密包应保存在源文件夹之外。</p>
          <div class="divider"></div>
          <div class="section-heading"><span class="step">2</span><div><h2 id="password-title">设置加密密码</h2><p id="password-help">建议使用较长的独立密码。密码遗失后无法恢复内容。</p></div></div>
          <div class="password-grid"><div><label for="password">密码</label><input id="password" type="password" required maxlength="256" autocomplete="new-password"></div><div id="confirm-field"><label for="confirm-password">确认密码</label><input id="confirm-password" type="password" required maxlength="256" autocomplete="new-password"></div></div>
          <label class="checkbox"><input id="show-password" type="checkbox"> 显示密码</label>
          <div class="notice" id="vault-notice">加密后源文件仍然存在。请先验证恢复结果，再自行决定如何保管原件。</div>
          <div class="form-footer"><span id="crypto-description">Argon2id + AES-256-GCM</span><button class="primary" id="vault-submit" type="submit">创建加密包 <span aria-hidden="true">→</span></button></div>
        </fieldset>
      </form>
    </section>
    <section id="attributes-panel" aria-label="隐藏与只读" hidden>
      <div class="notice attribute-notice">文件属性用于减少误操作，不提供密码加密。能修改属性的用户仍可访问内容。</div>
      <form id="attribute-form" class="panel"><fieldset id="attribute-fields"><legend class="sr-only">属性保护</legend>
        <h2>管理文件属性</h2><label for="attribute-path">文件或文件夹路径</label><div class="path-row"><input id="attribute-path" required spellcheck="false" autocomplete="off"><button id="pick-attribute-file" class="secondary" type="button">选文件</button><button id="pick-attribute-folder" class="secondary" type="button">选文件夹</button></div>
        <label for="attribute-mode">应用策略</label><select id="attribute-mode"><option value="hidden-readonly">隐藏 + 只读</option><option value="hidden">仅隐藏</option><option value="readonly">仅只读</option></select>
        <p class="field-help">递归处理普通文件和目录；遇到无权限路径或重解析点会停止并显示错误。</p>
        <div class="attribute-actions"><button class="secondary" id="restore-attributes" type="button">恢复原始属性</button><button class="primary" type="submit">应用属性</button></div>
      </fieldset></form>
      <div class="records"><div class="records-heading"><h2>管理记录 <span id="record-count">0</span></h2><button id="refresh-records" class="text-button">刷新</button></div><p class="field-help">点击记录将路径填入上方；记录表示恢复信息，当前属性可能被其他程序修改。</p><ul id="record-list"></ul><p id="state-location" class="field-help"></p></div>
    </section>
    <div id="result" role="status" aria-live="polite" tabindex="-1" hidden></div>
    <footer class="main-footer"><span id="activity">就绪</span><span>源数据保留 · 输出不覆盖</span></footer>
  </main>`;

function element<T extends HTMLElement = HTMLElement>(id: string): T { return document.getElementById(id) as T; }
const input = (id: string) => element<HTMLInputElement>(id);
let decrypt = false;
let busy = false;

function result(message: string, error = false): void {
  const box = element('result'); box.textContent = message; box.className = error ? 'result error' : 'result success'; box.hidden = false;
  box.setAttribute('role', error ? 'alert' : 'status'); box.focus();
}

function setBusy(value: boolean): void {
  busy = value;
  element<HTMLFieldSetElement>('vault-fields').disabled = value;
  element<HTMLFieldSetElement>('attribute-fields').disabled = value;
  for (const id of ['encrypt-tab', 'decrypt-tab', 'nav-vault', 'nav-attributes', 'refresh-records']) element<HTMLButtonElement>(id).disabled = value;
  element('activity').textContent = value ? '正在处理，请保持窗口打开…' : '就绪';
  element('app').setAttribute('aria-busy', String(value));
}

async function run(operation: () => Promise<void>): Promise<void> {
  if (busy) return;
  if (!isTauri()) { result('请通过 FoldSentinel 桌面程序运行文件操作。', true); return; }
  element('result').hidden = true; setBusy(true);
  try { await operation(); } catch (error) { result(String(error), true); } finally { setBusy(false); }
}

function clearPasswords(): void {
  input('password').value = ''; input('confirm-password').value = '';
  input('show-password').checked = false; input('password').type = 'password'; input('confirm-password').type = 'password';
}

function setMode(value: boolean): void {
  decrypt = value; clearPasswords(); element('result').hidden = true;
  for (const [id, active] of [['encrypt-tab', !value], ['decrypt-tab', value]] as const) { element(id).classList.toggle('active', active); element(id).setAttribute('aria-pressed', String(active)); }
  element('pick-folder').hidden = value; element('confirm-field').hidden = value; input('confirm-password').required = !value;
  input('input-path').value = ''; input('output-path').value = '';
  element('source-title').textContent = value ? '选择要恢复的加密包' : '选择要保护的内容';
  element('source-help').textContent = value ? '支持由 FoldSentinel v0.1 创建的 .fse 加密包。' : '支持普通文件或文件夹，归档上限 64 MiB / 10,000 项。';
  element('output-label').textContent = value ? '恢复位置（新文件或新文件夹的完整名称）' : '加密包保存位置';
  element('output-help').textContent = value ? '文件夹包会生成新文件夹；文件包会生成新文件。目标必须尚不存在。' : '建议使用 .fse 后缀；加密包应保存在源文件夹之外。';
  element('password-title').textContent = value ? '输入加密包密码' : '设置加密密码';
  element('password-help').textContent = value ? '认证通过后才恢复内容。密码错误或文件损坏会停止操作。' : '建议使用较长的独立密码。密码遗失后无法恢复内容。';
  input('password').autocomplete = value ? 'current-password' : 'new-password';
  element('vault-notice').textContent = value ? '恢复到新位置，不覆盖已有内容；原加密包会保留。' : '加密后源文件仍然存在。请先验证恢复结果，再自行决定如何保管原件。';
  element('vault-submit').textContent = value ? '恢复内容 →' : '创建加密包 →';
}
element('encrypt-tab').onclick = () => setMode(false);
element('decrypt-tab').onclick = () => setMode(true);
input('show-password').onchange = () => { const type = input('show-password').checked ? 'text' : 'password'; input('password').type = type; input('confirm-password').type = type; };

async function chooseSource(folder: boolean, target: string): Promise<void> {
  await run(async () => {
    const path = await open({ directory: folder, multiple: false, title: folder ? '选择文件夹' : '选择文件' });
    if (typeof path === 'string') {
      input(target).value = path;
      if (target === 'input-path') input('output-path').value = decrypt ? path.replace(/\.fse$/i, '') + '.restored' : path + '.fse';
    }
  });
}
element('pick-file').onclick = () => { void chooseSource(false, 'input-path'); };
element('pick-folder').onclick = () => { void chooseSource(true, 'input-path'); };
element('pick-attribute-file').onclick = () => { void chooseSource(false, 'attribute-path'); };
element('pick-attribute-folder').onclick = () => { void chooseSource(true, 'attribute-path'); };
element('pick-output').onclick = () => { void run(async () => { const path = await save({ title: decrypt ? '指定新恢复位置' : '保存加密包', defaultPath: input('output-path').value || undefined }); if (path) input('output-path').value = path; }); };

interface VaultReport { input_bytes: number; output_bytes: number; directory: boolean }
element<HTMLFormElement>('vault-form').onsubmit = (event) => {
  event.preventDefault();
  if (!decrypt && input('password').value !== input('confirm-password').value) { result('两次输入的密码不一致。', true); input('confirm-password').focus(); return; }
  void run(async () => {
    const output = input('output-path').value.trim();
    try {
      const report = await invoke<VaultReport>('vault_operation', { input: input('input-path').value.trim(), output, password: input('password').value, decrypt });
      result(`${decrypt ? '恢复' : '加密'}完成 · ${report.directory ? '文件夹' : '文件'}\n输出：${output}\n${report.output_bytes.toLocaleString()} 字节。源数据已保留。`);
    } finally { clearPasswords(); }
  });
};

interface Snapshot { entries: { path: string; mode: string }[]; state_path: string }
async function refreshRecords(): Promise<void> {
  const snapshot = await invoke<Snapshot>('protection_status');
  element('record-count').textContent = String(snapshot.entries.length);
  const list = element('record-list'); list.replaceChildren();
  if (!snapshot.entries.length) { const empty = document.createElement('li'); empty.className = 'empty-records'; empty.textContent = '暂无管理记录。应用属性后，恢复信息会保存在本机。'; list.append(empty); }
  for (const entry of snapshot.entries.slice(0, 200)) {
    const item = document.createElement('li'); const button = document.createElement('button'); button.type = 'button'; button.className = 'record';
    const path = document.createElement('span'); path.textContent = entry.path.replace(/^\\\\\?\\/, '');
    const mode = document.createElement('small'); mode.textContent = ({ hidden: '隐藏', readonly: '只读', 'hidden-readonly': '隐藏 + 只读' } as Record<string, string>)[entry.mode] || entry.mode;
    button.append(path, mode); button.onclick = () => { input('attribute-path').value = entry.path; input('attribute-path').focus(); }; item.append(button); list.append(item);
  }
  element('state-location').textContent = `${snapshot.entries.length > 200 ? '仅显示前 200 项。' : ''}恢复记录：${snapshot.state_path}`;
}
async function attributes(restore: boolean): Promise<void> {
  if (!input('attribute-path').reportValidity()) return;
  await run(async () => {
    const report = await invoke<{ changed: number; missing: number }>('attribute_operation', { path: input('attribute-path').value.trim(), mode: element<HTMLSelectElement>('attribute-mode').value, restore });
    result(`${restore ? '属性恢复' : '属性应用'}完成：修改 ${report.changed} 项，缺失 ${report.missing} 项。`);
    await refreshRecords();
  });
}
element<HTMLFormElement>('attribute-form').onsubmit = event => { event.preventDefault(); void attributes(false); };
element('restore-attributes').onclick = () => { void attributes(true); };
element('refresh-records').onclick = () => { void run(refreshRecords); };
for (const page of ['vault', 'attributes']) {
  element(`nav-${page}`).onclick = () => {
    element('vault-panel').hidden = page !== 'vault'; element('attributes-panel').hidden = page !== 'attributes'; element('result').hidden = true; clearPasswords();
    for (const item of ['vault', 'attributes']) { element(`nav-${item}`).classList.toggle('selected', item === page); if (item === page) element(`nav-${item}`).setAttribute('aria-current', 'page'); else element(`nav-${item}`).removeAttribute('aria-current'); }
    element('page-title').textContent = page === 'vault' ? '加密与恢复' : '隐藏与只读';
    element('page-description').textContent = page === 'vault' ? '为文件和文件夹生成独立的密码加密包。' : '应用可恢复的 Windows 文件属性，减少日常误操作。';
    if (page === 'attributes') void run(refreshRecords);
  };
}
if (!isTauri()) element('activity').textContent = '界面预览 · 文件操作需桌面程序';
