#Requires -Version 5.1
<#
    Pester tests for the Windows release scripts.

    The two scripts are the only shipped code nothing else in CI can exercise —
    the Rust and bun gates run on ubuntu — so `.github/workflows/ci.yml` has one
    small `windows-latest` job that runs this file and nothing else. Run it
    locally on a Windows box with:

        Invoke-Pester -Path scripts/release/tests

    Both scripts are standalone (one copy of each ships per release archive), so
    each defines its own copy of the store-root helpers; the two Describe blocks
    below dot-source one script apiece and assert the copies agree.
#>

BeforeAll {
    $script:ReleaseDir      = Split-Path -Parent $PSScriptRoot
    $script:InstallScript   = Join-Path $script:ReleaseDir 'install-windows.ps1'
    $script:UninstallScript = Join-Path $script:ReleaseDir 'uninstall-windows.ps1'

    $script:SavedHomeStore   = [Environment]::GetEnvironmentVariable('OPENALPACA_HOME_STORE')
    $script:SavedUserProfile = $env:USERPROFILE

    $script:TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('openalpaca-pester-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $script:TempRoot | Out-Null

    function Write-Discovery {
        param([int]$ProcessId)
        $stateDir = Join-Path $script:TempRoot 'state'
        New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
        Set-Content -Path (Join-Path $stateDir 'discovery.json') -Value ('{"pid": ' + $ProcessId + ', "port": 7777}')
    }

    function Remove-Discovery {
        $file = Join-Path $script:TempRoot 'state\discovery.json'
        if (Test-Path $file) { Remove-Item $file -Force }
    }

    # A process that lives long enough to be found and (not) killed.
    function Start-Sleeper {
        param([string]$Exe)
        return (Start-Process -FilePath $Exe -ArgumentList '/c', 'ping -n 30 127.0.0.1' -PassThru -WindowStyle Hidden)
    }
}

AfterAll {
    if ($script:TempRoot -and (Test-Path $script:TempRoot)) {
        Remove-Item $script:TempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $script:SavedHomeStore)
    $env:USERPROFILE = $script:SavedUserProfile
}

Describe 'install-windows.ps1' {

    BeforeAll {
        . $script:InstallScript -DotSourceOnly
    }

    Context 'the store root' {

        BeforeEach {
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $null)
        }

        It 'defaults to the .openalpaca directory under USERPROFILE' {
            $env:USERPROFILE = $script:TempRoot
            Get-DataRoot | Should -Be (Join-Path $script:TempRoot '.openalpaca')
        }

        It 'is OPENALPACA_HOME_STORE when that names an absolute path' {
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $script:TempRoot)
            Get-DataRoot | Should -Be $script:TempRoot
        }

        It 'refuses a relative OPENALPACA_HOME_STORE' {
            # Die only reaches its `exit` when errors are non-terminating; the
            # script body runs with Stop, so assert the failure under Stop too.
            $ErrorActionPreference = 'Stop'
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', 'relative\path')
            { Get-DataRoot } | Should -Throw
        }
    }

    Context 'the daemon it stops before overwriting binaries' {

        BeforeEach {
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $script:TempRoot)
            Remove-Discovery
        }

        It 'reads discovery.json from state under the store root' {
            Get-DiscoveryPath | Should -Be (Join-Path $script:TempRoot 'state\discovery.json')
        }

        It 'stages config into config under the store root' {
            Get-ConfigDir | Should -Be (Join-Path $script:TempRoot 'config')
        }

        It 'finds no daemon when discovery.json is absent' {
            Get-RunningDaemonPid | Should -BeNullOrEmpty
            { Stop-RunningDaemon } | Should -Not -Throw
        }

        It 'finds no daemon when discovery.json names a pid nothing owns' {
            # Windows pids are multiples of 4, so 999999 can never be one.
            Write-Discovery -ProcessId 999999
            Get-RunningDaemonPid | Should -BeNullOrEmpty
        }

        It 'never stops a live process that is not openalpacad' {
            $other = Start-Sleeper -Exe $env:ComSpec
            try {
                Write-Discovery -ProcessId $other.Id
                Get-RunningDaemonPid | Should -BeNullOrEmpty
                Stop-RunningDaemon
                (Get-Process -Id $other.Id -ErrorAction SilentlyContinue) | Should -Not -BeNullOrEmpty
            } finally {
                Stop-Process -Id $other.Id -Force -ErrorAction SilentlyContinue
            }
        }

        It 'stops a live openalpacad' {
            $fake = Join-Path $script:TempRoot 'openalpacad.exe'
            Copy-Item $env:ComSpec $fake -Force
            $daemon = Start-Sleeper -Exe $fake
            try {
                $daemon.ProcessName | Should -Be 'openalpacad'
                Write-Discovery -ProcessId $daemon.Id
                Get-RunningDaemonPid | Should -Be $daemon.Id
                Stop-RunningDaemon
                (Get-Process -Id $daemon.Id -ErrorAction SilentlyContinue) | Should -BeNullOrEmpty
            } finally {
                Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
            }
        }
    }
}

Describe 'uninstall-windows.ps1' {

    BeforeAll {
        . $script:UninstallScript -DotSourceOnly
    }

    Context 'the store root it names and reads' {

        BeforeEach {
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $null)
            Remove-Discovery
        }

        It 'defaults to the .openalpaca directory under USERPROFILE' {
            $env:USERPROFILE = $script:TempRoot
            Get-DataRoot | Should -Be (Join-Path $script:TempRoot '.openalpaca')
        }

        It 'is OPENALPACA_HOME_STORE when that names an absolute path' {
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $script:TempRoot)
            Get-DataRoot | Should -Be $script:TempRoot
        }

        It 'reads discovery.json from state under the store root' {
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $script:TempRoot)
            Get-DiscoveryPath | Should -Be (Join-Path $script:TempRoot 'state\discovery.json')
        }

        It 'never stops a live process that is not openalpacad' {
            [Environment]::SetEnvironmentVariable('OPENALPACA_HOME_STORE', $script:TempRoot)
            $other = Start-Sleeper -Exe $env:ComSpec
            try {
                Write-Discovery -ProcessId $other.Id
                Get-RunningDaemonPid | Should -BeNullOrEmpty
                Stop-RunningDaemon
                (Get-Process -Id $other.Id -ErrorAction SilentlyContinue) | Should -Not -BeNullOrEmpty
            } finally {
                Stop-Process -Id $other.Id -Force -ErrorAction SilentlyContinue
            }
        }
    }
}

Describe 'both scripts, as text' {

    It 'install-windows.ps1 names the real store root and no legacy data root' {
        $text = Get-Content $script:InstallScript -Raw
        $text | Should -Not -Match 'OpenAlpaca\\data'
        $text | Should -Match '\.openalpaca'
        $text | Should -Match 'state\\discovery\.json'
    }

    It 'uninstall-windows.ps1 names the real store root and no legacy data root' {
        $text = Get-Content $script:UninstallScript -Raw
        $text | Should -Not -Match 'OpenAlpaca\\data'
        $text | Should -Match '\.openalpaca'
        $text | Should -Match 'state\\discovery\.json'
    }
}
