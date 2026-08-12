# ZenDesktop Release Builder
# Creates: portable .exe + .zip + .msi installer
param(
    [string]$Version = "1.0.0"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$Target = "$Root\target\release"
$ReleaseDir = "$Root\..\..\release\$Version"

Write-Host "=== ZenDesktop Release Builder v$Version ===" -ForegroundColor Cyan

# 1. Build release
Write-Host "[1/4] Building release..." -ForegroundColor Yellow
cargo build --release --manifest-path "$Root\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

# 2. Prepare release directory
Write-Host "[2/4] Preparing release dir..." -ForegroundColor Yellow
New-Item -ItemType Directory -Force -Path $ReleaseDir | Out-Null

# 3. Copy and compress
Write-Host "[3/4] Creating artifacts..." -ForegroundColor Yellow

# Portable .exe
Copy-Item "$Target\zendesktop.exe" "$ReleaseDir\ZenDesktop.exe" -Force

# Portable .zip
Compress-Archive -Path "$ReleaseDir\ZenDesktop.exe" `
    -DestinationPath "$ReleaseDir\ZenDesktop-v$Version-portable.zip" -Force

# Create MSI with WiX if available
$wix = Get-Command "candle.exe" -ErrorAction SilentlyContinue
if ($wix) {
    Write-Host "  Building MSI installer..." -ForegroundColor Gray
    pushd "$ReleaseDir"
    & candle.exe "$Root\..\installer\zendesktop.wxs" -arch x64
    & light.exe "zendesktop.wixobj" -out "ZenDesktop-v$Version-x64.msi"
    popd
} else {
    Write-Host "  WiX not found - skipping MSI (install from https://wixtoolset.org)" -ForegroundColor Gray
}

# 4. Done
Write-Host "[4/4] Done!" -ForegroundColor Green
Write-Host ""
Write-Host "Artifacts in: $ReleaseDir" -ForegroundColor White
Get-ChildItem $ReleaseDir | ForEach-Object { Write-Host "  $($_.Name) ($('{0:N0}' -f ($_.Length/1KB)) KB)" }
