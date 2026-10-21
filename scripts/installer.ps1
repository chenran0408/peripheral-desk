$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$version = (Select-String -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
$bundle = Join-Path $projectRoot "dist/PeripheralDesk-$version-windows-x64"
$dist = Join-Path $projectRoot 'dist'
if (-not (Test-Path -LiteralPath (Join-Path $bundle 'PeripheralDesk.exe'))) { throw 'Run package.ps1 before building the installer.' }
if (Test-Path -LiteralPath (Join-Path $bundle 'data')) { throw 'Installer payload must not contain user data.' }
$compiler = Get-Command ISCC.exe -ErrorAction SilentlyContinue
$compilerPath = if ($compiler) { $compiler.Source } else { Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe' }
if (-not (Test-Path -LiteralPath $compilerPath)) { throw 'Inno Setup 6 is required; GitHub Windows runners include it.' }
& $compilerPath "/DAppVersion=$version" "/DBundleDir=$bundle" "/DOutputPath=$dist" (Join-Path $projectRoot 'installer/PeripheralDesk.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
$files = @("PeripheralDesk-$version-windows-x64.zip", "PeripheralDesk-$version-windows-x64-setup.exe")
$lines = foreach ($name in $files) {
    $hash = (Get-FileHash -LiteralPath (Join-Path $dist $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name"
}
$lines | Set-Content -LiteralPath (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii
