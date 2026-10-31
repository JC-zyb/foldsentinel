[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Install', 'Status', 'Start', 'Stop', 'Uninstall')]
    [string]$Action,
    [string]$ServiceBinary,
    [switch]$Force,
    [switch]$NonInteractive
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

function Get-ProtectionService {
    param([Parameter(Mandatory = $true)][string]$Name)
    $records = @(Get-CimInstance -ClassName Win32_Service -Filter ("Name='{0}'" -f $Name) -ErrorAction Stop)
    if ($records.Count -gt 1) { throw "服务登记出现重复记录，拒绝继续：$Name" }
    if ($records.Count -eq 0) { return $null }
    return $records[0]
}

function Assert-ServiceStopped {
    param([Parameter(Mandatory = $true)]$Service)
    if ($Service.State -ne 'Stopped') { throw "服务当前不是已停止状态（$($Service.State)），请先显式停止。" }
    foreach ($propertyName in @('ExitCode', 'ServiceSpecificExitCode')) {
        $property = $Service.PSObject.Properties[$propertyName]
        if ($null -eq $property -or $null -eq $property.Value) {
            throw "无法核对服务退出码（缺少 $propertyName），拒绝卸载。"
        }
        if ([uint32]$property.Value -ne 0) {
            throw "服务退出码为 $($property.Value)，不能视为已安全停止。"
        }
    }
}

function Assert-ServiceBinaryRegistration {
    param([Parameter(Mandatory = $true)]$Service, [Parameter(Mandatory = $true)][string]$ExpectedBinary)
    $rawPath = [string]$Service.PathName
    if ([string]::IsNullOrWhiteSpace($rawPath) -or -not $rawPath.StartsWith('"') -or -not $rawPath.EndsWith('"')) {
        throw '服务程序路径不是受信任的固定带引号路径，拒绝卸载。'
    }
    $registeredBinary = $rawPath.Substring(1, $rawPath.Length - 2)
    if (-not [string]::Equals([IO.Path]::GetFullPath($registeredBinary), [IO.Path]::GetFullPath($ExpectedBinary), [StringComparison]::OrdinalIgnoreCase)) {
        throw "服务程序路径不是本次安装的固定文件，拒绝卸载：$registeredBinary"
    }
}

function Get-OwnedServiceProgramFiles {
    param([Parameter(Mandatory = $true)][string]$Directory)
    $directoryItem = Get-Item -LiteralPath $Directory -Force -ErrorAction Stop
    if (-not $directoryItem.PSIsContainer -or ($directoryItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw '服务程序目录不是普通目录，拒绝清理。'
    }
    $ancestor = $directoryItem.Parent
    while ($null -ne $ancestor) {
        if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw '服务程序目录的父路径包含重解析项，拒绝清理。' }
        $ancestor = $ancestor.Parent
    }
    $known = @(); $unknown = @()
    foreach ($entry in @(Get-ChildItem -LiteralPath $Directory -Force -ErrorAction Stop)) {
        if ($entry.Name -ieq 'foldsentinel-service.exe' -and -not $entry.PSIsContainer -and -not ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            $known += $entry
        } else {
            $unknown += $entry
        }
    }
    [pscustomobject]@{ Known = @($known); Unknown = @($unknown) }
}

function Assert-OwnedServiceProgramFiles {
    param([Parameter(Mandatory = $true)]$Inventory)
    if ($Inventory.Unknown.Count -ne 0) {
        $names = ($Inventory.Unknown | ForEach-Object Name) -join ', '
        throw "固定服务目录包含未知文件或重解析项（$names），拒绝删除；请人工处理后重试。"
    }
}

function Remove-OwnedServiceProgram {
    param([Parameter(Mandatory = $true)][string]$Directory)
    $inventory = Get-OwnedServiceProgramFiles -Directory $Directory
    Assert-OwnedServiceProgramFiles -Inventory $inventory
    foreach ($entry in $inventory.Known) { Remove-Item -LiteralPath $entry.FullName -Force -ErrorAction Stop }
    if (@(Get-ChildItem -LiteralPath $Directory -Force -ErrorAction Stop).Count -ne 0) { throw '固定服务目录清理后仍有内容，拒绝删除目录。' }
    Remove-Item -LiteralPath $Directory -Force -ErrorAction Stop
    if (Test-Path -LiteralPath $Directory) { throw '固定服务目录删除后仍存在。' }
}

function Confirm-ForcedUninstall {
    param([Parameter(Mandatory = $true)][bool]$Interactive, [scriptblock]$ReadLine)
    if (-not $Interactive) { throw 'Force 卸载仅允许明确的交互式管理员终端；静默或输入重定向会拒绝执行。' }
    if ($null -eq $ReadLine) { $ReadLine = { param([string]$Prompt) Read-Host $Prompt } }
    Write-Warning '强制卸载会跳过“全部恢复记录为空”检查；受管文件可能仍保持保护且无法访问。ProgramData 中的恢复记录会保留，必须由管理员稍后恢复或清理。'
    if ((& $ReadLine '若理解并承担上述风险，请输入 FORCE-FOLDSENTINEL') -cne 'FORCE-FOLDSENTINEL') { throw '未确认强制卸载，已取消。' }
    if ((& $ReadLine '再次输入 FORCE-FOLDSENTINEL 以确认删除服务与固定程序文件') -cne 'FORCE-FOLDSENTINEL') { throw '第二次确认不匹配，已取消。' }
}

function Wait-ProtectionServiceStopped {
    param([Parameter(Mandatory = $true)][string]$Name)
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    do {
        $current = Get-ProtectionService -Name $Name
        if ($null -eq $current) { throw '服务登记在停止核对期间消失，拒绝继续。' }
        if ($current.State -eq 'Stopped') { return $current }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    throw '服务在120秒内未进入已停止状态。'
}

function Wait-ProtectionServiceAbsent {
    param([Parameter(Mandatory = $true)][string]$Name)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $current = Get-ProtectionService -Name $Name
        if ($null -eq $current) { return }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    throw '服务删除命令已返回，但服务登记仍存在或处于待删除状态。'
}

if ($Force -and $Action -ne 'Uninstall') { throw '-Force 只适用于 Uninstall，不能用于其他服务操作。' }

if ($Action -eq 'Status') {
    $service = Get-ProtectionService -Name $serviceName
    if ($null -eq $service) { Write-Output '未安装持续防护服务。'; exit 0 }
    $service | Select-Object Name, DisplayName, State, StartMode, ExitCode, ServiceSpecificExitCode
    exit 0
}

Assert-Administrator
$service = Get-ProtectionService -Name $serviceName
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
        $stopped = Wait-ProtectionServiceStopped -Name $serviceName
        Write-Output "服务已停止；服务退出码=$($stopped.ExitCode)，服务专用退出码=$($stopped.ServiceSpecificExitCode)。请查看应用中的逐项状态。"
    }
    exit 0
}
if ($Action -eq 'Uninstall') {
    Assert-ServiceBinaryRegistration -Service $service -ExpectedBinary $installedBinary
    if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) { throw '缺少受信任安装目录内的服务程序，不能安全核对恢复状态。' }
    Assert-OwnedServiceProgramFiles -Inventory (Get-OwnedServiceProgramFiles -Directory $installDirectory)
    if ($Force) {
        $nonInteractive = $NonInteractive -or @([Environment]::GetCommandLineArgs() | Where-Object { $_.StartsWith('-NonI', [StringComparison]::OrdinalIgnoreCase) }).Count -ne 0
        $interactive = [Environment]::UserInteractive -and -not $nonInteractive -and -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected
        Confirm-ForcedUninstall -Interactive $interactive
    } else {
        Assert-ServiceStopped -Service $service
        & $installedBinary --audit-empty
        if ($LASTEXITCODE -ne 0) { throw '受管项目仍存在或恢复状态无法验证，拒绝卸载；恢复状态未删除。' }
    }
    if ($PSCmdlet.ShouldProcess($serviceName, $(if ($Force) { '强制删除服务登记；保留ProgramData恢复记录' } else { '删除无受管项目的服务登记；保留ProgramData恢复资料' }))) {
        $service = Get-ProtectionService -Name $serviceName
        if ($null -eq $service) { throw '服务登记在卸载前消失，拒绝继续。' }
        Assert-ServiceBinaryRegistration -Service $service -ExpectedBinary $installedBinary
        if ($Force) {
            if ($service.State -ne 'Stopped') {
                Stop-Service -Name $serviceName
                $service = Wait-ProtectionServiceStopped -Name $serviceName
            }
            Assert-ServiceStopped -Service $service
        } else { Assert-ServiceStopped -Service $service }
        Invoke-ServiceCommand -ServiceArguments @('delete', $serviceName)
        Wait-ProtectionServiceAbsent -Name $serviceName
        Remove-OwnedServiceProgram -Directory $installDirectory
        if ($Force) { Write-Warning '已强制删除服务登记和固定程序文件；ProgramData 恢复记录已保留，受保护文件可能仍不可访问。' }
        else { Write-Output '已删除服务登记和固定程序文件；ProgramData 恢复资料已保留，未递归删除未知文件或恢复元数据。' }
    }
}
