param([string]$Makensis, [Parameter(Mandatory = $true)][string]$Report)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$reportPath = [IO.Path]::GetFullPath($Report)
if (Test-Path -LiteralPath $reportPath) { throw "报告已存在，拒绝覆盖：$reportPath" }
if (-not $Makensis) { $Makensis = Join-Path $env:LOCALAPPDATA 'tauri/NSIS/Bin/makensis.exe' }
$compiler = (Resolve-Path -LiteralPath $Makensis).Path
$csc = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
if (-not (Test-Path -LiteralPath $csc)) { throw '缺少 Windows 自带的 C# 编译器' }
$owned = Join-Path ([IO.Path]::GetTempPath()) ('foldsentinel-install-hook-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $owned | Out-Null

# 仅在自有目录运行夹具 GUI，记录固定参数与返回码；不安装产品或操作真实服务。
$helperSource = @'
using System;
using System.IO;
internal static class Fixture {
  static int Main(string[] args) {
    string directory = Environment.GetEnvironmentVariable("FOLDSENTINEL_INSTALL_FIXTURE_DIRECTORY");
    if (String.IsNullOrEmpty(directory) || !Directory.Exists(directory)) return 90;
    if (args.Length != 1 || args[0] != "--install-service") return 91;
    int code;
    if (!Int32.TryParse(Environment.GetEnvironmentVariable("FOLDSENTINEL_INSTALL_FIXTURE_EXIT"), out code)) return 92;
    File.AppendAllText(Path.Combine(directory, "calls.log"), args[0] + "|" + code + Environment.NewLine);
    return code;
  }
}
'@
$helperCs = Join-Path $owned 'Fixture.cs'
$helperExe = Join-Path $owned 'FoldSentinel.exe'
[IO.File]::WriteAllText($helperCs, $helperSource, [Text.UTF8Encoding]::new($false))
& $csc /nologo /target:winexe "/out:$helperExe" $helperCs
if ($LASTEXITCODE -ne 0) { throw '安装夹具编译失败' }
$nsis = @'
Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "@@HOOK@@"
!define MAINBINARYNAME "FoldSentinel"
Name "FoldSentinel isolated service install fixture"
OutFile "@@INSTALLER@@"
InstallDir "@@OWNED@@\unconfigured"
RequestExecutionLevel user
SilentInstall silent
AutoCloseWindow true
ShowInstDetails nevershow
Var PassiveMode
Var FixtureMode
Function .onInit
  ReadEnvStr $0 FOLDSENTINEL_INSTALL_FIXTURE_DIRECTORY
  StrCmp $0 $INSTDIR 0 invalid
  StrCmp $0 "" invalid
  StrCpy $1 $0 @@PREFIXLENGTH@@
  StrCmp $1 "@@OWNED@@\" 0 invalid
  ReadEnvStr $FixtureMode FOLDSENTINEL_INSTALL_FIXTURE_MODE
  StrCpy $FoldSentinelInstallServiceSelected 1
  StrCpy $FoldSentinelInstallServiceResult 0
  StrCpy $PassiveMode 0
  ${If} $FixtureMode == "passive"
    StrCpy $PassiveMode 1
    SetSilent normal
  ${EndIf}
  Return
invalid:
  SetErrorLevel 93
  Quit
FunctionEnd
Section
  SetOutPath "$INSTDIR"
  File "@@HELPER@@"
  ReadEnvStr $0 FOLDSENTINEL_INSTALL_FIXTURE_MISSING
  ${If} $0 != "payload"
    FileOpen $1 "$INSTDIR\foldsentinel-service.exe" w
    FileWrite $1 "owned metadata-only service payload fixture"
    FileClose $1
  ${EndIf}
  ${If} $0 == "gui"
    Delete "$INSTDIR\FoldSentinel.exe"
  ${EndIf}
  ${If} $FixtureMode == "run"
    !insertmacro FoldSentinelRunServiceInstall
  ${Else}
    !insertmacro NSIS_HOOK_POSTINSTALL
  ${EndIf}
  SetErrorLevel $FoldSentinelInstallServiceResult
SectionEnd
'@
$installer = Join-Path $owned 'fixture-setup.exe'
$hook = Join-Path $root 'src-tauri/windows/hooks.nsh'
$nsis = $nsis.Replace('@@HOOK@@', $hook).Replace('@@INSTALLER@@', $installer).Replace('@@HELPER@@', $helperExe).Replace('@@PREFIXLENGTH@@', [string]($owned.Length + 1)).Replace('@@OWNED@@', $owned)
$nsisPath = Join-Path $owned 'fixture.nsi'
[IO.File]::WriteAllText($nsisPath, $nsis, [Text.UTF8Encoding]::new($false))
& $compiler /INPUTCHARSET UTF8 /V2 $nsisPath
if ($LASTEXITCODE -ne 0) { throw 'NSIS 安装夹具编译失败' }
$cases = @(
    @{ Name = 'success'; Mode = 'run'; Result = 0; Exit = 0; Calls = 1 },
    @{ Name = 'uac-canceled'; Mode = 'run'; Result = 1223; Exit = 1223; Calls = 1 },
    @{ Name = 'existing-conflict'; Mode = 'run'; Result = 20; Exit = 20; Calls = 1 },
    @{ Name = 'setup-failed'; Mode = 'run'; Result = 21; Exit = 21; Calls = 1 },
    @{ Name = 'missing-payload'; Mode = 'run'; Missing = 'payload'; Result = 0; Exit = 21; Calls = 0 },
    @{ Name = 'missing-gui'; Mode = 'run'; Missing = 'gui'; Result = 0; Exit = 21; Calls = 0 },
    @{ Name = 'silent-never-elevates'; Mode = 'silent'; Result = 0; Exit = 0; Calls = 0 },
    @{ Name = 'passive-never-elevates'; Mode = 'passive'; Result = 0; Exit = 0; Calls = 0 }
)
$results = @()
foreach ($case in $cases) {
    $directory = Join-Path $owned $case.Name
    New-Item -ItemType Directory -Path $directory | Out-Null
    $variables = @{
        FOLDSENTINEL_INSTALL_FIXTURE_DIRECTORY = $directory
        FOLDSENTINEL_INSTALL_FIXTURE_EXIT = [string]$case.Result
        FOLDSENTINEL_INSTALL_FIXTURE_MODE = $case.Mode
        FOLDSENTINEL_INSTALL_FIXTURE_MISSING = [string]$case.Missing
    }
    $previous = @{}
    foreach ($key in $variables.Keys) {
        $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $variables[$key], 'Process')
    }
    try {
        $options = if ($case.Mode -eq 'passive') { "/P /D=$directory" } else { "/S /D=$directory" }
        $process = Start-Process -FilePath $installer -ArgumentList $options -WindowStyle Hidden -PassThru
    } finally {
        foreach ($key in $variables.Keys) { [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process') }
    }
    if (-not $process.WaitForExit(30000)) { $process.Kill(); throw "$($case.Name)：安装夹具超时" }
    if ($process.ExitCode -ne $case.Exit) { throw "$($case.Name)：预期退出码 $($case.Exit)，实际 $($process.ExitCode)" }
    $log = Join-Path $directory 'calls.log'
    $calls = @(if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log })
    if ($calls.Count -ne $case.Calls) { throw "$($case.Name)：helper 调用次数异常" }
    if ($calls.Count -eq 1 -and $calls[0] -ne "--install-service|$($case.Result)") { throw "$($case.Name)：安装固定参数或结果异常" }
    $results += [ordered]@{ name = $case.Name; exit_code = $process.ExitCode; helper_calls = $calls.Count }
}
$parent = Split-Path -Parent $reportPath
if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent | Out-Null }
$reportData = [ordered]@{
    schema_version = 1; passed = $true; tested_at = [DateTime]::UtcNow.ToString('o'); checks = $results
    limitations = @('Only private fixture files and a metadata helper are used; no product or SYSTEM service is installed.', 'Exit-code fixtures do not replace real UAC, service installation, or interactive page acceptance.')
} | ConvertTo-Json -Depth 6
$stream = [IO.File]::Open($reportPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
try { $bytes = [Text.UTF8Encoding]::new($false).GetBytes($reportData); $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
Write-Output "隔离 NSIS 服务安装 hooks 验证通过：$($results.Count) 项；报告：$reportPath"
