param()
$ErrorActionPreference = 'Stop'
# 仅加载被测函数的AST，不执行管理脚本的顶层逻辑，不连接或修改SCM。
$scriptPath = Join-Path $PSScriptRoot 'manage-protection-service.ps1'
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$null, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw ($parseErrors.Message -join '; ') }
$functionNames = @(
    'Get-ProtectionService', 'Assert-ServiceStopped', 'Assert-ServiceBinaryRegistration',
    'Get-OwnedServiceProgramFiles', 'Assert-OwnedServiceProgramFiles', 'Remove-OwnedServiceProgram',
    'Confirm-ForcedUninstall', 'Wait-ProtectionServiceStopped', 'Wait-ProtectionServiceAbsent'
)
foreach ($name in $functionNames) {
    $definition = @($ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $false))
    if ($definition.Count -ne 1) { throw "被测函数缺失或重复：$name" }
    . ([scriptblock]::Create($definition[0].Extent.Text))
}
function Assert-True([bool]$Value, [string]$Message) { if (-not $Value) { throw $Message } }
function Assert-Throws([scriptblock]$Work, [string]$Message) {
    $failed = $false
    try { & $Work } catch { $failed = $true }
    Assert-True $failed $Message
}
function Get-CimInstance { throw 'SCM/CIM 未配置测试替身；隔离验证禁止访问真实服务。' }
function Stop-Service { throw '隔离验证禁止停止真实服务。' }
function Invoke-ServiceCommand { throw '隔离验证禁止修改真实服务。' }

$owned = Join-Path ([IO.Path]::GetTempPath()) ('foldsentinel-uninstall-test-' + [Guid]::NewGuid().ToString('N'))
$junction = $null
New-Item -ItemType Directory -Path $owned | Out-Null
$checks = @()
try {
    $normal = [pscustomobject]@{ State = 'Stopped'; ExitCode = 0; ServiceSpecificExitCode = 0; PathName = '"C:\Program Files\FoldSentinel Protection\foldsentinel-service.exe"' }
    Assert-ServiceStopped -Service $normal
    Assert-Throws { Assert-ServiceStopped -Service ([pscustomobject]@{ State = 'Running'; ExitCode = 0; ServiceSpecificExitCode = 0 }) } '运行中的服务不能通过正常卸载预检'
    Assert-Throws { Assert-ServiceStopped -Service ([pscustomobject]@{ State = 'Stopped'; ExitCode = 1066; ServiceSpecificExitCode = 1 }) } '失败退出不能当作安全停止'
    Assert-Throws { Assert-ServiceStopped -Service ([pscustomobject]@{ State = 'Stopped'; ExitCode = 0 }) } '未知退出码必须失败关闭'
    Assert-ServiceBinaryRegistration -Service $normal -ExpectedBinary 'C:\Program Files\FoldSentinel Protection\foldsentinel-service.exe'
    foreach ($path in @('C:\untrusted.exe', '"C:\untrusted.exe"', '"C:\Program Files\FoldSentinel Protection\foldsentinel-service.exe" --console')) {
        Assert-Throws { Assert-ServiceBinaryRegistration -Service ([pscustomobject]@{ PathName = $path }) -ExpectedBinary 'C:\Program Files\FoldSentinel Protection\foldsentinel-service.exe' } '非固定程序路径必须拒绝'
    }
    $checks += 'stopped_exit_codes_and_fixed_binary'

    $unknown = Join-Path $owned 'unknown'
    New-Item -ItemType Directory -Path $unknown | Out-Null
    [IO.File]::WriteAllText((Join-Path $unknown 'foldsentinel-service.exe'), 'owned')
    [IO.File]::WriteAllText((Join-Path $unknown 'recovery.json'), 'preserve')
    Assert-Throws { Assert-OwnedServiceProgramFiles -Inventory (Get-OwnedServiceProgramFiles -Directory $unknown) } '未知文件必须在删除SCM前被拒绝'
    Assert-Throws { Remove-OwnedServiceProgram -Directory $unknown } '清理时必须再次拒绝未知文件'
    Assert-True ([IO.File]::ReadAllText((Join-Path $unknown 'recovery.json')) -eq 'preserve') '未知文件内容被修改'
    Assert-True (Test-Path -LiteralPath (Join-Path $unknown 'foldsentinel-service.exe')) '预检失败时不应删除已知程序'
    $checks += 'unknown_content_is_preserved'

    $nested = Join-Path $owned 'nested'
    New-Item -ItemType Directory -Path (Join-Path $nested 'foldsentinel-service.exe') | Out-Null
    Assert-Throws { Remove-OwnedServiceProgram -Directory $nested } '同名目录不能视为白名单程序'
    $checks += 'directories_are_never_recursive_deleted'

    $target = Join-Path $owned 'target'
    $junction = Join-Path $owned 'junction'
    New-Item -ItemType Directory -Path $target | Out-Null
    [IO.File]::WriteAllText((Join-Path $target 'sentinel'), 'preserve')
    New-Item -ItemType Junction -Path $junction -Target $target | Out-Null
    Assert-Throws { Remove-OwnedServiceProgram -Directory $junction } '根目录重解析点必须拒绝'
    $child = Join-Path $junction 'child'
    New-Item -ItemType Directory -Path $child | Out-Null
    Assert-Throws { Remove-OwnedServiceProgram -Directory $child } '祖先重解析点必须拒绝'
    Assert-True ([IO.File]::ReadAllText((Join-Path $target 'sentinel')) -eq 'preserve') '重解析目标被修改'
    # Junction仅删除链接本身；不使用递归删除。
    [IO.Directory]::Delete($junction)
    $checks += 'reparse_root_and_ancestor_are_rejected'

    $clean = Join-Path $owned 'clean'
    New-Item -ItemType Directory -Path $clean | Out-Null
    [IO.File]::WriteAllText((Join-Path $clean 'foldsentinel-service.exe'), 'owned')
    Assert-OwnedServiceProgramFiles -Inventory (Get-OwnedServiceProgramFiles -Directory $clean)
    Remove-OwnedServiceProgram -Directory $clean
    Assert-True (-not (Test-Path -LiteralPath $clean)) '白名单程序和空目录应完整删除'
    $checks += 'known_program_and_empty_directory_removed'

    Assert-Throws { Confirm-ForcedUninstall -Interactive $false -ReadLine { throw '非交互不应读取输入' } } '静默Force必须拒绝'
    Assert-Throws { Confirm-ForcedUninstall -Interactive $true -ReadLine { 'force-foldsentinel' } -WarningAction SilentlyContinue } '大小写不精确的确认不能接受'
    $script:reads = 0
    Assert-Throws { Confirm-ForcedUninstall -Interactive $true -ReadLine { $script:reads++; if ($script:reads -eq 1) { 'FORCE-FOLDSENTINEL' } else { 'NO' } } -WarningAction SilentlyContinue } '第二次确认不匹配必须拒绝'
    $script:reads = 0
    Confirm-ForcedUninstall -Interactive $true -ReadLine { $script:reads++; 'FORCE-FOLDSENTINEL' } -WarningAction SilentlyContinue
    Assert-True ($script:reads -eq 2) 'Force必须要求两次独立精确确认'
    $checks += 'force_requires_interaction_and_two_exact_confirmations'

    Assert-Throws { Get-ProtectionService -Name 'isolated-test' } 'CIM查询错误不能当作未安装'
    function Get-CimInstance { @() }
    Assert-True ($null -eq (Get-ProtectionService -Name 'isolated-test')) '只有明确零记录才是未安装'
    function Get-CimInstance { [pscustomobject]@{ State = 'Stopped'; ExitCode = 0; ServiceSpecificExitCode = 0 } }
    Assert-True ((Wait-ProtectionServiceStopped -Name 'isolated-test').State -eq 'Stopped') '已停止替身应完成核对'
    function Get-CimInstance { @() }
    Wait-ProtectionServiceAbsent -Name 'isolated-test'
    $checks += 'query_errors_and_confirmed_absence_are_distinct'

    [pscustomobject]@{ passed = $true; checks = $checks; isolation = '临时自有目录与函数替身；未执行管理脚本顶层，未操作真实SCM或恢复状态' } | ConvertTo-Json -Depth 4
} finally {
    $resolvedOwned = [IO.Path]::GetFullPath($owned)
    $expectedParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolvedOwned.StartsWith($expectedParent, [StringComparison]::OrdinalIgnoreCase) -or
        -not ([IO.Path]::GetFileName($resolvedOwned)).StartsWith('foldsentinel-uninstall-test-', [StringComparison]::Ordinal)) {
        throw '测试目录越界，拒绝自动清理。'
    }
    if (Test-Path -LiteralPath $junction) { [IO.Directory]::Delete($junction) }
    Remove-Item -LiteralPath $resolvedOwned -Recurse -Force -ErrorAction Stop
}
