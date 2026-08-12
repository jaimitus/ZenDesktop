# ZenDesktop Release Builder
# Creates: portable .exe + .zip + .msi installer
param(
    [string]$Version = "1.0.0"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$Target = "$Root\target\release"
$ReleaseDir = "$Root\release\$Version"
$InstallerDir = "$Root\installer"

Write-Host "=== ZenDesktop Release Builder v$Version ===" -ForegroundColor Cyan

# 1. Build release
Write-Host "[1/5] Building release..." -ForegroundColor Yellow
cargo build --release --manifest-path "$Root\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

# 2. Prepare release directory
Write-Host "[2/5] Preparing release dir..." -ForegroundColor Yellow
New-Item -ItemType Directory -Force -Path $ReleaseDir | Out-Null

# 3. Copy and compress portable
Write-Host "[3/5] Creating portable artifacts..." -ForegroundColor Yellow

# Portable .exe (versioned)
Copy-Item "$Target\zendesktop.exe" "$ReleaseDir\ZenDesktop-v$Version.exe" -Force

# Portable .exe (generic name)
Copy-Item "$Target\zendesktop.exe" "$ReleaseDir\ZenDesktop.exe" -Force

# Portable .zip (with docs)
$zipDir = "$ReleaseDir\portable-tmp"
New-Item -ItemType Directory -Force -Path $zipDir | Out-Null
Copy-Item "$Target\zendesktop.exe" "$zipDir\ZenDesktop.exe" -Force
Copy-Item "$Root\LICENSE" "$zipDir\" -Force
Copy-Item "$Root\README.md" "$zipDir\" -Force
Compress-Archive -Path "$zipDir\*" `
    -DestinationPath "$ReleaseDir\ZenDesktop-v$Version-portable.zip" -Force
Remove-Item -Recurse -Force $zipDir

# 4. Build MSI installer with WiX v4
Write-Host "[4/5] Building MSI installer..." -ForegroundColor Yellow

# Check if WiX v4 dotnet tool is available
$wixGlobal = "$env:USERPROFILE\.dotnet\tools\wix.exe"
$wixLocal = (Get-Command "wix.exe" -ErrorAction SilentlyContinue)

$wixExe = $null
if (Test-Path $wixGlobal) {
    $wixExe = $wixGlobal
} elseif ($wixLocal) {
    $wixExe = $wixLocal.Source
}

if ($wixExe) {
    # Prepare staging dir with all files
    $staging = "$InstallerDir\staging"
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    Copy-Item "$Target\zendesktop.exe" "$staging\ZenDesktop.exe" -Force
    Copy-Item "$Root\assets\icons\zendesktop.ico" "$staging\zendesktop.ico" -Force
    Copy-Item "$Root\LICENSE" "$staging\LICENSE" -Force
    Copy-Item "$Root\README.md" "$staging\README.md" -Force

    # Set version in .wxs
    $wxsContent = Get-Content "$InstallerDir\zendesktop.wxs" -Raw
    $wxsContent = $wxsContent -replace 'Version="[0-9.]+"', "Version=`"$Version`""
    Set-Content "$InstallerDir\zendesktop.wxs" -Value $wxsContent

    # Build MSI
    pushd "$InstallerDir"
    & $wixExe build -bf staging zendesktop.wxs -o "$ReleaseDir\ZenDesktop-v$Version-x64.msi"
    popd

    Remove-Item -Recurse -Force $staging
    Write-Host "  MSI built!" -ForegroundColor Green
} else {
    Write-Host "  WiX v4 not found. Install with:" -ForegroundColor Yellow
    Write-Host "    dotnet tool install --global wix" -ForegroundColor Gray
    Write-Host "  Skipping MSI." -ForegroundColor Gray
}

# 5. Done
Write-Host "[5/5] Done!" -ForegroundColor Green
Write-Host ""
Write-Host "Artifacts in: $ReleaseDir" -ForegroundColor White
Get-ChildItem $ReleaseDir | ForEach-Object {
    $sizeKB = '{0:N0}' -f ($_.Length/1KB)
    Write-Host "  $($_.Name)  ($sizeKB KB)"
}
