param([Parameter(Mandatory=$true)][string]$Version)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$setup = Join-Path $projectRoot "dist/PeripheralDesk-$Version-windows-x64-setup.exe"
$testRoot = Join-Path $projectRoot 'work/installer-smoke'
$target = Join-Path $testRoot 'app'
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
function Run-Installer([string]$Path, [string[]]$Arguments) {
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(60000)) { $process.Kill(); throw 'Installer operation timed out.' }
    if ($process.ExitCode -ne 0) { throw "Installer operation failed: $($process.ExitCode)" }
}
$argsForSetup = @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"/DIR=`"$target`"","/LOG=`"$(Join-Path $testRoot 'install.log')`"")
Run-Installer $setup $argsForSetup
$exe = Join-Path $target 'PeripheralDesk.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw 'Installed EXE missing.' }
if ((Get-FileHash -LiteralPath $exe).Hash -ne (Get-FileHash -LiteralPath (Join-Path $projectRoot 'target/release/peripheral-desk.exe')).Hash) { throw 'Installer EXE differs from release build.' }
$data = Join-Path $target 'data'
New-Item -ItemType Directory -Path $data -Force | Out-Null
$fixture = Join-Path $data 'future-user-config.json'
$bytes = [Text.Encoding]::UTF8.GetBytes('{"unknown":{"preserve":true}}')
[IO.File]::WriteAllBytes($fixture,$bytes)
Run-Installer $setup $argsForSetup
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($fixture)) -ne [Convert]::ToBase64String($bytes)) { throw 'Reinstall changed user configuration.' }
Run-Installer (Join-Path $target 'unins000.exe') @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART')
if (-not (Test-Path -LiteralPath $fixture)) { throw 'Uninstall removed user configuration.' }
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($fixture)) -ne [Convert]::ToBase64String($bytes)) { throw 'Uninstall changed user configuration.' }
if (Test-Path -LiteralPath $exe) { throw 'Uninstall left the installed executable.' }
Write-Output 'Installer fresh install, reinstall and uninstall preserve user-created data.'
