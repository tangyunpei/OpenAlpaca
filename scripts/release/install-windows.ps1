#Requires -Version 5.1
<#
.SYNOPSIS
    Install OpenAlpaca on Windows.
.DESCRIPTION
    Installs OpenAlpaca from a local .zip archive or a remote URL.
.PARAMETER File
    Path to a local .zip archive.
.PARAMETER Url
    URL of a remote .zip archive.
.PARAMETER Prefix
    Installation directory. Default: $env:LOCALAPPDATA\OpenAlpaca
.PARAMETER Yes
    Non-interactive mode: skip confirmation prompts.
.PARAMETER DotSourceOnly
    Define this script's functions in the caller's scope and return without
    installing anything. Used by scripts/release/tests/WindowsScripts.Tests.ps1.
#>
[CmdletBinding()]
param(
    [string]$File,
    [string]$Url,
    [string]$Prefix,
    [switch]$Yes,
    [switch]$DotSourceOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Die($msg) {
    Write-Error "ERROR: $msg"
    exit 1
}

function Read-ManifestValue {
    param([string]$Key, [string]$ManifestPath)
    $json = Get-Content $ManifestPath -Raw | ConvertFrom-Json
    $val = $json.$Key
    if (-not $val) { Die "Could not read '$Key' from manifest: $ManifestPath" }
    return $val
}

function Copy-MissingTree {
    param([string]$Src, [string]$Dst)
    if (-not (Test-Path $Src)) { return }
    if (-not (Test-Path $Dst)) { New-Item -ItemType Directory -Force -Path $Dst | Out-Null }
    Get-ChildItem $Src -Recurse | ForEach-Object {
        $rel = $_.FullName.Substring($Src.TrimEnd('\').Length + 1)
        $target = Join-Path $Dst $rel
        if ($_.PSIsContainer) {
            if (-not (Test-Path $target)) {
                New-Item -ItemType Directory -Force -Path $target | Out-Null
            }
        } else {
            if (-not (Test-Path $target)) {
                $parentDir = Split-Path $target -Parent
                if (-not (Test-Path $parentDir)) {
                    New-Item -ItemType Directory -Force -Path $parentDir | Out-Null
                }
                Copy-Item $_.FullName $target
            }
        }
    }
}

function Get-HostTarget {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        'X64'   { return 'x86_64-pc-windows-msvc' }
        'Arm64' { return 'aarch64-pc-windows-msvc' }
        default { Die "Unsupported Windows architecture: $arch" }
    }
}

function Compute-SHA256($path) {
    return (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
}

# The runtime data/config root: `%OPENALPACA_HOME_STORE%` when set, else
# `<home>\.openalpaca` — the same root the daemon computes
# (`crates/openalpaca_storage/src/store/mod.rs::home_root()`), which does not
# follow the OS's data-directory convention on any platform.
function Get-DataRoot {
    $override = [Environment]::GetEnvironmentVariable('OPENALPACA_HOME_STORE')
    if ($override) {
        if (-not [System.IO.Path]::IsPathRooted($override)) {
            Die "OPENALPACA_HOME_STORE must be an absolute path, got '$override'"
        }
        return $override
    }
    return (Join-Path $env:USERPROFILE '.openalpaca')
}

# `<root>\config` — the config directory the daemon reads.
function Get-ConfigDir {
    return (Join-Path (Get-DataRoot) 'config')
}

# `<root>\state\discovery.json` — where a running daemon publishes its pid.
function Get-DiscoveryPath {
    return (Join-Path (Get-DataRoot) 'state\discovery.json')
}

# The pid from discovery.json, but only when it belongs to a live `openalpacad`
# process: a stale file naming a pid the OS has since handed to something else
# must never get that something else killed.
function Get-RunningDaemonPid {
    $discoveryJson = Get-DiscoveryPath
    if (-not (Test-Path $discoveryJson)) { return $null }

    $daemonPid = $null
    try {
        $discovery = Get-Content $discoveryJson -Raw | ConvertFrom-Json
        $daemonPid = $discovery.pid
    } catch {
        Write-Host "Warning: no readable pid in $discoveryJson; assuming no daemon is running."
        return $null
    }
    if (-not $daemonPid) { return $null }

    $proc = Get-Process -Id $daemonPid -ErrorAction SilentlyContinue
    if (-not $proc) { return $null }
    if ($proc.ProcessName -ne 'openalpacad') {
        Write-Host "Warning: $discoveryJson names pid $daemonPid, which is '$($proc.ProcessName)', not openalpacad. Leaving it alone."
        return $null
    }
    return [int]$daemonPid
}

function Stop-RunningDaemon {
    $daemonPid = Get-RunningDaemonPid
    if (-not $daemonPid) { return }

    Write-Host "Stopping running daemon (pid=$daemonPid)..."
    Stop-Process -Id $daemonPid -Force -ErrorAction SilentlyContinue
    $waited = 0
    while ($waited -lt 10) {
        Start-Sleep -Seconds 1
        $waited++
        if (-not (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue)) { return }
    }
    Die "The OpenAlpaca daemon (pid=$daemonPid) is still running after 10s. Stop it, then run this script again."
}

# ── Dot-source guard ─────────────────────────────────────────
# `. .\install-windows.ps1 -DotSourceOnly` defines the functions above in the
# caller's scope and installs nothing (scripts/release/tests).

if ($DotSourceOnly) { return }

# ── Defaults ──────────────────────────────────────────────────

if (-not $Prefix) {
    $Prefix = Join-Path $env:LOCALAPPDATA 'OpenAlpaca'
}

# ── Validate args ─────────────────────────────────────────────

if (-not $File -and -not $Url) {
    Write-Host @"
Install OpenAlpaca on Windows.

Usage:
  install-windows.ps1 -File <archive.zip> [-Prefix <dir>] [-Yes]
  install-windows.ps1 -Url  <https://.../archive.zip> [-Prefix <dir>] [-Yes]

Options:
  -File <path>    Install from a local .zip archive.
  -Url <url>      Install from a remote .zip URL.
  -Prefix <dir>   Installation directory. Default: %LOCALAPPDATA%\OpenAlpaca
  -Yes            Non-interactive: skip confirmation prompts.
"@
    exit 1
}

if ($File -and $Url) {
    Die "Use only one source: -File or -Url."
}

if ($File -and -not (Test-Path $File)) {
    Die "Archive not found: $File"
}

# ── Temporary directory ──────────────────────────────────────

$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "openalpaca-install-$([guid]::NewGuid().ToString('N').Substring(0,8))"
New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null
try {

$archivePath = Join-Path $tmpDir 'openalpaca.zip'
$shaFilePath = Join-Path $tmpDir 'openalpaca.zip.sha256'

if ($Url) {
    Write-Host "Downloading archive from $Url ..."
    Invoke-WebRequest -Uri $Url -OutFile $archivePath -UseBasicParsing
    try {
        Invoke-WebRequest -Uri "$Url.sha256" -OutFile $shaFilePath -UseBasicParsing
    } catch {
        if (Test-Path $shaFilePath) { Remove-Item $shaFilePath }
    }
} else {
    Copy-Item $File $archivePath
    $shaSource = "$File.sha256"
    if (Test-Path $shaSource) { Copy-Item $shaSource $shaFilePath }
}

# Verify checksum
if (Test-Path $shaFilePath) {
    $expected = (Get-Content $shaFilePath -Raw).Trim().Split(' ')[0].Split("`t")[0]
    if ($expected) {
        $actual = Compute-SHA256 $archivePath
        if ($actual -ne $expected) {
            Die "SHA256 mismatch. expected=$expected actual=$actual"
        }
        Write-Host "Verified SHA256 checksum."
    }
}

Write-Host "Extracting archive ..."
$extractDir = Join-Path $tmpDir 'extracted'
Expand-Archive -Path $archivePath -DestinationPath $extractDir -Force

# Find the package root (may be nested one level)
$packageRoot = $extractDir
$manifestPath = Join-Path $packageRoot 'manifest.json'
if (-not (Test-Path $manifestPath)) {
    $subDirs = Get-ChildItem $extractDir -Directory | Select-Object -First 1
    if ($subDirs) {
        $packageRoot = $subDirs.FullName
        $manifestPath = Join-Path $packageRoot 'manifest.json'
    }
}
if (-not (Test-Path $manifestPath)) { Die "manifest.json not found in archive." }

$manifestTarget  = Read-ManifestValue 'target' $manifestPath
$manifestVersion = Read-ManifestValue 'version' $manifestPath
$hostTarget      = Get-HostTarget

if ($manifestTarget -ne $hostTarget) {
    Die "Target mismatch: host=$hostTarget package=$manifestTarget"
}

# ── Confirm overwrite ────────────────────────────────────────

$binPath = Join-Path $Prefix 'bin\openalpaca.exe'
if ((Test-Path $binPath) -and -not $Yes) {
    $confirm = Read-Host "Existing installation detected. Overwrite binaries? [y/N]"
    if ($confirm -notmatch '^[yY]') {
        Write-Host "Installation cancelled."
        exit 0
    }
}

# ── Stop daemon ──────────────────────────────────────────────

Stop-RunningDaemon

# ── Install binaries ─────────────────────────────────────────

Write-Host "Installing binaries to $Prefix ..."
$binDir    = Join-Path $Prefix 'bin'
$libexecDir = Join-Path $Prefix 'libexec'
New-Item -ItemType Directory -Force -Path $binDir     | Out-Null
New-Item -ItemType Directory -Force -Path $libexecDir | Out-Null

Copy-Item (Join-Path $packageRoot 'bin\openalpaca.exe')      (Join-Path $binDir 'openalpaca.exe')    -Force
Copy-Item (Join-Path $packageRoot 'libexec\openalpacad.exe') (Join-Path $libexecDir 'openalpacad.exe') -Force

# ── Install GUI (MSI) ────────────────────────────────────────

$msiSource = Join-Path $packageRoot 'gui\openalpaca-gui.msi'
if (Test-Path $msiSource) {
    Write-Host "Installing GUI (MSI) ..."
    $msiArgs = @('/i', $msiSource, '/qn', '/norestart')
    $proc = Start-Process 'msiexec.exe' -ArgumentList $msiArgs -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        Write-Host "Warning: MSI install exited with code $($proc.ExitCode). You may need to install the GUI manually."
    }
}

# ── Install config ───────────────────────────────────────────

$configDir = Get-ConfigDir
New-Item -ItemType Directory -Force -Path $configDir | Out-Null
Copy-MissingTree (Join-Path $packageRoot 'config') $configDir

# ── Add to PATH ──────────────────────────────────────────────

$currentPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($currentPath -notlike "*$binDir*") {
    Write-Host "Adding $binDir to user PATH ..."
    $newPath = "$binDir;$currentPath"
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
}

# ── Start Menu shortcut ─────────────────────────────────────

$startMenuDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\OpenAlpaca'
try {
    New-Item -ItemType Directory -Force -Path $startMenuDir | Out-Null
    $wshell = New-Object -ComObject WScript.Shell
    $shortcut = $wshell.CreateShortcut((Join-Path $startMenuDir 'OpenAlpaca CLI.lnk'))
    $shortcut.TargetPath = Join-Path $binDir 'openalpaca.exe'
    $shortcut.WorkingDirectory = $env:USERPROFILE
    $shortcut.Description = 'OpenAlpaca CLI'
    $shortcut.Save()
} catch {
    Write-Host "Warning: Could not create Start Menu shortcut."
}

# ── Done ─────────────────────────────────────────────────────

Write-Host ""
Write-Host "OpenAlpaca $manifestVersion installed successfully."
Write-Host "CLI:    $binDir\openalpaca.exe"
Write-Host "Config: $configDir"
Write-Host "Restart your terminal for PATH changes to take effect."

} finally {
    Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
}
