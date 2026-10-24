param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    if (-not $SkipBuild) {
        & npm.cmd run tauri build
        if ($LASTEXITCODE -ne 0) { throw '桌面构建失败' }
        & cargo build --release --locked -p foldsentinel
        if ($LASTEXITCODE -ne 0) { throw 'CLI 构建失败' }
    }
    $artifactDirectory = Join-Path $projectRoot 'artifacts'
    New-Item -ItemType Directory -Force -Path $artifactDirectory | Out-Null
    $version = (Get-Content -Raw -LiteralPath (Join-Path $projectRoot 'package.json') | ConvertFrom-Json).version
    $portable = Join-Path $artifactDirectory "FoldSentinel_${version}_windows-x64_portable.zip"
    $files = @('target/release/FoldSentinel.exe', 'target/release/foldsentinel.exe', 'README.md', 'LICENSE', 'docs/FORMAT.md')
    foreach ($file in $files) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "缺少交付文件: $file" }
    }
    Compress-Archive -LiteralPath $files -DestinationPath $portable -Force
    $installer = Join-Path $projectRoot "target/release/bundle/nsis/FoldSentinel_${version}_x64-setup.exe"
    Copy-Item -LiteralPath $installer -Destination $artifactDirectory -Force
    $hashes = Get-ChildItem -LiteralPath $artifactDirectory -File | Where-Object { $_.Extension -in '.exe', '.zip' } | Sort-Object Name | ForEach-Object {
        $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
        "$($hash.Hash.ToLowerInvariant())  $($_.Name)"
    }
    [System.IO.File]::WriteAllLines((Join-Path $artifactDirectory 'SHA256SUMS.txt'), [string[]]$hashes, [System.Text.UTF8Encoding]::new($false))
    Get-ChildItem -LiteralPath $artifactDirectory -File | Select-Object Name, Length
} finally {
    Pop-Location
}
