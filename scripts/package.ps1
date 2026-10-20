$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $projectRoot 'target' }
    $binary = Join-Path $targetDir 'release/peripheral-desk.exe'
    if (-not (Test-Path -LiteralPath $binary)) { throw 'Run cargo build --release first.' }
    $dist = Join-Path $projectRoot 'dist'
    $bundle = Join-Path $dist "PeripheralDesk-$version-windows-x64"
    New-Item -ItemType Directory -Force -Path $bundle | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $bundle 'PeripheralDesk.exe') -Force
    foreach ($name in @('README.md','CHANGELOG.md','NOTICE','LICENSE','SOURCES.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot $name) -Destination $bundle -Force
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot 'THIRD_PARTY_LICENSES') -Destination $bundle -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs') -Destination $bundle -Recurse -Force
    $archive = "$bundle.zip"
    Compress-Archive -Path (Join-Path $bundle '*') -DestinationPath $archive -Force
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $(Split-Path -Leaf $archive)" | Set-Content -LiteralPath (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii
    Write-Output $archive
} finally { Pop-Location }
