#Requires -Version 5.1
<#
.SYNOPSIS
    Uninstall OpenAlpaca from Windows.
.DESCRIPTION
    Removes OpenAlpaca binaries, GUI MSI, Start Menu shortcuts, and PATH entries.
    User data at %APPDATA%\OpenAlpaca\data\ is preserved.
.PARAMETER Prefix
    Where binaries were installed. Default: $env:LOCALAPPDATA\OpenAlpaca
.PARAMETER Yes
    Non-interactive: skip confirmation prompt.
#>
[CmdletBinding()]
param(
    [string]$Prefix,
    [switch]$Yes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $Prefix) {
    $Prefix = Join-Path $env:LOCALAPPDATA 'OpenAlpaca'
}

$binDir       = Join-Path $Prefix 'bin'
$dataDir      = Join-Path $env:APPDATA 'OpenAlpaca\data'
$startMenuDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\OpenAlpaca'

Write-Host "This will remove:"
Write-Host "  Binaries:       $Prefix"
Write-Host "  Start Menu:     $startMenuDir"
Write-Host "  PATH entry for: $binDir"
Write-Host "  GUI MSI (if installed)"
Write-Host ""
Write-Host "User data at $dataDir will NOT be removed."

if (-not $Yes) {
    $confirm = Read-Host "Proceed with uninstall? [y/N]"
    if ($confirm -notmatch '^[yY]') {
        Write-Host "Uninstall cancelled."
        exit 0
    }
}

# ── Stop running daemon ──────────────────────────────────────

$discoveryJson = Join-Path $dataDir 'discovery.json'
if (Test-Path $discoveryJson) {
    try {
        $discovery = Get-Content $discoveryJson -Raw | ConvertFrom-Json
        $pid = $discovery.pid
        if ($pid) {
            $proc = Get-Process -Id $pid -ErrorAction SilentlyContinue
            if ($proc) {
                Write-Host "Stopping running daemon (pid=$pid)..."
                Stop-Process -Id $pid -Force -ErrorAction SilentlyContinue
                $waited = 0
                while ($waited -lt 10) {
                    Start-Sleep -Seconds 1
                    $waited++
                    $proc = Get-Process -Id $pid -ErrorAction SilentlyContinue
                    if (-not $proc) { break }
                }
            }
        }
    } catch {
        # Ignore errors parsing discovery.json
    }
}

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
Write-Host "Note: User data remains at $dataDir"
Write-Host "      Remove it manually if you want a complete cleanup."
