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
$null = New-Item -ItemType Directory -Path $Work -Force
function Expand-ReviewedSource($source) {
    $zip = Join-Path $Work $source.asset
    if (!(Test-Path -LiteralPath $zip)) { Invoke-WebRequest -Uri $source.url -OutFile $zip }
    if ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ne $source.sha256) {
        throw "The driver center source does not match the reviewed revision: $($source.asset)"
    }
    $folder = Join-Path $Work ('snapshot-' + [guid]::NewGuid())
    Expand-Archive -LiteralPath $zip -DestinationPath $folder
    Join-Path $folder 'upstream'
}
$workspace = Expand-ReviewedSource $pin
$extras = @(foreach ($source in $pin.supplements) {
    '--vendor-extra'
    Expand-ReviewedSource $source
})
python (Join-Path $PSScriptRoot 'package-mhub.py') --workspace $workspace @extras --core-zip $CoreZip --output $Output --work $Work --iscc $Compiler
if ($LASTEXITCODE -ne 0) { throw 'Driver center packaging failed.' }
