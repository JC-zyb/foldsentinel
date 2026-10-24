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
New-Item -ItemType Directory -Path $owned | Out-Null
$oldInstaller = Join-Path $owned 'FoldSentinel_0.3.0_x64-setup.exe'
Invoke-WebRequest -Uri 'https://github.com/JC-zyb/foldsentinel/releases/download/v0.3.0/FoldSentinel_0.3.0_x64-setup.exe' -OutFile $oldInstaller
if ((Get-FileHash -LiteralPath $oldInstaller -Algorithm SHA256).Hash.ToLowerInvariant() -ne '75f2397dad0ea0b674f4c0e6ea1dc3704c90c784f3779a7689ca1a4c0a2a3c16') { throw 'v0.3.0 基线安装包 SHA-256 不符' }

function Run-Installer([string]$Path, [string]$Arguments) {
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(180000)) { $process.Kill(); throw '安装器运行超时' }
    if ($process.ExitCode -ne 0) { throw "安装器退出码: $($process.ExitCode)" }
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
        if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
    }
}

# NSIS /D 必须是最后一个参数，整个参数值不加引号：
# https://nsis.sourceforge.io/Docs/Chapter3.html#installerusagecommon
Run-Installer $newInstaller "/S /D=$install"
Start-Smoke $version
Run-Installer (Join-Path $install 'uninstall.exe') "/S _?=$install"
if (Test-Path -LiteralPath (Join-Path $install 'FoldSentinel.exe')) { throw '首次卸载后应用 exe 仍存在' }
Run-Installer $oldInstaller "/S /D=$install"
Start-Smoke '0.3.0'
$markers = @(
    (Join-Path $env:APPDATA 'FoldSentinel\v04-upgrade-sentinel.txt'),
    (Join-Path $env:APPDATA 'FoldSentinel\jobs\v04-upgrade-sentinel.txt'),
    (Join-Path $env:LOCALAPPDATA 'io.github.jc-zyb.foldsentinel\v04-upgrade-sentinel.txt')
)
$token = [Guid]::NewGuid().ToString('N')
foreach ($marker in $markers) {
    if (Test-Path -LiteralPath $marker) { throw '升级哨兵已存在' }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $marker) | Out-Null
    [IO.File]::WriteAllText($marker, $token, [Text.UTF8Encoding]::new($false))
}
Run-Installer $newInstaller "/S /D=$install"
Start-Smoke $version
foreach ($marker in $markers) { if ([IO.File]::ReadAllText($marker) -ne $token) { throw '升级修改了用户数据哨兵' } }
$uninstaller = Join-Path $install 'uninstall.exe'
Run-Installer $uninstaller "/S _?=$install"
if (Test-Path -LiteralPath (Join-Path $install 'FoldSentinel.exe')) { throw '卸载后应用 exe 仍存在' }
foreach ($marker in $markers) { if ([IO.File]::ReadAllText($marker) -ne $token) { throw '默认卸载修改了用户数据哨兵' } }
$result = [ordered]@{
    schema_version = 1; version = $version; baseline = '0.3.0'; passed = $true
    checks = @('baseline_hash', 'install_v04', 'webview_process_v04_fresh', 'uninstall_v04_fresh', 'install_v03', 'webview_process_v03', 'upgrade', 'webview_process_v04', 'user_data_sentinels_retained', 'uninstall', 'default_uninstall_retains_sentinels')
    limitations = @('WebView2 child-process creation is a process smoke check, not interactive UI validation.', 'Sentinels check directory retention, not stored preferences or history semantics.', 'Runner has WebView2; missing-runtime and clean Windows VM checks remain separate.')
}
$json = $result | ConvertTo-Json -Depth 5
$file = [IO.File]::Open($reportPath, [IO.FileMode]::CreateNew)
try { $data = [Text.UTF8Encoding]::new($false).GetBytes($json); $file.Write($data, 0, $data.Length); $file.Flush($true) } finally { $file.Dispose() }
Write-Output "安装、升级及默认卸载验证通过；测试文件保留于: $owned"
