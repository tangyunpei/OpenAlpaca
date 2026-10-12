#Requires -Version 5.1
<#
.SYNOPSIS
    Build and package OpenAlpaca for Windows.
.DESCRIPTION
    Installs the GUI's JS dependencies from the frozen lockfile, builds the daemon
    once (prepare-sidecar.ts --release stages cargo's artifact as the Tauri sidecar
    and records its sha256), builds the CLI alone, builds the Tauri MSI, verifies
    the daemon inside the MSI against that sha256, and produces a distributable
    .zip archive whose libexec\openalpacad.exe is the same build.
.PARAMETER DotSourceOnly
    Define this script's functions in the caller's scope and return without
    building anything. Used by scripts/release/tests/WindowsScripts.Tests.ps1.
#>
[CmdletBinding()]
param(
    [switch]$DotSourceOnly
)

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

function Compute-SHA256($path) {
    return (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
}

# The hex digest of a `shasum -a 256`-format file (`<hex>  <name>`), as
# prepare-sidecar.ts writes it beside the staged sidecar.
function Read-Sha256File($path) {
    $digest = ((Get-Content $path -Raw).Trim() -split '\s+')[0].ToLower()
    if ($digest -notmatch '^[0-9a-f]{64}$') {
        Die "No sha256 digest in $path"
    }
    return $digest
}

# ── Dot-source guard ─────────────────────────────────────────
# `. .\package-windows.ps1 -DotSourceOnly` defines the functions above in the
# caller's scope and builds nothing (scripts/release/tests).

if ($DotSourceOnly) { return }

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

$guiDir        = Join-Path $RepoRoot 'apps\openalpaca-gui'
$stagedSidecar = Join-Path $guiDir "src-tauri\bin\openalpacad-$hostTarget.exe"
$stagedShaFile = "$stagedSidecar.sha256"

Write-Host "==> Installing JS dependencies (frozen lockfile)"
Push-Location $guiDir
try {
    bun install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { Die "bun install failed." }

    # One canonical daemon-only build: prepare-sidecar.ts builds openalpacad
    # through cargo, stages cargo's own artifact at src-tauri\bin\ and records
    # its sha256. That staged file is the sidecar Tauri bundles AND the
    # libexec\ daemon packaged below.
    Write-Host "==> Building and staging the daemon (release)"
    bun run scripts/prepare-sidecar.ts --release
    if ($LASTEXITCODE -ne 0) { Die "prepare-sidecar.ts failed." }
} finally {
    Pop-Location
}
if (-not (Test-Path $stagedSidecar)) { Die "Staged sidecar not found: $stagedSidecar" }
$stagedSha = Read-Sha256File $stagedShaFile
if ((Compute-SHA256 $stagedSidecar) -ne $stagedSha) { Die "Staged sidecar does not match $stagedShaFile" }

Write-Host "==> Building the CLI"
cargo build --release --locked -p openalpaca
if ($LASTEXITCODE -ne 0) { Die "Cargo build failed." }

# beforeBuildCommand runs prepare-sidecar.ts --release again; cargo finds the
# daemon fresh and re-stages the same bytes. The checks after the build hold
# the MSI to the hash recorded above, before `tauri build` ran.
Write-Host "==> Building Tauri app bundle"
Push-Location $guiDir
try {
    bunx tauri build --bundles msi
    if ($LASTEXITCODE -ne 0) { Die "Tauri build failed." }
} finally {
    Pop-Location
}

# Locate MSI
$msiPath = $null
# The shell builds into target\gui (src-tauri\.cargo\config.toml); the other
# two are where an older tree put the bundle.
$msiCandidates = @(
    (Join-Path $RepoRoot 'target\gui\release\bundle\msi'),
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

# The daemon inside the MSI: an administrative install (`msiexec /a`, no
# elevation, nothing registered) lays the MSI's files out under TARGETDIR.
Write-Host "==> Verifying the bundled daemon against the staged sidecar"
$extractDir = Join-Path $stagingBase 'msi-extract'
if (Test-Path $extractDir) { Remove-Item $extractDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
$msiexec = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/a', "`"$msiPath`"", '/qn', "TARGETDIR=`"$extractDir`"") -Wait -PassThru -NoNewWindow
if ($msiexec.ExitCode -ne 0) { Die "msiexec /a failed with exit code $($msiexec.ExitCode)." }
$bundledDaemon = Get-ChildItem $extractDir -Recurse -Filter 'openalpacad.exe' -File | Select-Object -First 1
if (-not $bundledDaemon) { Die "openalpacad.exe not found inside the MSI." }
$bundledSha = Compute-SHA256 $bundledDaemon.FullName
if ($bundledSha -ne $stagedSha) { Die "The daemon inside the MSI ($bundledSha) is not the staged sidecar ($stagedSha)." }
if ((Read-Sha256File $stagedShaFile) -ne $stagedSha) { Die "The staged sidecar changed while the MSI was built; rerun the script." }
Remove-Item $extractDir -Recurse -Force

$openalpacaBin = Join-Path $RepoRoot 'target\release\openalpaca.exe'
if (-not (Test-Path $openalpacaBin)) { Die "Binary not found: $openalpacaBin" }

Write-Host "==> Preparing staging directory"
if (Test-Path $stagingDir) { Remove-Item $stagingDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'bin')     | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'libexec') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'gui')     | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stagingDir 'config')  | Out-Null

Copy-Item $openalpacaBin (Join-Path $stagingDir 'bin\openalpaca.exe')
Copy-Item $stagedSidecar (Join-Path $stagingDir 'libexec\openalpacad.exe')
if ((Compute-SHA256 (Join-Path $stagingDir 'libexec\openalpacad.exe')) -ne $stagedSha) {
    Die "libexec\openalpacad.exe does not match the staged sidecar."
}
Copy-Item $msiPath (Join-Path $stagingDir 'gui\openalpaca-gui.msi')

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
