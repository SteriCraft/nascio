# Copies the development textures (textures_temp/blocks) next to the executables
# (target/<profile>/textures), where the viewer looks for them by default.
# Usage: .\scripts\sync-textures.ps1 [-Profile release|debug]
param([string[]]$Profile = @("release", "debug"))

$root = Split-Path -Parent $PSScriptRoot
$src = Join-Path $root "textures_temp\blocks"
if (-not (Test-Path $src)) { throw "missing folder: $src" }

foreach ($p in $Profile) {
    $targetDir = Join-Path $root "target\$p"
    if (-not (Test-Path $targetDir)) { Write-Host "skipped $p (no $targetDir)"; continue }
    $dst = Join-Path $targetDir "textures"
    New-Item -ItemType Directory -Force $dst | Out-Null
    # Mirror: files removed from the source disappear from the destination.
    Get-ChildItem $dst -Filter *.png | Remove-Item -Force
    Copy-Item (Join-Path $src "*.png") $dst
    $n = (Get-ChildItem $dst -Filter *.png | Measure-Object).Count
    Write-Host "$n textures -> $dst"
}
