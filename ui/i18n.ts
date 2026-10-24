export type Language = 'zh-CN' | 'en';
export const messages = {
  eyebrow: ['你的文件，由你掌控。', 'YOUR FILES. YOUR CONTROL.'],
  requiredField: ['请填写此字段。', 'Please fill in this field.'],
  brand: ['文件夹哨兵', 'Folder protection'],
  local: ['本地保护', 'LOCAL PROTECTION'],
  vault: ['加密、恢复与验证', 'Encrypt, restore & verify'],
  attributes: ['隐藏与只读', 'Hidden & read-only'],
  localOnly: ['数据仅在本机处理', 'Processed on this device'],
  license: ['开源 · MIT · v0.3.0', 'Open source · MIT · v0.3.0'],
  vaultDescription: ['为文件和文件夹生成密码加密包，或验证已有加密包。', 'Protect files and folders with a password, or check an existing package.'],
  attributesDescription: ['应用可恢复的 Windows 文件属性，减少日常误操作。', 'Apply reversible Windows file attributes to reduce accidental changes.'],
  language: ['界面语言', 'Language'],
  encrypt: ['创建加密包', 'Encrypt'], decrypt: ['恢复加密包', 'Restore'], verify: ['验证加密包', 'Verify'],
  selection: ['选择内容', 'Choose content'],
  sourceHelp: ['可分次选择或拖入文件与文件夹，自动追加并去重。最多排队 100 项；每个任务上限 1 TiB / 100,000 个条目。', 'Choose or drop files and folders in batches; new paths are appended without duplicates. Up to 100 queued tasks; each task supports 1 TiB / 100,000 entries.'],
  packageHelp: ['支持 v1、v2 加密包；v1 保留 64 MiB 上限。验证只读取数据，不生成恢复文件。', 'Supports v1 and v2 packages; v1 keeps its 64 MiB limit. Verification reads content without restoring files.'],
  source: ['源路径（每行一个）', 'Source paths (one per line)'],
  sourcePlaceholder: ['输入完整路径，或选择 / 拖入多个文件', 'Enter full paths, choose files, or drop them here'],
  pickFiles: ['选择文件', 'Choose files'], pickFolders: ['选择文件夹', 'Choose folders'],
  outputDirectory: ['输出目录（可选）', 'Output folder (optional)'],
  outputPlaceholder: ['留空保存在每个源文件的父目录', 'Leave empty to use each source’s parent folder'],
  chooseDirectory: ['选择目录', 'Choose folder'],
  outputHelp: ['自动建议新名称，避开已有文件；加入队列后可检查完整输出路径。加密包须保存在源文件夹之外。', 'New names avoid existing files. Review full output paths in the queue. Keep packages outside source folders.'],
  passwordTitle: ['密码', 'Password'],
  passwordHelp: ['这批任务使用同一密码。仅在内存中保留到任务结束或取消；重试需重新输入。密码遗失后无法恢复内容。', 'This batch uses one password, kept in memory until completion or cancellation. Enter it again to retry. Lost passwords cannot be recovered.'],
  password: ['密码', 'Password'], confirmPassword: ['确认密码', 'Confirm password'], showPassword: ['显示密码', 'Show password'],
  notice: ['源数据始终保留，输出禁止覆盖。加密后请先验证并试恢复，再决定如何保管原件。', 'Source data is kept and outputs never overwrite files. Verify and test a restore before deciding how to store the originals.'],
  verifyNotice: ['完整检查密码认证、终块和归档路径；不会创建恢复文件或临时明文。', 'Checks authentication, the final block and archive paths, without creating restored files or temporary plaintext.'],
  add: ['加入队列', 'Add to queue'],
  queue: ['任务队列与历史', 'Queue & history'],
  queueHelp: ['任务串行执行。历史仅保存在本机，包含路径和状态，不保存密码或密钥。异常退出后的任务需重新输入密码。', 'Tasks run one at a time. Local history contains paths and status, without passwords or keys. Interrupted tasks need a password to retry.'],
  start: ['开始 / 继续队列', 'Start / resume'], pause: ['当前任务后暂停', 'Pause after current'],
  clearHistory: ['清空历史', 'Clear history'], cancel: ['取消', 'Cancel'], retry: ['重填并重试', 'Refill to retry'],
  emptyQueue: ['尚无任务。选择内容并输入密码后加入队列。', 'No tasks yet. Choose content and enter a password to add tasks.'],
  waiting: ['等待处理', 'Waiting'], running: ['正在处理', 'Running'], succeeded: ['完成', 'Completed'], failed: ['失败', 'Failed'], cancelled: ['已取消', 'Cancelled'], interrupted: ['已中断，需重试', 'Interrupted; retry needed'],
  queueSummary: ['等待 {waiting} · 运行 {running} · 完成 {done} · 失败 {failed} · 取消 / 中断 {stopped}', 'Waiting {waiting} · Running {running} · Completed {done} · Failed {failed} · Cancelled / interrupted {stopped}'],
  output: ['输出：{path}', 'Output: {path}'], noOutput: ['只读验证，无输出', 'Read-only verification; no output'],
  preparing: ['准备任务', 'Preparing'], scanning: ['扫描文件清单', 'Scanning files'], deriving: ['正在派生密钥', 'Deriving key'], encrypting: ['正在加密', 'Encrypting'], decrypting: ['正在验证并恢复', 'Authenticating and restoring'], verifying: ['正在核对源目录', 'Checking source changes'], checking: ['正在验证加密包', 'Verifying package'], syncing: ['正在写入磁盘', 'Flushing to disk'], committing: ['正在完成任务', 'Finishing task'], completed: ['处理完成', 'Completed'], legacy: ['正在读取 v1 加密包', 'Reading v1 package'],
  cancelling: ['正在取消并清理临时数据…', 'Cancelling and cleaning temporary data…'],
  progress: ['已处理 {done} / {total} · {percent}%', 'Processed {done} / {total} · {percent}%'],
  speed: ['{speed}/s · 预计剩余 {eta}', '{speed}/s · Estimated remaining {eta}'],
  estimating: ['正在估算速度与剩余时间…', 'Estimating speed and time remaining…'],
  seconds: ['{n} 秒', '{n} s'], minutes: ['{n} 分钟', '{n} min'],
  added: ['已加入 {n} 个任务，请检查队列中的输出路径。', 'Added {n} tasks. Review their output paths in the queue.'],
  retryHelp: ['路径已重填，请检查输出目录并重新输入密码。新的输出名称会再次避开已有文件。', 'Paths refilled. Check the output folder and enter the password again. New names will avoid existing files.'],
  paused: ['将在当前任务结束后暂停。等待任务的密码仍暂存内存，可逐项取消以释放。', 'The queue will pause after the current task. Waiting passwords stay in memory; cancel waiting tasks to release them.'],
  ready: ['就绪', 'Ready'], working: ['正在处理，可取消当前任务', 'Working; current task can be cancelled'],
  committingHelp: ['正在完成任务，请稍候…', 'Finishing task; please wait…'],
  keepSource: ['源数据保留 · 输出不覆盖', 'Sources kept · No overwriting'],
  preview: ['界面预览 · 文件操作需桌面程序', 'UI preview · File operations require the desktop app'],
  jobs: ['中断任务临时数据', 'Interrupted task data'], refreshJobs: ['检查中断任务', 'Check interrupted tasks'],
  jobsHelp: ['异常退出后可清理临时数据，再重新输入密码重试。恢复过程中可能留有临时明文；清理保留源文件及最终输出。', 'Clean up interrupted data before retrying. Restores may leave temporary plaintext. Cleanup keeps source data and final outputs.'],
  staging: ['待清理：{path}', 'Temporary data: {path}'], cleanup: ['清理并重填', 'Clean up & refill'],
  cleanupDone: ['临时数据已清理，路径已重填。请重新输入密码并加入队列。', 'Temporary data cleaned and paths refilled. Enter the password and add to the queue.'],
  emptyJobs: ['没有需要清理的中断任务。', 'No interrupted task data to clean up.'],
  attributeNotice: ['文件属性用于减少误操作，不提供密码加密。能修改属性的用户仍可访问内容。', 'File attributes reduce accidental changes. They do not encrypt content; users who can change attributes can still access it.'],
  manageAttributes: ['管理文件属性', 'Manage file attributes'], attributePath: ['文件或文件夹路径', 'File or folder path'],
  pickFile: ['选文件', 'Choose file'], pickFolder: ['选文件夹', 'Choose folder'],
  policy: ['应用策略', 'Attribute policy'], hiddenReadonly: ['隐藏 + 只读', 'Hidden + read-only'], hidden: ['仅隐藏', 'Hidden'], readonly: ['仅只读', 'Read-only'],
  attributeHelp: ['递归处理普通文件和目录；遇到无权限路径或重解析点会停止并显示错误。', 'Processes regular files and folders recursively. Stops on access errors or reparse points.'],
  restoreAttributes: ['恢复原始属性', 'Restore attributes'], applyAttributes: ['应用属性', 'Apply attributes'],
  records: ['管理记录', 'Managed entries'], refresh: ['刷新', 'Refresh'],
  recordHelp: ['点击记录重填路径；记录是恢复信息，当前属性可能已被其他程序修改。', 'Select an entry to refill its path. These are recovery records; other programs may have changed current attributes.'],
  emptyRecords: ['暂无管理记录。', 'No managed entries.'], stateLocation: ['恢复记录：{path}', 'Recovery records: {path}'],
  attributeDone: ['属性操作完成：修改 {changed} 项，缺失 {missing} 项。', 'Attribute operation completed: {changed} changed, {missing} missing.'],
  passwordMismatch: ['两次输入的密码不一致。', 'The passwords do not match.'],
  desktopRequired: ['请通过 FoldSentinel 桌面程序运行文件操作。', 'Use the FoldSentinel desktop app for file operations.'],
  storageFailed: ['无法读取或保存本机偏好 / 历史。请检查 WebView2 用户数据目录；本次操作已停止。', 'Cannot read or save local preferences / history. Check the WebView2 user data folder. This action was stopped.'],
  historyInvalid: ['本机历史记录损坏。可使用“清空历史”移除记录后继续；文件内容不会被删除。', 'Local history is invalid. Use Clear history to remove these records and continue. File contents are kept.'],
  queueLimit: ['等待和运行中的任务最多 100 个；每次选择最多 200 个路径。', 'Up to 100 waiting/running tasks; select at most 200 paths at once.'],
  dialogFailed: ['文件选择或拖放功能失败，请重试或手动输入路径。', 'File selection or drag and drop failed. Retry or enter paths manually.'],
  error_cancelled: ['操作已取消，未提交输出。', 'Cancelled before output was committed.'],
  error_authentication: ['认证失败：密码错误或加密包已损坏。', 'Authentication failed: incorrect password or damaged package.'],
  error_unsupported_format: ['不支持此加密包格式或版本。', 'This package format or version is not supported.'],
  error_invalid_package: ['加密包不完整，或归档结构、路径不合法。', 'The package is incomplete, or its archive structure or paths are invalid.'],
  error_read_failed: ['读取失败，请检查源路径、文件占用和磁盘连接。', 'Cannot read the source. Check its path, open handles and drive connection.'],
  error_write_failed: ['写入或提交失败，请检查目标目录、文件占用和磁盘状态。', 'Cannot write or commit the output. Check the target folder, open handles and drive.'],
  error_output_exists: ['输出位置已存在，未覆盖内容。请重试以选择新名称。', 'The output already exists and was kept. Retry to choose a new name.'],
  error_insufficient_space: ['目标磁盘可用空间不足。请释放空间或选择其他输出目录。', 'Not enough space on the target drive. Free space or choose another output folder.'],
  error_permission_denied: ['无权访问指定位置。请检查源文件和目标目录的访问权限。', 'Access denied. Check permissions for the source and target folder.'],
  error_invalid_input: ['路径、密码或内容不符合要求。请检查输入；不支持链接、特殊文件或源目录内部的输出。', 'Check the paths, password and content. Links, special files and outputs inside source folders are not supported.'],
  error_source_changed: ['源文件在处理期间发生变化，请停止修改后重试。', 'The source changed during processing. Stop editing it and retry.'],
  error_busy: ['已有任务正在运行，请稍后重试。', 'A task is already running. Please try again later.'],
  error_recovery: ['中断记录或临时数据处理失败，请检查访问权限并保留记录以便排查。', 'Cannot process interrupted records or temporary data. Check access permissions and keep the records for diagnosis.'],
  error_internal: ['后台任务或本机状态处理失败。请重新打开程序并检查中断任务。', 'A background task or local state operation failed. Reopen the app and check interrupted tasks.'],
} as const;
export type MessageKey = keyof typeof messages;
export let language: Language = 'en';
export function detectLanguage(saved: string | null, system: readonly string[]): Language {
  if (saved === 'zh-CN' || saved === 'en') return saved;
  return system[0]?.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en';
}
export function setLanguage(value: Language): void { language = value; document.documentElement.lang = value; }
export function t(key: MessageKey, values: Record<string, string | number> = {}): string {
  let message: string = messages[key][language === 'zh-CN' ? 0 : 1];
  for (const [key, value] of Object.entries(values)) message = message.replaceAll(`{${key}}`, String(value));
  return message;
}
export function failureCode(error: unknown): string {
  if (error && typeof error === 'object' && 'code' in error && typeof error.code === 'string' && `error_${error.code}` in messages) return error.code;
  return 'internal';
}
export function errorText(code: string): string { return t((`error_${code}` in messages ? `error_${code}` : 'error_internal') as MessageKey); }
