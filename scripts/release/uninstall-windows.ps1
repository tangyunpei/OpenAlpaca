#Requires -Version 5.1
<#
.SYNOPSIS
    Uninstall OpenAlpaca from Windows.
.DESCRIPTION
    Removes OpenAlpaca binaries, GUI MSI, Start Menu shortcuts, and PATH entries.
    User data at %USERPROFILE%\.openalpaca (or %OPENALPACA_HOME_STORE%) is preserved.
.PARAMETER Prefix
    Where binaries were installed. Default: $env:LOCALAPPDATA\OpenAlpaca
.PARAMETER Yes
    Non-interactive: skip confirmation prompt.
.PARAMETER DotSourceOnly
    Define this script's functions in the caller's scope and return without
    uninstalling anything. Used by scripts/release/tests/WindowsScripts.Tests.ps1.
#>
[CmdletBinding()]
param(
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
# `. .\uninstall-windows.ps1 -DotSourceOnly` defines the functions above in the
# caller's scope and removes nothing (scripts/release/tests).

if ($DotSourceOnly) { return }

# ── Defaults ─────────────────────────────────────────────────

if (-not $Prefix) {
    $Prefix = Join-Path $env:LOCALAPPDATA 'OpenAlpaca'
}

$binDir       = Join-Path $Prefix 'bin'
$dataRoot     = Get-DataRoot
$startMenuDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\OpenAlpaca'

Write-Host "This will remove:"
Write-Host "  Binaries:       $Prefix"
Write-Host "  Start Menu:     $startMenuDir"
Write-Host "  PATH entry for: $binDir"
Write-Host "  GUI MSI (if installed)"
Write-Host ""
Write-Host "User data at $dataRoot will NOT be removed."

if (-not $Yes) {
    $confirm = Read-Host "Proceed with uninstall? [y/N]"
    if ($confirm -notmatch '^[yY]') {
        Write-Host "Uninstall cancelled."
        exit 0
    }
}

# ── Stop running daemon ──────────────────────────────────────
# Dies rather than removing files out from under a daemon it could not stop.

Stop-RunningDaemon

# ── Uninstall MSI ────────────────────────────────────────────

$uninstallKeys = @(
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall'
)

foreach ($keyPath in $uninstallKeys) {
    if (-not (Test-Path $keyPath)) { continue }
    Get-ChildItem $keyPath -ErrorAction SilentlyContinue | ForEach-Object {
        $props = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
        if ($props.DisplayName -and $props.DisplayName -match 'openalpaca') {
            $uninstallString = $props.UninstallString
            if ($uninstallString -and $uninstallString -match 'msiexec') {
                # Extract product code
                if ($uninstallString -match '\{[0-9A-Fa-f\-]+\}') {
                    $productCode = $Matches[0]
                    Write-Host "Uninstalling GUI MSI (product code: $productCode)..."
                    $proc = Start-Process 'msiexec.exe' -ArgumentList @('/x', $productCode, '/qn', '/norestart') -Wait -PassThru
                    if ($proc.ExitCode -ne 0) {
                        Write-Host "Warning: MSI uninstall exited with code $($proc.ExitCode)."
                    }
                }
            }
        }
    }
}

# ── Remove binaries ──────────────────────────────────────────

if (Test-Path $Prefix) {
    Remove-Item $Prefix -Recurse -Force
    Write-Host "Removed $Prefix"
}

# ── Remove Start Menu shortcuts ──────────────────────────────

if (Test-Path $startMenuDir) {
    Remove-Item $startMenuDir -Recurse -Force
    Write-Host "Removed $startMenuDir"
}

# ── Remove from PATH ────────────────────────────────────────

$currentPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($currentPath -and $currentPath -like "*$binDir*") {
    $parts = $currentPath -split ';' | Where-Object { $_.TrimEnd('\') -ne $binDir.TrimEnd('\') }
    $newPath = ($parts -join ';').TrimEnd(';')
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "Removed $binDir from user PATH"
}

# ── Done ─────────────────────────────────────────────────────

Write-Host ""
Write-Host "OpenAlpaca has been uninstalled."
Write-Host "Note: User data remains at $dataRoot"
Write-Host "      Remove it manually if you want a complete cleanup."
