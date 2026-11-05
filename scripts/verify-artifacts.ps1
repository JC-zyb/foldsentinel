param([string]$Directory, [switch]$VerifyBuildPayload)
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
    foreach ($required in @('FoldSentinel.exe', 'foldsentinel-cli.exe', 'foldsentinel-service.exe', 'manage-protection-service.ps1', 'README.md', 'LICENSE', 'CHANGELOG.md', 'docs/FORMAT.md', 'docs/INSTALL.md', 'docs/VALIDATION.md', 'docs/SERVICE.md')) {
        if ($required -notin $names) { throw "便携包缺少文件: $required" }
    }
    if (($names | Select-Object -Unique).Count -ne $names.Count) { throw '便携包包含重复路径' }
    foreach ($name in $names) {
        if ($name -match '(^/|(^|/)\.\.(/|$)|:)' -or ($name -notin @('FoldSentinel.exe', 'foldsentinel-cli.exe', 'foldsentinel-service.exe', 'manage-protection-service.ps1', 'README.md', 'LICENSE', 'CHANGELOG.md') -and -not $name.StartsWith('docs/'))) {
            throw "便携包含意外文件或不安全路径: $name"
        }
    }
    if ($VerifyBuildPayload) {
        $service = Join-Path $root 'target/release/foldsentinel-service.exe'
        $sourceHash = (Get-FileHash -LiteralPath $service -Algorithm SHA256).Hash
        $stream = $zip.GetEntry('foldsentinel-service.exe').Open()
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $zipHash = [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '') }
        finally { $sha.Dispose(); $stream.Dispose() }
        if ($sourceHash -ne $zipHash) { throw '便携包服务与本次构建不一致' }
        $nsis = Get-Content -Raw -LiteralPath (Join-Path $root 'target/release/nsis/x64/installer.nsi')
        if ($nsis -notmatch ('(?m)^!define VERSION "' + [Regex]::Escape($version) + '"\r?$')) { throw 'NSIS 版本与本次打包不一致' }
        if ($nsis -notmatch '(?m)^\s*File .*foldsentinel-service\.exe"\r?$' -or $nsis -notmatch '(?m)^\s*Delete "\$INSTDIR\\foldsentinel-service\.exe"\r?$') {
            throw 'NSIS 缺少服务 payload 的安装或卸载指令；必须通过 scripts/package.ps1 完整打包'
        }
        $bundleConfig = Get-Content -Raw -LiteralPath (Join-Path $root 'src-tauri/tauri.bundle.conf.json') | ConvertFrom-Json
        if ($bundleConfig.bundle.resources.'../target/release/foldsentinel-service.exe' -ne 'foldsentinel-service.exe') { throw '服务 payload 必须位于安装根目录' }
    }
} finally { $zip.Dispose() }
Write-Output "Artifacts and SHA-256 OK: $version"
