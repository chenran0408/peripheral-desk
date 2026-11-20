$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
$previousFlags = $env:RUSTFLAGS
try {
    $env:RUSTFLAGS = "$previousFlags -C target-feature=+crt-static".Trim()
    cargo build --release --locked --target x86_64-pc-windows-msvc --no-default-features --features desktop --bin peripheral-desk
    if ($LASTEXITCODE -ne 0) { throw "Windows desktop build failed" }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    $release = Join-Path $target 'release'
    New-Item -ItemType Directory -Force -Path $release | Out-Null
    Copy-Item -LiteralPath (Join-Path $target 'x86_64-pc-windows-msvc/release/peripheral-desk.exe') -Destination (Join-Path $release 'peripheral-desk.exe') -Force
} finally {
    $env:RUSTFLAGS = $previousFlags
    Pop-Location
}
