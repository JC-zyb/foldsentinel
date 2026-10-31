param(
    [string]$Makensis,
    [Parameter(Mandatory = $true)][string]$Report
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$reportPath = [IO.Path]::GetFullPath($Report)
if (Test-Path -LiteralPath $reportPath) { throw "报告已存在，拒绝覆盖：$reportPath" }
if (-not $Makensis) {
    $Makensis = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\Bin\makensis.exe'
}
$compiler = (Resolve-Path -LiteralPath $Makensis).Path
$csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
if (-not (Test-Path -LiteralPath $csc)) { throw '缺少 Windows 自带的 .NET Framework C# 编译器' }
$hook = (Resolve-Path -LiteralPath (Join-Path $root 'src-tauri\windows\hooks.nsh')).Path
$owned = Join-Path ([IO.Path]::GetFullPath([IO.Path]::GetTempPath())) ('foldsentinel-hook-fixture-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $owned | Out-Null
$registryRoot = 'Software\FoldSentinelHookFixture-' + [Guid]::NewGuid().ToString('N')
if ($null -ne [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($registryRoot)) { throw '私有注册表夹具已存在，拒绝覆盖' }

# 此夹具只使用新建私有目录和唯一 HKCU 命名空间，不安装产品或操作服务。
$fixtureSource = @'
using System;
using System.Diagnostics;
using System.IO;

internal static class FixtureHelper
{
    private static int Main(string[] args)
    {
        string directory = Environment.GetEnvironmentVariable("FOLDSENTINEL_HOOK_FIXTURE_CASE");
        if (String.IsNullOrEmpty(directory) || !Directory.Exists(directory)) return 90;
        string phase = null;
        int parent = 0;
        bool silent = false;
        for (int i = 0; i < args.Length; i++)
        {
            if (args[i] == "--uninstall-prepare") phase = "prepare";
            else if (args[i] == "--uninstall-finish") phase = "finish";
            else if (args[i] == "--silent") silent = true;
            else if (args[i] == "--parent-pid" && i + 1 < args.Length)
            {
                if (!Int32.TryParse(args[++i], out parent)) return 91;
            }
            else return 92;
        }
        if (phase == null || parent <= 0) return 93;
        try { if (Process.GetProcessById(parent).HasExited) return 94; }
        catch (ArgumentException) { return 94; }
        int result;
        string key = phase == "prepare" ? "FOLDSENTINEL_HOOK_PREPARE_EXIT" : "FOLDSENTINEL_HOOK_FINISH_EXIT";
        if (!Int32.TryParse(Environment.GetEnvironmentVariable(key), out result)) return 95;
        string record = String.Join("|", phase, parent, silent ? "silent" : "interactive", result,
            File.Exists(Path.Combine(directory, "installed-marker")) ? "before-delete" : "after-delete",
            Path.GetFileName(Process.GetCurrentProcess().MainModule.FileName));
        File.AppendAllText(Path.Combine(directory, "helper.log"), record + Environment.NewLine);
        return result;
    }
}
'@
$fixtureCs = Join-Path $owned 'FixtureHelper.cs'
$fixtureExe = Join-Path $owned 'FoldSentinel.exe'
[IO.File]::WriteAllText($fixtureCs, $fixtureSource, [Text.UTF8Encoding]::new($false))
& $csc /nologo /target:winexe "/out:$fixtureExe" $fixtureCs
if ($LASTEXITCODE -ne 0) { throw "夹具 helper 编译失败：$LASTEXITCODE" }

$fixtureNsis = @'
Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"
; 匹配 Tauri 顺序：hooks 先于产品常量和变量定义载入。
!include "@@HOOK@@"
!define MAINBINARYNAME "FoldSentinel"
!define PRODUCTNAME "FoldSentinel"
!define ARCH "x64"
!define INSTALLMODE "currentUser"
!define MANUKEY "@@REGISTRYROOT@@"
!define MANUPRODUCTKEY "$FixtureRegistryKey"
!define UNINSTKEY "$FixtureRegistryKey\Uninstall"
!define FOLDSENTINEL_UNINSTALL_RUN_KEY "$FixtureRegistryKey\Run"
!define FOLDSENTINEL_UNINSTALL_DESKTOP_SHORTCUT "$INSTDIR\desktop.lnk"
!define FOLDSENTINEL_UNINSTALL_STARTMENU_SHORTCUT "$INSTDIR\startmenu.lnk"
!define FOLDSENTINEL_UNINSTALL_STARTMENU_FOLDER_SHORTCUT "$INSTDIR\folder.lnk"
!define FIXTURE_REGISTRY_PREFIX "${MANUKEY}\"
!define FIXTURE_REGISTRY_PREFIX_LENGTH @@REGISTRYPREFIXLENGTH@@
Name "FoldSentinel isolated uninstall hook fixture"
OutFile "@@INSTALLER@@"
InstallDir "@@OWNED@@\unconfigured"
RequestExecutionLevel user
SilentInstall silent
AutoCloseWindow true
ShowInstDetails nevershow
ShowUninstDetails nevershow
Var PassiveMode
Var UpdateMode
Var DeleteAppDataCheckboxState
Var FixtureRegistryKey

Function .onInit
  SetShellVarContext current
  SetRegView 64
  ReadEnvStr $FixtureRegistryKey FOLDSENTINEL_HOOK_FIXTURE_REGISTRY_KEY
  StrCpy $0 $FixtureRegistryKey ${FIXTURE_REGISTRY_PREFIX_LENGTH}
  StrCmp $0 "${FIXTURE_REGISTRY_PREFIX}" 0 invalid_target
  ReadEnvStr $0 FOLDSENTINEL_HOOK_FIXTURE_CASE
  StrCmp $INSTDIR $0 0 invalid_target
  StrCmp $0 "" invalid_target
  Return
invalid_target:
  SetErrorLevel 96
  Quit
FunctionEnd

Function un.onInit
  SetShellVarContext current
  SetRegView 64
  ReadEnvStr $FixtureRegistryKey FOLDSENTINEL_HOOK_FIXTURE_REGISTRY_KEY
  StrCpy $0 $FixtureRegistryKey ${FIXTURE_REGISTRY_PREFIX_LENGTH}
  StrCmp $0 "${FIXTURE_REGISTRY_PREFIX}" 0 invalid_uninstall_target
  ReadEnvStr $0 FOLDSENTINEL_HOOK_FIXTURE_CASE
  StrCmp $INSTDIR $0 0 invalid_uninstall_target
  StrCmp $0 "" invalid_uninstall_target
  StrCpy $PassiveMode 0
  StrCpy $UpdateMode 0
  StrCpy $DeleteAppDataCheckboxState 1
  SetAutoClose true
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/P" $1
  IfErrors +2
  StrCpy $PassiveMode 1
  ClearErrors
  ${GetOptions} $0 "/UPDATE" $1
  IfErrors +2
  StrCpy $UpdateMode 1
  Return
invalid_uninstall_target:
  SetErrorLevel 97
  Quit
FunctionEnd

Section
  SetOutPath "$INSTDIR"
  File "@@HELPER@@"
  FileOpen $0 "$INSTDIR\installed-marker" w
  FileWrite $0 "owned fixture file"
  FileClose $0
  WriteRegStr HKCU "$FixtureRegistryKey" "" "$INSTDIR"
  WriteRegStr HKCU "$FixtureRegistryKey" "Installer Language" "2052"
  WriteRegStr HKCU "$FixtureRegistryKey" "UnknownFixtureValue" "preserve-unknown"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "FoldSentinel fixture"
  WriteRegStr HKCU "${FOLDSENTINEL_UNINSTALL_RUN_KEY}" "${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe"
  WriteRegStr HKCU "${FOLDSENTINEL_UNINSTALL_RUN_KEY}" "UnknownFixtureValue" "preserve-unknown-run"
  CreateShortcut "${FOLDSENTINEL_UNINSTALL_DESKTOP_SHORTCUT}" "$INSTDIR\${MAINBINARYNAME}.exe"
  CreateShortcut "${FOLDSENTINEL_UNINSTALL_STARTMENU_SHORTCUT}" "$INSTDIR\${MAINBINARYNAME}.exe"
  CreateShortcut "${FOLDSENTINEL_UNINSTALL_STARTMENU_FOLDER_SHORTCUT}" "$INSTDIR\${MAINBINARYNAME}.exe"
  ReadEnvStr $0 FOLDSENTINEL_HOOK_UNKNOWN_SHORTCUT
  ${If} $0 == 1
    CreateShortcut "${FOLDSENTINEL_UNINSTALL_DESKTOP_SHORTCUT}" "$INSTDIR\user-program.exe"
  ${EndIf}
  FileOpen $0 "$INSTDIR\unknown-user-file" w
  FileWrite $0 "preserve user data"
  FileClose $0
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  !insertmacro NSIS_HOOK_PREUNINSTALL
  FileOpen $0 "$INSTDIR\deletion.log" w
  FileWrite $0 "$DeleteAppDataCheckboxState|$UpdateMode"
  FileClose $0
  Delete "$INSTDIR\installed-marker"
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"
  Delete "$INSTDIR\uninstall.exe"
  ReadEnvStr $0 FOLDSENTINEL_HOOK_UNKNOWN_SHORTCUT
  ${If} $0 != 1
    Delete "${FOLDSENTINEL_UNINSTALL_DESKTOP_SHORTCUT}"
  ${EndIf}
  Delete "${FOLDSENTINEL_UNINSTALL_STARTMENU_SHORTCUT}"
  Delete "${FOLDSENTINEL_UNINSTALL_STARTMENU_FOLDER_SHORTCUT}"
  ReadEnvStr $0 FOLDSENTINEL_HOOK_KEEP_REGISTRATION
  ${If} $0 != 1
    DeleteRegKey HKCU "${UNINSTKEY}"
  ${EndIf}
  ReadEnvStr $0 FOLDSENTINEL_HOOK_KEEP_RUN
  ${If} $0 != 1
    DeleteRegValue HKCU "${FOLDSENTINEL_UNINSTALL_RUN_KEY}" "${PRODUCTNAME}"
  ${EndIf}
  !insertmacro NSIS_HOOK_POSTUNINSTALL
  SetErrorLevel 0
SectionEnd
'@
$installer = Join-Path $owned 'hook-fixture-setup.exe'
$fixtureNsis = $fixtureNsis.Replace('@@HOOK@@', $hook).Replace('@@INSTALLER@@', $installer).Replace('@@OWNED@@', $owned).Replace('@@HELPER@@', $fixtureExe).Replace('@@REGISTRYROOT@@', $registryRoot).Replace('@@REGISTRYPREFIXLENGTH@@', [string]($registryRoot.Length + 1))
$fixtureNsiPath = Join-Path $owned 'hook-fixture.nsi'
[IO.File]::WriteAllText($fixtureNsiPath, $fixtureNsis, [Text.UTF8Encoding]::new($false))
& $compiler /INPUTCHARSET UTF8 /V2 $fixtureNsiPath
if ($LASTEXITCODE -ne 0) { throw "NSIS hooks 夹具编译失败：$LASTEXITCODE" }

function Run-FixtureProcess([string]$Path, [string]$Arguments, [hashtable]$Variables) {
    $oldVariables = @{}
    foreach ($name in $Variables.Keys) {
        $oldVariables[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, [string]$Variables[$name], 'Process')
    }
    try {
        $process = Start-Process -FilePath $Path -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    } finally {
        foreach ($name in $Variables.Keys) {
            [Environment]::SetEnvironmentVariable($name, $oldVariables[$name], 'Process')
        }
    }
    if (-not $process.WaitForExit(30000)) {
        $process.Kill()
        throw "夹具进程超时：$Path $Arguments"
    }
    return $process.ExitCode
}

$cases = @(
    @{ Name = 'normal'; Prepare = 0; Finish = 0; Options = ''; Exit = 0; Delete = $true; Silent = $false },
    @{ Name = 'force'; Prepare = 10; Finish = 0; Options = ''; Exit = 0; Delete = $true; Silent = $false },
    @{ Name = 'blocked'; Prepare = 20; Finish = 0; Options = ''; Exit = 20; Delete = $false; Silent = $false },
    @{ Name = 'unknown'; Prepare = 77; Finish = 0; Options = ''; Exit = 77; Delete = $false; Silent = $false },
    @{ Name = 'silent-normal'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 0; Delete = $true; Silent = $true },
    @{ Name = 'silent-force-rejected'; Prepare = 10; Finish = 0; Options = '/S'; Exit = 20; Delete = $false; Silent = $true },
    @{ Name = 'silent-blocked'; Prepare = 20; Finish = 0; Options = '/S'; Exit = 20; Delete = $false; Silent = $true },
    @{ Name = 'passive-force-rejected'; Prepare = 10; Finish = 0; Options = '/P'; Exit = 20; Delete = $false; Silent = $true },
    @{ Name = 'update-blocked'; Prepare = 20; Finish = 0; Options = '/S /UPDATE'; Exit = 20; Delete = $false; Silent = $true },
    @{ Name = 'update-normal'; Prepare = 0; Finish = 0; Options = '/S /UPDATE'; Exit = 0; Delete = $true; Silent = $true },
    @{ Name = 'finish-failed'; Prepare = 0; Finish = 31; Options = '/S'; Exit = 31; Delete = $true; Silent = $true },
    @{ Name = 'missing-helper'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 21; Delete = $false; Silent = $true; Missing = $true },
    @{ Name = 'main-exe-delete-denied'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 25; Delete = $true; Silent = $true; Lock = 'FoldSentinel.exe'; StopBeforeFinish = $true },
    @{ Name = 'uninstaller-delete-denied'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 25; Delete = $true; Silent = $true; Lock = 'uninstall.exe'; StopBeforeFinish = $true },
    @{ Name = 'shortcut-delete-denied'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 27; Delete = $true; Silent = $true; Lock = 'desktop.lnk'; StopBeforeFinish = $true },
    @{ Name = 'uninstall-registration-remains'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 26; Delete = $true; Silent = $true; KeepRegistration = '1'; StopBeforeFinish = $true },
    @{ Name = 'autostart-registration-remains'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 26; Delete = $true; Silent = $true; KeepRun = '1'; StopBeforeFinish = $true },
    @{ Name = 'unknown-shortcut-retained'; Prepare = 0; Finish = 0; Options = '/S'; Exit = 0; Delete = $true; Silent = $true; UnknownShortcut = '1' }
)
$results = @()
foreach ($case in $cases) {
    $caseDirectory = Join-Path $owned $case['Name']
    $variables = @{
        FOLDSENTINEL_HOOK_FIXTURE_CASE = $caseDirectory
        FOLDSENTINEL_HOOK_PREPARE_EXIT = [string]$case['Prepare']
        FOLDSENTINEL_HOOK_FINISH_EXIT = [string]$case['Finish']
        FOLDSENTINEL_HOOK_FIXTURE_REGISTRY_KEY = $registryRoot + '\' + $case['Name']
        FOLDSENTINEL_HOOK_KEEP_REGISTRATION = [string]$case['KeepRegistration']
        FOLDSENTINEL_HOOK_KEEP_RUN = [string]$case['KeepRun']
        FOLDSENTINEL_HOOK_UNKNOWN_SHORTCUT = [string]$case['UnknownShortcut']
    }
    $installExit = Run-FixtureProcess $installer "/S /D=$caseDirectory" $variables
    if ($installExit -ne 0) { throw "$($case['Name'])：夹具准备失败，退出码 $installExit" }
    if ($case['Missing']) { Remove-Item -LiteralPath (Join-Path $caseDirectory 'FoldSentinel.exe') }
    # 从同一私有根中的独立路径运行，既等待真正卸载进程，又允许删除安装目录卸载器。
    $uninstaller = Join-Path $owned ($case['Name'] + '-uninstall.exe')
    Copy-Item -LiteralPath (Join-Path $caseDirectory 'uninstall.exe') -Destination $uninstaller
    $deleteLock = $null
    try {
        if ($case['Lock']) {
            $lockedPath = Join-Path $caseDirectory $case['Lock']
            # 允许读取/写入以便 PRE 复制 helper，但真实拒绝 DELETE 共享。
            $deleteLock = [IO.File]::Open($lockedPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
        }
        # _?= 必须最后且不加引号；目标已由 .onInit/un.onInit 核对私有路径。
        $exitCode = Run-FixtureProcess $uninstaller "$($case['Options']) _?=$caseDirectory" $variables
        if ($case['Lock'] -and -not (Test-Path -LiteralPath $lockedPath)) { throw "$($case['Name'])：未真实保留被禁止删除共享的文件" }
    } finally {
        if ($null -ne $deleteLock) { $deleteLock.Dispose() }
    }
    if ($exitCode -ne $case['Exit']) { throw "$($case['Name'])：实际卸载进程退出码应为 $($case['Exit'])，实际为 $exitCode" }
    $marker = Join-Path $caseDirectory 'installed-marker'
    $expectedMarker = -not [bool]$case['Delete']
    $settleDeadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        if ((Test-Path -LiteralPath $marker) -eq $expectedMarker) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $settleDeadline)
    # 同时核对真实退出码与删除状态；拒绝删除不能由退出码0假装成功。
    $deleted = -not (Test-Path -LiteralPath (Join-Path $caseDirectory 'installed-marker'))
    if ([bool]$deleted -ne [bool]$case['Delete']) { throw "$($case['Name'])：删除前拦截结果错误" }
    $logPath = Join-Path $caseDirectory 'helper.log'
    $expectedFinish = $case['Delete'] -and -not $case['StopBeforeFinish']
    $expectedCalls = if ($case['Missing']) { 0 } elseif ($expectedFinish) { 2 } else { 1 }
    $logDeadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $records = if (Test-Path -LiteralPath $logPath) { @(Get-Content -LiteralPath $logPath | ForEach-Object { [string]$_ }) } else { @() }
        if ($records.Count -ge $expectedCalls) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $logDeadline)
    if ($case['Missing']) {
        if ($records.Count -ne 0) { throw "$($case['Name'])：缺少 helper 时不应执行预检" }
    } else {
        if ($records.Count -ne $expectedCalls) { throw "$($case['Name'])：helper 调用次数错误" }
        $prepare = @(Get-Content -LiteralPath $logPath)[0] -split '\|'
        if ([int]$prepare[3] -ne [int]$case['Prepare']) { throw "$($case['Name'])：PRE helper 返回码记录错误" }
        $mode = if ($case['Silent']) { 'silent' } else { 'interactive' }
        if ($prepare[0] -ne 'prepare' -or $prepare[2] -ne $mode -or $prepare[4] -ne 'before-delete' -or $prepare[5] -ne 'FoldSentinel-uninstall-helper.exe') {
            throw "$($case['Name'])：PRE 调用路径、模式或删除时序错误：$($records[0])"
        }
        if ($expectedFinish) {
            $finish = @(Get-Content -LiteralPath $logPath)[1] -split '\|'
            if ([int]$finish[3] -ne [int]$case['Finish']) { throw "$($case['Name'])：POST helper 返回码记录错误" }
            if ($finish[0] -ne 'finish' -or $finish[1] -ne $prepare[1] -or $finish[4] -ne 'after-delete' -or $finish[5] -ne $prepare[5]) {
                throw "$($case['Name'])：POST 调用路径、父进程或删除时序错误：$($records[1])"
            }
            if ($finish[2] -ne $mode) { throw "$($case['Name'])：POST 静默参数与 PRE 不一致" }
        }
    }
    if ($case['Delete'] -and [IO.File]::ReadAllText((Join-Path $caseDirectory 'deletion.log')) -ne '0|0') {
        throw "$($case['Name'])：Tauri 通用应用数据删除未被禁用，或 /UPDATE 保留了实际卸载痕迹"
    }
    if ([IO.File]::ReadAllText((Join-Path $caseDirectory 'unknown-user-file')) -ne 'preserve user data') { throw "$($case['Name'])：安装目录未知用户文件被修改或删除" }
    if ($case['UnknownShortcut'] -and -not (Test-Path -LiteralPath (Join-Path $caseDirectory 'desktop.lnk'))) { throw "$($case['Name'])：其他程序的快捷方式被删除" }
    $caseRegistry = $registryRoot + '\' + $case['Name']
    $registry = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($caseRegistry, $true)
    if ($null -eq $registry) { throw "$($case['Name'])：未知注册表值的键被删除" }
    try {
        if ($registry.GetValue('UnknownFixtureValue') -ne 'preserve-unknown') { throw "$($case['Name'])：未知注册表值被修改或删除" }
        $expectedClean = $expectedFinish -and $case['Prepare'] -eq 0 -and $case['Finish'] -eq 0
        if ($expectedClean) {
            if ($null -ne $registry.GetValue('', $null) -or $null -ne $registry.GetValue('Installer Language', $null)) { throw "$($case['Name'])：正常卸载未清除固定安装位置/语言值" }
        } else {
            if ($registry.GetValue('') -ne $caseDirectory -or $registry.GetValue('Installer Language') -ne '2052') { throw "$($case['Name'])：强制/失败路径错误删除了产品偏好值" }
        }
        foreach ($name in @('', 'Installer Language', 'UnknownFixtureValue')) { $registry.DeleteValue($name, $false) }
    } finally { $registry.Dispose() }
    foreach ($child in @('Uninstall', 'Run')) {
        $childKey = $caseRegistry + '\' + $child
        $registry = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($childKey, $true)
        if ($null -ne $registry) {
            try {
                if ($child -eq 'Run' -and $registry.GetValue('UnknownFixtureValue') -ne 'preserve-unknown-run') { throw "$($case['Name'])：未知自启动值被删除" }
                foreach ($name in @('DisplayName', 'FoldSentinel', 'UnknownFixtureValue')) { $registry.DeleteValue($name, $false) }
            } finally { $registry.Dispose() }
            [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey($childKey, $false)
        }
    }
    # 只删除本次创建的已清空子键，不递归删除注册表。
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey($caseRegistry, $false)
    $results += [ordered]@{ name = $case['Name']; exit_code = $exitCode; removed_fixture_file = $deleted; helper_calls = $records.Count; known_preferences_removed = [bool]$expectedClean; unknown_preference_retained = $true; deletion_denied_by_real_handle = [bool]$case['Lock']; stopped_before_finish = [bool]$case['StopBeforeFinish']; unknown_user_file_retained = $true }
}
[Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey($registryRoot, $false)

$parent = Split-Path -Parent $reportPath
if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent | Out-Null }
$reportData = [ordered]@{
    schema = 2
    validation = 'isolated-nsis-uninstall-hooks'
    passed = $true
    owned_directory = $owned
    product_registry_or_service_changes = $false
    private_registry_fixture = $true
    direct_uninstaller_process = $true
    limitations = @('The fixture helper supplies protocol outcomes; this validates NSIS branching and process/file order, not the product preflight or service removal.')
    checks = $results
}
$stream = [IO.File]::Open($reportPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
try {
    $writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false))
    $writer.Write(($reportData | ConvertTo-Json -Depth 5))
    $writer.Flush()
} finally {
    $stream.Dispose()
}
Write-Host "隔离 NSIS hooks 验证通过：$($results.Count) 项；报告：$reportPath"
