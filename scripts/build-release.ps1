# ZenDesktop Release Builder
# Creates: portable .exe + .zip + .msi installer + SHA256SUMS.txt
param(
    [string]$Version = "1.0.8",
    [string]$SigningKey = $env:SIGNING_KEY
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$Target = "$Root\target\release"
$ReleaseDir = "$Root\release\$Version"
$InstallerDir = "$Root\installer"

Write-Host "=== ZenDesktop Release Builder v$Version ===" -ForegroundColor Cyan

# 1. Build release
Write-Host "[1/6] Building release..." -ForegroundColor Yellow
cargo build --release --manifest-path "$Root\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

# 2. Prepare release directory
Write-Host "[2/6] Preparing release dir..." -ForegroundColor Yellow
New-Item -ItemType Directory -Force -Path $ReleaseDir | Out-Null

# 3. Copy and compress portable
Write-Host "[3/6] Creating portable artifacts..." -ForegroundColor Yellow

# Portable .exe (versioned + generic name)
Copy-Item "$Target\zendesktop.exe" "$ReleaseDir\ZenDesktop-v$Version.exe" -Force
Copy-Item "$Target\zendesktop.exe" "$ReleaseDir\ZenDesktop.exe" -Force

# Portable .zip (with docs)
$zipDir = "$ReleaseDir\portable-tmp"
New-Item -ItemType Directory -Force -Path $zipDir | Out-Null
Copy-Item "$Target\zendesktop.exe" "$zipDir\ZenDesktop.exe" -Force
Copy-Item "$Root\LICENSE" "$zipDir\" -Force
Copy-Item "$Root\README.md" "$zipDir\" -Force
Copy-Item "$Root\CHANGELOG.md" "$zipDir\" -Force
Compress-Archive -Path "$zipDir\*" `
    -DestinationPath "$ReleaseDir\ZenDesktop-v$Version-portable.zip" -Force
Remove-Item -Recurse -Force $zipDir

# 4. Sign portable EXE (Ed25519) if a key is provided
if ($SigningKey) {
    Write-Host "[4/6] Signing executable..." -ForegroundColor Yellow
    $env:SIGNING_KEY = $SigningKey
    cargo run --release --bin sign-release -- "$Target\zendesktop.exe"
    if (Test-Path "$Target\zendesktop.exe.sig") {
        Copy-Item "$Target\zendesktop.exe.sig" "$ReleaseDir\ZenDesktop.exe.sig" -Force
    }
} else {
    Write-Host "[4/6] No SIGNING_KEY set - skipping Ed25519 signature." -ForegroundColor Yellow
}

# 5. Build MSI installer with WiX v4
Write-Host "[5/6] Building MSI installer..." -ForegroundColor Yellow

$wixGlobal = "$env:USERPROFILE\.dotnet\tools\wix.exe"
$wixLocal = (Get-Command "wix.exe" -ErrorAction SilentlyContinue)

$wixExe = $null
if (Test-Path $wixGlobal) {
    $wixExe = $wixGlobal
} elseif ($wixLocal) {
    $wixExe = $wixLocal.Source
}

if ($wixExe) {
    $staging = "$InstallerDir\staging"
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    Copy-Item "$Target\zendesktop.exe" "$staging\ZenDesktop.exe" -Force
    Copy-Item "$Root\assets\icons\zendesktop.ico" "$staging\zendesktop.ico" -Force
    Copy-Item "$Root\LICENSE" "$staging\LICENSE" -Force
    Copy-Item "$Root\README.md" "$staging\README.md" -Force

    # Paths in the .wxs resolve relative to the .wxs file.
    # -arch x64 sets the MSI platform (WiX v5 rejects Platform on Package).
    & $wixExe build -arch x64 -d Version=$Version -d SrcDir=staging `
        "$InstallerDir\zendesktop.wxs" `
        -o "$ReleaseDir\ZenDesktop-v$Version-x64.msi"
    if ($LASTEXITCODE -ne 0) { throw "WiX build failed" }

    Remove-Item -Recurse -Force $staging
    Write-Host "  MSI built!" -ForegroundColor Green
} else {
    Write-Host "  WiX v4 not found. Install with:" -ForegroundColor Yellow
    Write-Host "    dotnet tool install --global wix" -ForegroundColor Gray
    Write-Host "  Skipping MSI." -ForegroundColor Gray
}

# 6. Build EXE installer with Inno Setup
Write-Host "[6/7] Building EXE installer (Inno Setup)..." -ForegroundColor Yellow

$iscc = $null
foreach ($path in @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 5\ISCC.exe"
)) {
    if (Test-Path $path) { $iscc = $path; break }
}

if ($iscc) {
    $staging = "$InstallerDir\staging"
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    Copy-Item "$Target\zendesktop.exe" "$staging\ZenDesktop.exe" -Force
    Copy-Item "$Root\assets\icons\zendesktop.ico" "$staging\zendesktop.ico" -Force
    Copy-Item "$Root\LICENSE" "$staging\LICENSE" -Force
    Copy-Item "$Root\README.md" "$staging\README.md" -Force

    & $iscc -DMyAppVersion=$Version "$InstallerDir\zendesktop.iss"
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup build failed" }

    Remove-Item -Recurse -Force $staging
    Write-Host "  EXE installer built!" -ForegroundColor Green
} else {
    Write-Host "  Inno Setup not found. Install from https://jrsoftware.org/isdl.php" -ForegroundColor Gray
    Write-Host "  Skipping EXE installer." -ForegroundColor Gray
}

# 7. Generate SHA256 checksums + done
Write-Host "[7/7] Generating checksums..." -ForegroundColor Yellow
Get-ChildItem -Path $ReleaseDir -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    "$hash  $($_.Name)"
} | Set-Content -Path "$ReleaseDir\SHA256SUMS.txt" -Encoding ascii

Write-Host "Done!" -ForegroundColor Green
Write-Host ""
Write-Host "Artifacts in: $ReleaseDir" -ForegroundColor White
Get-ChildItem $ReleaseDir | ForEach-Object {
    $sizeKB = '{0:N0}' -f ($_.Length/1KB)
    Write-Host "  $($_.Name)  ($sizeKB KB)"
}
