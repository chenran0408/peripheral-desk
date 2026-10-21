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
# A silent caller can explicitly request launch; interactive installation always launches.
Run-Installer $setup ($argsForSetup + '/RESTARTAPP=1')
$deadline = [DateTime]::UtcNow.AddSeconds(15)
$launched = $null
while ([DateTime]::UtcNow -lt $deadline) {
    $launched = Get-Process -Name PeripheralDesk -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | Select-Object -First 1
    if ($launched -and $launched.MainWindowHandle -ne 0) { break }
    Start-Sleep -Milliseconds 100
}
if (-not $launched) { throw 'Installer did not automatically launch the installed app.' }
# Updating a running instance should close it normally and relaunch the replacement.
$previousId = $launched.Id
Run-Installer $setup $argsForSetup
$deadline = [DateTime]::UtcNow.AddSeconds(15)
$replacement = $null
while ([DateTime]::UtcNow -lt $deadline) {
    $replacement = Get-Process -Name PeripheralDesk -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe -and $_.Id -ne $previousId } | Select-Object -First 1
    if ($replacement -and $replacement.MainWindowHandle -ne 0) { break }
    Start-Sleep -Milliseconds 100
}
if (-not $replacement -or -not $launched.HasExited) { throw 'Updating a running app did not relaunch the replacement.' }
$launched = $replacement
# This is the isolated CI fixture process, identified by its exact executable path.
$launched.Kill()
$launched.WaitForExit()
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($fixture)) -ne [Convert]::ToBase64String($bytes)) { throw 'Automatic launch changed unknown user configuration.' }
Run-Installer (Join-Path $target 'unins000.exe') @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART')
if (-not (Test-Path -LiteralPath $fixture)) { throw 'Uninstall removed user configuration.' }
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($fixture)) -ne [Convert]::ToBase64String($bytes)) { throw 'Uninstall changed user configuration.' }
if (Test-Path -LiteralPath $exe) { throw 'Uninstall left the installed executable.' }
Write-Output 'Installer fresh install, reinstall and uninstall preserve user-created data.'
