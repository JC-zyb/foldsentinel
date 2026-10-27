[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Install', 'Status', 'Start', 'Stop', 'Uninstall')]
    [string]$Action,
    [string]$ServiceBinary
)

$ErrorActionPreference = 'Stop'
$serviceName = 'FoldSentinelProtection'
$installDirectory = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'FoldSentinel Protection'
$installedBinary = Join-Path $installDirectory 'foldsentinel-service.exe'

function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw '此操作需要管理员PowerShell；脚本不会自动提权。'
    }
}

function Invoke-ServiceCommand {
    param([string[]]$ServiceArguments)
    & "$env:SystemRoot\System32\sc.exe" @ServiceArguments
    if ($LASTEXITCODE -ne 0) { throw "服务管理命令失败（$LASTEXITCODE）" }
}

if ($Action -eq 'Status') {
    $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if ($null -eq $service) { Write-Output '未安装持续防护服务。'; exit 0 }
    $service | Select-Object Name, DisplayName, Status, StartType
    exit 0
}

Assert-Administrator
$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($Action -eq 'Install') {
    if ($null -ne $service) { throw '服务已安装。请先解除全部受管保护并卸载旧服务；本脚本不覆盖活跃服务或恢复状态。' }
    if ([string]::IsNullOrWhiteSpace($ServiceBinary)) { throw '安装时必须指定已构建的-ServiceBinary。' }
    $sourceBinary = (Resolve-Path -LiteralPath $ServiceBinary).ProviderPath
    if ([IO.Path]::GetExtension($sourceBinary) -ine '.exe' -or (Get-Item -LiteralPath $sourceBinary).PSIsContainer) { throw '服务源文件必须是exe。' }
    if (Test-Path -LiteralPath $installDirectory) { throw "安装目录已存在，拒绝覆盖：$installDirectory" }
    $sourceHash = (Get-FileHash -LiteralPath $sourceBinary -Algorithm SHA256).Hash
    Write-Output "安装来源SHA256：$sourceHash"
    if ($PSCmdlet.ShouldProcess("$serviceName；SYSTEM；自动启动；$installedBinary", '安装持续防护服务并启动；恢复状态保留在ProgramData')) {
        New-Item -ItemType Directory -Path $installDirectory | Out-Null
        $directoryAcl = [Security.AccessControl.DirectorySecurity]::new()
        $directoryAcl.SetAccessRuleProtection($true, $false)
        $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
        foreach ($sidText in @('S-1-5-18', 'S-1-5-32-544')) {
            $sid = [Security.Principal.SecurityIdentifier]::new($sidText)
            $directoryAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($sid, 'FullControl', $inherit, 'None', 'Allow'))
        }
        Set-Acl -LiteralPath $installDirectory -AclObject $directoryAcl
        Copy-Item -LiteralPath $sourceBinary -Destination $installedBinary
        if ((Get-FileHash -LiteralPath $installedBinary -Algorithm SHA256).Hash -ne $sourceHash) { throw '复制后的服务哈希不一致，尚未登记服务。' }
        New-Service -Name $serviceName -DisplayName 'FoldSentinel 持续防护' -BinaryPathName ('"{0}"' -f $installedBinary) -StartupType Automatic -Description '按Windows账户隔离的目录密码访问、到期重锁与持续防护。卸载前必须解除全部受管项目。' | Out-Null
        Invoke-ServiceCommand -ServiceArguments @('failure', $serviceName, 'reset=', '86400', 'actions=', 'restart/5000/restart/15000/restart/60000')
        Invoke-ServiceCommand -ServiceArguments @('failureflag', $serviceName, '1')
        Start-Service -Name $serviceName
        (Get-Service -Name $serviceName).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
        Write-Output '服务已安装并运行。安装状态不等于目录防护已开启，请在应用中核对受管项目与错误。'
    }
    exit 0
}

if ($null -eq $service) { throw '服务未安装。' }
if ($Action -eq 'Start') {
    if ($PSCmdlet.ShouldProcess($serviceName, '启动持续防护服务')) {
        Start-Service -Name $serviceName
        (Get-Service -Name $serviceName).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
    }
    exit 0
}
if ($Action -eq 'Stop') {
    if ($PSCmdlet.ShouldProcess($serviceName, '停止持续防护；服务先尝试重锁临时访问，已锁项目继续保留，停止后不再补查新项')) {
        Stop-Service -Name $serviceName
        (Get-Service -Name $serviceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(120))
        Write-Output '服务已停止。请查看服务退出码及应用中的逐项状态；退出失败不能视为全部临时访问已重锁。'
    }
    exit 0
}
if ($Action -eq 'Uninstall') {
    if ($service.Status -ne 'Stopped') { throw '先显式停止服务并核对临时访问重锁结果，再执行卸载。' }
    if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) { throw '缺少受信任安装目录内的服务程序，不能安全核对恢复状态。' }
    & $installedBinary --audit-empty
    if ($LASTEXITCODE -ne 0) { throw '受管项目仍存在或恢复状态无法验证，拒绝卸载；恢复状态未删除。' }
    if ($PSCmdlet.ShouldProcess($serviceName, '删除无受管项目的服务登记；保留程序与所有恢复资料，便于审计和恢复')) {
        Invoke-ServiceCommand -ServiceArguments @('delete', $serviceName)
        Write-Output '已删除服务登记。安装程序目录和ProgramData资料均保留，未递归删除任何文件。'
    }
}
