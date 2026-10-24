param([Parameter(Mandatory = $true)][string]$Report)
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw '此隔离 VHD 测试需要管理员权限；未修改磁盘。' }
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target/release/examples/validate_v04.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw '先构建 cargo build --release --locked --example validate_v04' }
$reportPath = [IO.Path]::GetFullPath($Report)
if (Test-Path -LiteralPath $reportPath) { throw '报告已存在，拒绝覆盖' }
$token = [Guid]::NewGuid().ToString('N')
$owned = Join-Path ([IO.Path]::GetTempPath()) "foldsentinel-volume-$token"
if ($owned -match '[^\x20-\x7e]') { throw 'diskpart 脚本要求 ASCII 临时目录；请为本次进程指定 ASCII TEMP 后重试。未修改磁盘。' }
New-Item -ItemType Directory -Path $owned | Out-Null
$vhd = Join-Path $owned 'isolated-128m.vhd'
$diskpartScript = Join-Path $owned 'attach.txt'
# 只创建新的明确文件，绝不 select disk 或接收物理磁盘编号。
[IO.File]::WriteAllLines($diskpartScript, @("create vdisk file=`"$vhd`" maximum=128 type=expandable", "select vdisk file=`"$vhd`"", 'attach vdisk'), [Text.Encoding]::ASCII)
try {
    & diskpart.exe /s $diskpartScript
    if ($LASTEXITCODE -ne 0) { throw '创建或挂载隔离 VHD 失败' }
    $image = Get-DiskImage -ImagePath $vhd -ErrorAction Stop
    if (-not $image.Attached) { throw '隔离 VHD 未挂载' }
    $disks = @($image | Get-Disk -ErrorAction Stop)
    if ($disks.Count -ne 1) { throw '不能唯一定位新建 VHD' }
    $disk = $disks[0]
    if ($disk.Size -ne 128MB -or $disk.IsBoot -or $disk.IsSystem -or $disk.PartitionStyle -ne 'RAW') { throw '目标不是预期的新建 128 MiB RAW VHD，拒绝初始化' }
    $disk | Initialize-Disk -PartitionStyle MBR -ErrorAction Stop
    $partition = New-Partition -DiskNumber $disk.Number -UseMaximumSize -AssignDriveLetter -ErrorAction Stop
    $partition | Format-Volume -FileSystem NTFS -NewFileSystemLabel 'FoldSentinelTest' -Confirm:$false -ErrorAction Stop | Out-Null
    if (-not $partition.DriveLetter) { throw '隔离卷没有盘符' }
    $volumeRoot = "$($partition.DriveLetter):\"
    [IO.File]::WriteAllText((Join-Path $volumeRoot 'foldsentinel-test-volume'), $token, [Text.UTF8Encoding]::new($false))
    & $exe --report $reportPath --volume-root $volumeRoot --volume-token $token
    if ($LASTEXITCODE -ne 0) { throw '真实 IO / 隔离卷写满验证失败' }
} finally {
    # 从明确的自有镜像路径卸载；保留 VHD 及脚本以便追溯，不删除其他文件。
    if (Test-Path -LiteralPath $vhd -PathType Leaf) {
        $image = Get-DiskImage -ImagePath $vhd -ErrorAction Stop
        if ($image.Attached) { Dismount-DiskImage -ImagePath $vhd -ErrorAction Stop }
        if ((Get-DiskImage -ImagePath $vhd -ErrorAction Stop).Attached) { throw "隔离 VHD 卸载失败: $vhd" }
        Write-Output '隔离 VHD 已确认未挂载。'
    }
    Write-Output "测试文件保留于: $owned"
}
