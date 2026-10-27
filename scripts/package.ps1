param([switch]$SkipBuild, [string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    & node (Join-Path $PSScriptRoot 'verify-versions.mjs')
    if ($LASTEXITCODE -ne 0) { throw '版本检查失败' }
    if (-not $SkipBuild) {
        & npm.cmd run tauri build
        if ($LASTEXITCODE -ne 0) { throw '桌面构建失败' }
        & cargo build --release --locked -p foldsentinel
        if ($LASTEXITCODE -ne 0) { throw 'CLI 与保护服务构建失败' }
    }
    $version = (Get-Content -Raw -LiteralPath (Join-Path $projectRoot 'package.json') | ConvertFrom-Json).version
    $artifactDirectory = if ($OutputDirectory) { [IO.Path]::GetFullPath($OutputDirectory, $projectRoot) } else { Join-Path $projectRoot "artifacts/$version" }
    New-Item -ItemType Directory -Force -Path $artifactDirectory | Out-Null
    $portable = Join-Path $artifactDirectory "FoldSentinel_${version}_windows-x64_portable.zip"
    $files = @('target/release/FoldSentinel.exe', 'target/release/foldsentinel-cli.exe', 'target/release/foldsentinel-service.exe', 'scripts/manage-protection-service.ps1', 'README.md', 'LICENSE', 'CHANGELOG.md')
    foreach ($file in $files) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "缺少交付文件: $file" }
    }
    if (-not (Test-Path -LiteralPath 'docs' -PathType Container)) { throw '缺少 docs 目录' }
    $files += 'docs'
    Compress-Archive -LiteralPath $files -DestinationPath $portable -Force
    $installer = Join-Path $projectRoot "target/release/bundle/nsis/FoldSentinel_${version}_x64-setup.exe"
    Copy-Item -LiteralPath $installer -Destination $artifactDirectory -Force
    $hashes = Get-Item -LiteralPath $portable, (Join-Path $artifactDirectory "FoldSentinel_${version}_x64-setup.exe") | Sort-Object Name | ForEach-Object {
        $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
        "$($hash.Hash.ToLowerInvariant())  $($_.Name)"
    }
    [System.IO.File]::WriteAllLines((Join-Path $artifactDirectory 'SHA256SUMS.txt'), [string[]]$hashes, [System.Text.UTF8Encoding]::new($false))
    & (Join-Path $PSScriptRoot 'verify-artifacts.ps1') -Directory $artifactDirectory
    Get-ChildItem -LiteralPath $artifactDirectory -File | Select-Object Name, Length
} finally {
    Pop-Location
}
