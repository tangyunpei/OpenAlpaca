#Requires -Version 5.1
<#
    Pester tests for the Windows release scripts.

    The installer and uninstaller are the only shipped code nothing else in CI
    can exercise — the Rust and bun gates run on ubuntu — so
    `.github/workflows/ci.yml` has one small `windows-latest` job that runs this
    file and nothing else. Run it locally on a Windows box with:

        Invoke-Pester -Path scripts/release/tests

    Both are standalone (one copy of each ships per release archive), so each
    defines its own copy of the store-root helpers; the first two Describe
    blocks below dot-source one script apiece and assert the copies agree. The
    third dot-sources package-windows.ps1 for the two digest helpers that hold
    the packaged daemon to the staged sidecar's sha256 and the two that hold
    the bun on PATH to the root package.json's pin; the packaging itself
    (cargo, bun, tauri, msiexec) runs nowhere in CI.
#>

BeforeAll {
    $script:ReleaseDir      = Split-Path -Parent $PSScriptRoot
    $script:InstallScript   = Join-Path $script:ReleaseDir 'install-windows.ps1'
    $script:UninstallScript = Join-Path $script:ReleaseDir 'uninstall-windows.ps1'
    $script:PackageScript   = Join-Path $script:ReleaseDir 'package-windows.ps1'

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

Describe 'package-windows.ps1' {

    BeforeAll {
        . $script:PackageScript -DotSourceOnly
    }

    Context 'the staged sidecar digest it holds the packaged daemon to' {

        It 'reads the hex digest from a shasum-format .sha256 file' {
            $file = Join-Path $script:TempRoot 'staged.sha256'
            Set-Content -Path $file -Value ("$('a' * 64)  openalpacad-x86_64-pc-windows-msvc.exe")
            Read-Sha256File $file | Should -Be ('a' * 64)
        }

        It 'lower-cases a digest written in upper case' {
            $file = Join-Path $script:TempRoot 'upper.sha256'
            Set-Content -Path $file -Value ("$('B' * 64)  openalpacad.exe")
            Read-Sha256File $file | Should -Be ('b' * 64)
        }

        It 'refuses a .sha256 file that holds no 64-hex digest' {
            # As in the installer's relative-path test: Die reaches its `exit`
            # only when errors are non-terminating, so assert under Stop.
            $ErrorActionPreference = 'Stop'
            $file = Join-Path $script:TempRoot 'bad.sha256'
            Set-Content -Path $file -Value 'not a digest'
            { Read-Sha256File $file } | Should -Throw
        }

        It 'hashes a file the way shasum -a 256 does' {
            $file = Join-Path $script:TempRoot 'abc.bin'
            [System.IO.File]::WriteAllBytes($file, [byte[]](0x61, 0x62, 0x63))
            Compute-SHA256 $file | Should -Be 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
        }
    }

    Context 'the Bun it requires: the version the root package.json pins' {

        It 'reads the pinned version from a packageManager field' {
            $file = Join-Path $script:TempRoot 'package.json'
            Set-Content -Path $file -Value '{ "name": "x", "packageManager": "bun@1.4.3" }'
            Read-PinnedBunVersion $file | Should -Be '1.4.3'
        }

        It 'reads a version from the repository root package.json' {
            $root = Join-Path (Split-Path -Parent (Split-Path -Parent $script:ReleaseDir)) 'package.json'
            Read-PinnedBunVersion $root | Should -Match '^\d+\.\d+\.\d+$'
        }

        It 'refuses a package.json that pins no bun' {
            $ErrorActionPreference = 'Stop'
            $file = Join-Path $script:TempRoot 'no-pin.json'
            Set-Content -Path $file -Value '{ "name": "x" }'
            { Read-PinnedBunVersion $file } | Should -Throw
        }

        It 'accepts the pinned version' {
            { Assert-BunVersion '1.4.3' '1.4.3' } | Should -Not -Throw
        }

        It 'refuses any other version, naming both' {
            $ErrorActionPreference = 'Stop'
            { Assert-BunVersion '1.4.2' '1.4.3' } | Should -Throw -ExpectedMessage '*bun 1.4.2*bun@1.4.3*'
        }
    }
}

Describe 'the scripts, as text' {

    It 'package-windows.ps1 builds the daemon once through prepare-sidecar.ts, with a frozen install and locked builds' {
        $text = Get-Content $script:PackageScript -Raw
        $text | Should -Match 'prepare-sidecar\.ts --release'
        $text | Should -Match 'bun install --frozen-lockfile'
        $text | Should -Match 'cargo build --release --locked -p openalpaca'
        $text | Should -Not -Match '-p openalpaca -p openalpacad'
    }

    It 'package-windows.ps1 holds the bun on PATH to the root package.json pin' {
        $text = Get-Content $script:PackageScript -Raw
        $text | Should -Match 'Assert-BunVersion \(\(bun --version \| Out-String\)\.Trim\(\)\)'
        $text | Should -Match 'Read-PinnedBunVersion \(Join-Path \$RepoRoot ''package\.json''\)'
    }

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
