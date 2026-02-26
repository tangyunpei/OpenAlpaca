#Requires -Version 5.1
<#
.SYNOPSIS
    Build and package OpenAlpaca for Windows.
.DESCRIPTION
    Compiles Rust binaries, builds Tauri MSI, and produces a distributable .zip archive.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Die($msg) {
    Write-Error "ERROR: $msg"
    exit 1
}

function Require-Cmd($name) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        Die "Required command not found: $name"
    }
}

if ($env:OS -ne 'Windows_NT') {
    Die "package-windows.ps1 must run on Windows."
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../..')).Path
$DistDir   = Join-Path $RepoRoot 'dist'

Require-Cmd 'cargo'
Require-Cmd 'rustc'
Require-Cmd 'bun'
Require-Cmd 'git'

$hostTarget = (rustc -vV | Select-String '^host:').ToString() -replace '^host:\s*', ''
switch ($hostTarget) {
    'x86_64-pc-windows-msvc'  { }
    'aarch64-pc-windows-msvc' { }
    default { Die "Unsupported host target for Windows packaging: $hostTarget" }
}

$versionRaw = cargo pkgid -p openalpaca 2>$null
$version = ($versionRaw -replace '.*#', '').Trim()
if (-not $version) { Die "Failed to resolve package version." }

$gitSha = try { (git -C $RepoRoot rev-parse --short HEAD 2>$null).Trim() } catch { 'unknown' }
$builtAtUtc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$packageName = "openalpaca-windows-${hostTarget}-v${version}"
$stagingBase = Join-Path $RepoRoot 'target\release-package'
$stagingDir  = Join-Path $stagingBase $packageName
$archivePath = Join-Path $DistDir "$packageName.zip"
$shaPath     = "$archivePath.sha256"

Write-Host "==> Building Rust binaries"
cargo build --release -p openalpaca -p openalpacad
if ($LASTEXITCODE -ne 0) { Die "Cargo build failed." }

Write-Host "==> Building Tauri app bundle"
Push-Location (Join-Path $RepoRoot 'apps\openalpaca-gui')
try {
    bun install
    bunx tauri build --bundles msi
    if ($LASTEXITCODE -ne 0) { Die "Tauri build failed." }
} finally {
    Pop-Location
}

# Locate MSI
$msiPath = $null
$msiCandidates = @(
    (Join-Path $RepoRoot 'target\release\bundle\msi'),
    (Join-Path $RepoRoot 'apps\openalpaca-gui\src-tauri\target\release\bundle\msi')
)
foreach ($dir in $msiCandidates) {
    if (Test-Path $dir) {
        $found = Get-ChildItem $dir -Filter 'openalpaca-gui_*.msi' -File | Select-Object -First 1
        if ($found) { $msiPath = $found.FullName; break }
    }
}
if (-not $msiPath) { Die "Tauri MSI not found." }

$openalpacaBin  = Join-Path $RepoRoot 'target\release\openalpaca.exe'
$openalpacadBin = Join-Path $RepoRoot 'target\release\openalpacad.exe'
if (-not (Test-Path $openalpacaBin))  { Die "Binary not found: $openalpacaBin" }
if (-not (Test-Path $openalpacadBin)) { Die "Binary not found: $openalpacadBin" }

Write-Host "==> Preparing staging directory"
if (Test-Path $stagingDir) { Remove-Item $stagingDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'bin')     | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'libexec') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'gui')     | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'config')  | Out-Null

Copy-Item $openalpacaBin  (Join-Path $stagingDir 'bin\openalpaca.exe')
Copy-Item $openalpacadBin (Join-Path $stagingDir 'libexec\openalpacad.exe')
Copy-Item $msiPath        (Join-Path $stagingDir 'gui\openalpaca-gui.msi')

# Never package repository runtime config directly (it may contain secrets).
$templateConfig = Join-Path $RepoRoot 'scripts\release\templates\config'
if (Test-Path $templateConfig) {
    Copy-Item "$templateConfig\*" (Join-Path $stagingDir 'config') -Recurse -Force
}

Copy-Item (Join-Path $RepoRoot 'scripts\release\install-windows.ps1')   (Join-Path $stagingDir 'install-windows.ps1')
Copy-Item (Join-Path $RepoRoot 'scripts\release\uninstall-windows.ps1') (Join-Path $stagingDir 'uninstall-windows.ps1')

$manifest = @{
    name         = 'openalpaca'
    version      = $version
    target       = $hostTarget
    built_at_utc = $builtAtUtc
    git_sha      = $gitSha
} | ConvertTo-Json -Depth 2
Set-Content -Path (Join-Path $stagingDir 'manifest.json') -Value $manifest -Encoding UTF8

Write-Host "==> Packaging archive"
if (-not (Test-Path $DistDir)) { New-Item -ItemType Directory -Force -Path $DistDir | Out-Null }
if (Test-Path $archivePath) { Remove-Item $archivePath -Force }
Compress-Archive -Path $stagingDir -DestinationPath $archivePath

$hash = (Get-FileHash $archivePath -Algorithm SHA256).Hash.ToLower()
$archiveBaseName = Split-Path $archivePath -Leaf
Set-Content -Path $shaPath -Value "$hash  $archiveBaseName" -Encoding UTF8

Write-Host "Done."
Write-Host "Archive: $archivePath"
Write-Host "SHA256 : $shaPath"
