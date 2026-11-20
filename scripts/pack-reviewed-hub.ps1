param(
    [Parameter(Mandatory)][string]$SourceLock,
    [Parameter(Mandatory)][string]$CoreZip,
    [Parameter(Mandatory)][string]$Output,
    [Parameter(Mandatory)][string]$Work,
    [string]$Compiler = (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe')
)
$ErrorActionPreference = 'Stop'
$pin = Get-Content -Raw -LiteralPath $SourceLock | ConvertFrom-Json
# Rebuild from the reviewed public source archive even when the vendor changes its website.
$sourceName = "PeripheralDesk-$($pin.version)-mhub-web-source.zip"
$url = "https://github.com/chenran0408/peripheral-desk/releases/download/v$($pin.version)/$sourceName"
$null = New-Item -ItemType Directory -Path $Work -Force
$zip = Join-Path $Work $sourceName
if (!(Test-Path -LiteralPath $zip)) { Invoke-WebRequest -Uri $url -OutFile $zip }
if ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ne $pin.sha256) {
    throw 'The downloaded driver center source does not match the reviewed revision.'
}
$folder = Join-Path $Work ('snapshot-' + [guid]::NewGuid())
Expand-Archive -LiteralPath $zip -DestinationPath $folder
python (Join-Path $PSScriptRoot 'package-mhub.py') --workspace (Join-Path $folder 'upstream') --core-zip $CoreZip --output $Output --work $Work --iscc $Compiler
if ($LASTEXITCODE -ne 0) { throw 'Driver center packaging failed.' }
