param([string]$Directory)
$ErrorActionPreference = 'Stop'
if (-not $Directory) { throw '必须指定产物目录' }
$root = Split-Path -Parent $PSScriptRoot
$version = (Get-Content -Raw -LiteralPath (Join-Path $root 'package.json') | ConvertFrom-Json).version
$directoryPath = (Resolve-Path -LiteralPath $Directory).ProviderPath
$expected = @("FoldSentinel_${version}_windows-x64_portable.zip", "FoldSentinel_${version}_x64-setup.exe")
$sums = Get-Content -LiteralPath (Join-Path $directoryPath 'SHA256SUMS.txt')
if ($sums.Count -ne 2) { throw '校验和文件必须恰好包含两个产物' }
$seen = @{}
foreach ($line in $sums) {
    if ($line -notmatch '^([a-f0-9]{64})  ([A-Za-z0-9_.-]+)$') { throw '校验和行格式无效' }
    $digest = $Matches[1]; $name = $Matches[2]
    if ($name -notin $expected -or $seen.ContainsKey($name)) { throw "校验和包含意外或重复文件: $name" }
    $seen[$name] = $true
    $file = Join-Path $directoryPath $name
    if ((Get-Item -LiteralPath $file).Length -lt 1024) { throw "产物异常小: $name" }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() -ne $digest) { throw "校验和不一致: $name" }
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead((Join-Path $directoryPath $expected[0]))
try {
    $names = @($zip.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    foreach ($required in @('FoldSentinel.exe', 'foldsentinel-cli.exe', 'README.md', 'LICENSE', 'CHANGELOG.md', 'docs/FORMAT.md', 'docs/INSTALL.md', 'docs/VALIDATION.md')) {
        if ($required -notin $names) { throw "便携包缺少文件: $required" }
    }
    if (($names | Select-Object -Unique).Count -ne $names.Count) { throw '便携包包含重复路径' }
    foreach ($name in $names) {
        if ($name -match '(^/|(^|/)\.\.(/|$)|:)' -or ($name -notin @('FoldSentinel.exe', 'foldsentinel-cli.exe', 'README.md', 'LICENSE', 'CHANGELOG.md') -and -not $name.StartsWith('docs/'))) {
            throw "便携包含意外文件或不安全路径: $name"
        }
    }
} finally { $zip.Dispose() }
Write-Output "Artifacts and SHA-256 OK: $version"
