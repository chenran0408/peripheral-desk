param([string]$Work, [string]$Compiler)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$version = (Select-String -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
$dist = Join-Path $projectRoot 'dist'
$core = Join-Path $dist "PeripheralDesk-$version-windows-x64.zip"
if (!(Test-Path -LiteralPath $core)) { throw 'Run scripts/package.ps1 before building the installer.' }
if (!$Work) { $Work = Join-Path $projectRoot 'work/mhub-package' }
if (!$Compiler) {
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    $Compiler = if ($command) { $command.Source }
        elseif (Test-Path -LiteralPath 'D:/Dev/Toolchains/InnoSetup/ISCC.exe') { 'D:/Dev/Toolchains/InnoSetup/ISCC.exe' }
        else { Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe' }
}
if (!(Test-Path -LiteralPath $Compiler)) { throw 'Provide -Compiler with the path to Inno Setup 6 ISCC.exe.' }
& (Join-Path $PSScriptRoot 'pack-reviewed-hub.ps1') -SourceLock (Join-Path $projectRoot 'installer/mhub/source.lock.json') -CoreZip $core -Output $dist -Work $Work -Compiler $Compiler
