param([string]$Installer, [Parameter(Mandatory = $true)][string]$Report)
$ErrorActionPreference = 'Stop'
# 仅用于一次性 GitHub Windows runner；不会安装到开发者本机。
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) { throw '安装验证仅允许在一次性 GitHub Actions runner 执行' }
$root = Split-Path -Parent $PSScriptRoot
$version = (Get-Content -Raw -LiteralPath (Join-Path $root 'package.json') | ConvertFrom-Json).version
if (-not $Installer) { $Installer = Join-Path $root "artifacts/$version/FoldSentinel_${version}_x64-setup.exe" }
$newInstaller = (Resolve-Path -LiteralPath $Installer).ProviderPath
$reportPath = [IO.Path]::GetFullPath($Report)
if (Test-Path -LiteralPath $reportPath) { throw '报告已存在，拒绝覆盖' }
$owned = Join-Path ([IO.Path]::GetFullPath($env:RUNNER_TEMP)) ('foldsentinel-install-' + [Guid]::NewGuid().ToString('N'))
$install = Join-Path $owned 'app'
$registry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\FoldSentinel'
if (Test-Path -LiteralPath $registry) { throw 'runner 已存在 FoldSentinel 安装，拒绝覆盖' }
$preferences = 'HKCU:\Software\github\FoldSentinel'
if (Test-Path -LiteralPath $preferences) { throw 'runner 已存在产品安装偏好，拒绝覆盖' }
$shortcuts = @(
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'FoldSentinel.lnk'),
    (Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'FoldSentinel.lnk')
)
foreach ($shortcut in $shortcuts) { if (Test-Path -LiteralPath $shortcut) { throw "runner 已存在产品快捷方式，拒绝覆盖：$shortcut" } }
New-Item -ItemType Directory -Path $owned | Out-Null
$oldInstaller = Join-Path $owned 'FoldSentinel_0.4.0_x64-setup.exe'
Invoke-WebRequest -Uri 'https://github.com/JC-zyb/foldsentinel/releases/download/v0.4.0/FoldSentinel_0.4.0_x64-setup.exe' -OutFile $oldInstaller
if ((Get-FileHash -LiteralPath $oldInstaller -Algorithm SHA256).Hash.ToLowerInvariant() -ne '0a8a5f8bc796b0ace914254bb298e645c14b5a71d6e313e80366ff288d15de36') { throw 'v0.4.0 基线安装包 SHA-256 不符' }

function Run-Installer([string]$Path, [string]$Arguments, [int]$ExpectedExit = 0) {
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(180000)) { $process.Kill(); throw '安装器运行超时' }
    if ($process.ExitCode -ne $ExpectedExit) { throw "安装/卸载进程退出码应为 $ExpectedExit，实际为 $($process.ExitCode)" }
}
function Run-Uninstaller([int]$ExpectedExit = 0) {
    # 从测试自有目录运行复制件，_?= 指向安装目录：等待真实卸载进程，
    # 同时允许它删除安装目录内的原始 uninstall.exe。
    $installedUninstaller = Join-Path $install 'uninstall.exe'
    $runner = Join-Path $owned ('uninstall-runner-' + [Guid]::NewGuid().ToString('N') + '.exe')
    Copy-Item -LiteralPath $installedUninstaller -Destination $runner -ErrorAction Stop
    if ((Get-FileHash -LiteralPath $installedUninstaller -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $runner -Algorithm SHA256).Hash) { throw '卸载验证复制件哈希不匹配' }
    Run-Installer $runner "/S _?=$install" $ExpectedExit
}
function Assert-InstalledFootprint([string]$ExpectedVersion) {
    foreach ($name in @('FoldSentinel.exe', 'uninstall.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $install $name) -PathType Leaf)) { throw "安装文件缺失：$name" }
    }
    $registration = Get-ItemProperty -LiteralPath $registry -ErrorAction Stop
    if ($registration.DisplayVersion -ne $ExpectedVersion -or $registration.UninstallString -ne ('"{0}"' -f (Join-Path $install 'uninstall.exe'))) { throw '卸载登记版本或路径错误' }
    $preferenceKey = Get-Item -LiteralPath $preferences -ErrorAction Stop
    if ($preferenceKey.GetValue('') -ne $install) { throw '产品安装位置偏好错误' }
    $shell = New-Object -ComObject WScript.Shell
    try {
        foreach ($shortcut in $shortcuts) {
            if (-not (Test-Path -LiteralPath $shortcut -PathType Leaf)) { throw "安装快捷方式缺失：$shortcut" }
            $target = $shell.CreateShortcut($shortcut).TargetPath
            if (-not [string]::Equals([IO.Path]::GetFullPath($target), [IO.Path]::GetFullPath((Join-Path $install 'FoldSentinel.exe')), [StringComparison]::OrdinalIgnoreCase)) { throw "安装快捷方式目标错误：$shortcut" }
        }
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) }
}
function Assert-UninstalledFootprint {
    if (Test-Path -LiteralPath $install) { throw '正常卸载后程序目录仍存在' }
    if (Test-Path -LiteralPath $registry) { throw '正常卸载后应用与功能登记仍存在' }
    if (Test-Path -LiteralPath $preferences) {
        $preferenceKey = Get-Item -LiteralPath $preferences -ErrorAction Stop
        if ($null -ne $preferenceKey.GetValue('', $null) -or $null -ne $preferenceKey.GetValue('Installer Language', $null)) { throw '正常卸载后已知安装位置或安装器语言值仍存在' }
    }
    foreach ($shortcut in $shortcuts) { if (Test-Path -LiteralPath $shortcut) { throw "正常卸载后快捷方式仍存在：$shortcut" } }
}
function Start-Smoke([string]$ExpectedVersion) {
    $exe = Join-Path $install 'FoldSentinel.exe'
    if ((Get-Item -LiteralPath $exe).VersionInfo.ProductVersion -ne $ExpectedVersion) { throw '已安装 exe 版本错误' }
    $process = Start-Process -FilePath $exe -WindowStyle Hidden -PassThru
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(40)
        $webview = @()
        do {
            Start-Sleep -Milliseconds 250
            $process.Refresh()
            if ($process.HasExited) { throw "首次启动提前退出: $($process.ExitCode)" }
            $webview = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($process.Id)" | Where-Object Name -eq 'msedgewebview2.exe')
        } while ($webview.Count -eq 0 -and [DateTime]::UtcNow -lt $deadline)
        if ($webview.Count -eq 0) { throw '未观察到应用创建的 WebView2 子进程' }
        Start-Sleep -Seconds 2
        if ($process.HasExited) { throw 'WebView2 创建后应用退出' }
    } finally {
        # 无文件任务的自有冒烟进程；只结束测试进程，不操作窗口控件。
        if (-not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
    }
}

# NSIS /D 必须是最后一个参数，整个参数值不加引号：
# https://nsis.sourceforge.io/Docs/Chapter3.html#installerusagecommon
Run-Installer $newInstaller "/S /D=$install"
Assert-InstalledFootprint $version
Start-Smoke $version
Run-Uninstaller
Assert-UninstalledFootprint
Run-Installer $oldInstaller "/S /D=$install"
Start-Smoke '0.4.0'
$markers = @(
    (Join-Path $env:APPDATA 'FoldSentinel\v08-upgrade-sentinel.txt'),
    (Join-Path $env:APPDATA 'FoldSentinel\jobs\v08-upgrade-sentinel.txt'),
    (Join-Path $env:LOCALAPPDATA 'io.github.jc-zyb.foldsentinel\v08-upgrade-sentinel.txt')
)
$token = [Guid]::NewGuid().ToString('N')
foreach ($marker in $markers) {
    if (Test-Path -LiteralPath $marker) { throw '升级哨兵已存在' }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $marker) | Out-Null
    [IO.File]::WriteAllText($marker, $token, [Text.UTF8Encoding]::new($false))
}
Run-Installer $newInstaller "/S /D=$install"
Assert-InstalledFootprint $version
Start-Smoke $version
foreach ($marker in $markers) { if ([IO.File]::ReadAllText($marker) -ne $token) { throw '升级修改了用户数据哨兵' } }
# 未知应用数据须在删除程序前阻止正常静默卸载，不能靠保留目录伪装完整成功。
Run-Uninstaller 20
Assert-InstalledFootprint $version
foreach ($marker in $markers) {
    if ([IO.File]::ReadAllText($marker) -ne $token) { throw '被阻止的卸载修改了用户数据哨兵' }
}
# 只移除本次测试创建且内容精确匹配的哨兵；不清理任何其他用户状态。
foreach ($marker in $markers) {
    if ([IO.File]::ReadAllText($marker) -ne $token) { throw '哨兵内容变化，拒绝测试清理' }
    Remove-Item -LiteralPath $marker -ErrorAction Stop
}
Run-Uninstaller
Assert-UninstalledFootprint
$result = [ordered]@{
    schema_version = 2; version = $version; baseline = '0.4.0'; passed = $true
    checks = @('baseline_hash', 'install_current', 'uninstall_registration_and_shortcuts', 'webview_process_current_fresh', 'uninstall_current_fresh', 'install_baseline', 'webview_process_baseline', 'upgrade', 'webview_process_current', 'upgrade_sentinels_retained', 'silent_uninstall_rejects_unknown_metadata_before_removal', 'blocked_uninstall_retains_registration_shortcuts_and_sentinels', 'normal_uninstall_removes_program_directory_registration_shortcuts_and_known_preferences')
    limitations = @('WebView2 child-process creation is a process smoke check, not interactive UI validation.', 'Sentinels check upgrade retention and unknown-metadata gating, not stored preferences or history semantics.', 'This disposable runner does not install the optional SYSTEM service; real service removal requires separate acceptance.', 'Runner has WebView2; missing-runtime and clean Windows VM checks remain separate.')
}
$json = $result | ConvertTo-Json -Depth 5
$file = [IO.File]::Open($reportPath, [IO.FileMode]::CreateNew)
try { $data = [Text.UTF8Encoding]::new($false).GetBytes($json); $file.Write($data, 0, $data.Length); $file.Flush($true) } finally { $file.Dispose() }
Write-Output "安装、升级及默认卸载验证通过；测试文件保留于: $owned"
