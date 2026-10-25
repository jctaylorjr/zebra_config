<#
.SYNOPSIS
    Builds Zebra Configure on this PC from its source code.

.DESCRIPTION
    Run it in the source folder (the one with package.json), for example the folder GitHub's
    "Download ZIP" gives you. Run anywhere else, it downloads the source from GitHub first, into a
    folder next to the script.

    It installs what the build needs, only if it isn't there already:
      - Visual Studio Build Tools with the C++ tools, for every user, so Windows asks for admin
      - Rust, through rustup, into your user profile
      - Bun, into your user profile
    then builds the app into dist\Zebra Configure.exe, which runs on its own. The build's files go
    in %LOCALAPPDATA%\zebra-configure-build, unless CARGO_TARGET_DIR says otherwise; every copy
    of the source shares it, so building a newer download only rebuilds what changed.

.PARAMETER Installer
    Also build the setup program, which installs the app for the current user with a Start menu
    entry. The build downloads the tools it needs for that.

.PARAMETER Branch
    The branch to download when the script isn't in a source folder.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\build-windows.ps1

.EXAMPLE
    .\build.cmd -Installer
#>
[CmdletBinding()]
param(
    [switch]$Installer,
    [string]$Branch = 'main'
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest is many times slower while it draws a progress bar
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
# a proxy that asks who you are gets the signed-in user
if ([Net.WebRequest]::DefaultWebProxy) {
    [Net.WebRequest]::DefaultWebProxy.Credentials = [Net.CredentialCache]::DefaultNetworkCredentials
}

$repo = 'jctaylorjr/zebra_config'
$toolchain = 'stable-x86_64-pc-windows-msvc'
$minRust = [version]'1.90'  # the oldest the Tauri crates in Cargo.lock build with
$bunVersion = '1.3.14'      # the one bun.lock was made with
$minBun = [version]'1.2'    # the first that reads a text bun.lock

function Write-Step([string]$Text) {
    Write-Host ''
    Write-Host "==> $Text" -ForegroundColor Cyan
}

# downloads to $File and checks it starts as it should ("MZ" for a program, "PK" for a zip),
# since a blocked download often comes back as a web page instead
function Get-File([string]$Url, [string]$File, [string]$Magic) {
    Write-Host "    downloading $Url"
    try {
        Invoke-WebRequest -Uri $Url -OutFile $File -UseBasicParsing
    } catch {
        throw "Couldn't download $Url ($($_.Exception.Message))"
    }
    $head = New-Object byte[] 2
    $stream = [IO.File]::OpenRead($File)
    try { [void]$stream.Read($head, 0, 2) } finally { $stream.Close() }
    if ([Text.Encoding]::ASCII.GetString($head) -ne $Magic) {
        throw "$Url didn't download as the file it should be; a proxy or web filter may be blocking it"
    }
}

# runs a program and stops if it fails. its error output isn't redirected, since Windows
# PowerShell would turn every line of it (cargo's progress, for one) into an error
function Invoke-Native([string]$File, [string[]]$Arguments) {
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$([IO.Path]::GetFileName($File)) $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

# on PATH, or else where its installer puts it
function Find-Tool([string]$Name, [string]$Default) {
    $found = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($found) { return $found.Source }
    if (Test-Path $Default) { return $Default }
    return $null
}

function Test-Source([string]$Dir) {
    $Dir -and (Test-Path (Join-Path $Dir 'package.json')) -and
        (Test-Path (Join-Path $Dir 'src-tauri\tauri.conf.json'))
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'

# the newest Visual Studio, of any edition, with the C++ tools
function Find-VCTools {
    if (-not (Test-Path $vswhere)) { return $null }
    & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}

# the Windows SDK's libraries, which Rust links against
function Test-WindowsSdk {
    $lib = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Lib'
    [bool](Get-ChildItem $lib -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path (Join-Path $_.FullName 'um\x64\kernel32.lib') })
}

function Get-RustVersion([string]$Rustup) {
    $text = & $Rustup run $toolchain rustc --version
    if ($LASTEXITCODE -eq 0 -and "$text" -match 'rustc (\d+\.\d+\.\d+)') { return [version]$Matches[1] }
    return $null
}

function Get-BunVersion([string]$Bun) {
    $text = & $Bun --version
    if ($LASTEXITCODE -eq 0 -and "$text" -match '^(\d+\.\d+\.\d+)') { return [version]$Matches[1] }
    return $null
}

$started = Get-Date
try {
    if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
        throw "This builds the 64-bit x86 version of the app, and this PC is $env:PROCESSOR_ARCHITECTURE"
    }

    # the source: next to this script, or in the current folder, or else downloaded
    $here = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).Path }
    $root = @($here, (Get-Location).Path) | Where-Object { Test-Source $_ } | Select-Object -First 1
    if (-not $root) {
        # a GitHub zip holds one folder named after the repository and the branch
        $folder = 'zebra_config-' + ($Branch -replace '/', '-')
        $root = Join-Path $here $folder
        Write-Step "Downloading the source ($Branch) from GitHub"
        $zip = Join-Path $env:TEMP "$folder.zip"
        Get-File "https://github.com/$repo/archive/refs/heads/$Branch.zip" $zip 'PK'
        # over an earlier download, keeping its packages and build output so rebuilding is quicker
        Expand-Archive -Path $zip -DestinationPath $here -Force
        Remove-Item $zip
        if (-not (Test-Source $root)) { throw "The download didn't have the source in $folder" }
    }
    Write-Host "Source: $root"
    # the frontend build compares paths with where they really lead, which differ on a network drive
    $network = $root.StartsWith('\\') -or
        ([IO.DriveInfo]::new([IO.Path]::GetPathRoot($root)).DriveType -eq 'Network')
    if ($network) {
        Write-Warning 'The source is on a network drive, where the build can fail. If it does, extract it to a folder on this PC and run this there.'
    }

    Write-Step 'Visual Studio C++ build tools'
    $vs = Find-VCTools
    $restart = $false
    if ($vs -and (Test-WindowsSdk)) {
        Write-Host "    found in $vs"
    } else {
        Write-Host '    not found, so installing Visual Studio Build Tools with the C++ workload.'
        Write-Host '    Windows asks for permission, since they install for every user. This takes 10-20 minutes.'
        $setup = Join-Path $env:TEMP 'vs_BuildTools.exe'
        $got = $false
        foreach ($url in 'https://aka.ms/vs/stable/vs_BuildTools.exe', 'https://aka.ms/vs/17/release/vs_BuildTools.exe') {
            try {
                Get-File $url $setup 'MZ'
                $got = $true
                break
            } catch {
                Write-Warning $_.Exception.Message
            }
        }
        if (-not $got) {
            throw "Couldn't download Visual Studio Build Tools. Install them with the 'Desktop development with C++' workload from https://visualstudio.microsoft.com/visual-cpp-build-tools/, or ask IT to, then run this again"
        }
        $arguments = '--passive --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
        try {
            $p = Start-Process -FilePath $setup -ArgumentList $arguments -Verb RunAs -Wait -PassThru
        } catch {
            throw "Visual Studio Build Tools need admin rights to install. Ask IT to install them with the 'Desktop development with C++' workload, then run this again"
        }
        Remove-Item $setup -ErrorAction SilentlyContinue
        # 3010 means installed, with a restart wanted
        if ($p.ExitCode -ne 0 -and $p.ExitCode -ne 3010) {
            throw "The Visual Studio Build Tools installer failed with exit code $($p.ExitCode)"
        }
        $restart = $p.ExitCode -eq 3010
        if (-not (Find-VCTools) -or -not (Test-WindowsSdk)) {
            throw "Visual Studio Build Tools installed, but the C++ tools or the Windows SDK still aren't there"
        }
        Write-Host "    installed in $(Find-VCTools)"
    }

    Write-Step 'Rust'
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $rustup = Find-Tool 'rustup.exe' (Join-Path $cargoHome 'bin\rustup.exe')
    if (-not $rustup) {
        Write-Host "    not found, so installing it into $cargoHome"
        $init = Join-Path $env:TEMP 'rustup-init.exe'
        Get-File 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' $init 'MZ'
        # PATH is left as it is: this script finds Rust where rustup puts it
        Invoke-Native $init @('-y', '--no-modify-path', '--profile', 'minimal',
            '--default-host', 'x86_64-pc-windows-msvc', '--default-toolchain', 'stable')
        Remove-Item $init
        $rustup = Join-Path $cargoHome 'bin\rustup.exe'
    }
    # the build uses this toolchain, whichever one rustup defaults to
    $env:RUSTUP_TOOLCHAIN = $toolchain
    $rust = Get-RustVersion $rustup
    if (-not $rust -or $rust -lt $minRust) {
        Write-Host "    getting the latest $toolchain"
        Invoke-Native $rustup @('toolchain', 'install', $toolchain, '--profile', 'minimal')
        $rust = Get-RustVersion $rustup
        if (-not $rust -or $rust -lt $minRust) {
            throw "The build needs Rust $minRust or later, and $toolchain is $rust"
        }
    }
    Write-Host "    rustc $rust"

    Write-Step 'Bun'
    $bunHome = if ($env:BUN_INSTALL) { $env:BUN_INSTALL } else { Join-Path $env:USERPROFILE '.bun' }
    $bun = Find-Tool 'bun.exe' (Join-Path $bunHome 'bin\bun.exe')
    $bunNow = if ($bun) { Get-BunVersion $bun } else { $null }
    if (-not $bunNow -or $bunNow -lt $minBun) {
        $bin = Join-Path $bunHome 'bin'
        Write-Host "    installing Bun $bunVersion into $bin"
        $zip = Join-Path $env:TEMP "bun-$bunVersion.zip"
        # the baseline build runs on any 64-bit x86 processor
        Get-File "https://github.com/oven-sh/bun/releases/download/bun-v$bunVersion/bun-windows-x64-baseline.zip" $zip 'PK'
        $unpacked = Join-Path $env:TEMP "bun-$bunVersion"
        if (Test-Path $unpacked) { Remove-Item $unpacked -Recurse -Force }
        Expand-Archive -Path $zip -DestinationPath $unpacked
        New-Item -ItemType Directory -Force $bin | Out-Null
        Get-ChildItem $unpacked -Recurse -Filter 'bun.exe' | Select-Object -First 1 |
            Copy-Item -Destination $bin -Force
        Remove-Item $zip
        Remove-Item $unpacked -Recurse -Force
        $bun = Join-Path $bin 'bun.exe'
        $bunNow = Get-BunVersion $bun
        if (-not $bunNow) { throw "Bun didn't run after installing it in $bin" }
    }
    Write-Host "    bun $bunNow"

    $webview = 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    if (-not ($webview | Where-Object { (Get-ItemProperty $_ -ErrorAction SilentlyContinue).pv })) {
        Write-Warning "The WebView2 Runtime isn't installed, and the app needs it to open. Windows 11 has it; for Windows 10 see https://developer.microsoft.com/microsoft-edge/webview2/"
    }

    Write-Step 'Building the app. The first build takes a while.'
    # cargo's build folder goes several folders deep, and the linker can't open a path over 260
    # characters unless Windows has long paths turned on, so it's kept short rather than in the
    # source. every copy of the source shares it, so a newer download only rebuilds what changed
    if (-not $env:CARGO_TARGET_DIR) {
        $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'zebra-configure-build'
    }
    Write-Host "    build folder: $env:CARGO_TARGET_DIR"
    $built = Join-Path $env:CARGO_TARGET_DIR 'release\zebra_configure.exe'
    $dist = Join-Path $root 'dist'
    $app = Join-Path $dist 'Zebra Configure.exe'
    # a running copy can't be replaced
    if (Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $built -or $_.Path -eq $app }) {
        throw "Zebra Configure is running from $dist or the build folder. Close it, from the notification area too, and run this again"
    }
    # the build runs cargo and bun by name
    $env:Path = "$(Split-Path $bun);$(Split-Path $rustup);$env:Path"
    Push-Location $root
    try {
        Invoke-Native $bun @('install', '--frozen-lockfile')
        $build = @('run', 'tauri', 'build')
        if ($Installer) { $build += @('--bundles', 'nsis') } else { $build += '--no-bundle' }
        Invoke-Native $bun $build
    } finally {
        Pop-Location
    }

    New-Item -ItemType Directory -Force $dist | Out-Null
    Copy-Item $built $app -Force
    Write-Step "Done in $([int]((Get-Date) - $started).TotalMinutes) min"
    Write-Host "    $app"
    if ($Installer) {
        $setup = Get-ChildItem (Join-Path $env:CARGO_TARGET_DIR 'release\bundle\nsis') -Filter '*-setup.exe' |
            Sort-Object LastWriteTime | Select-Object -Last 1
        Copy-Item $setup.FullName $dist -Force
        # only the version just built is kept
        Get-ChildItem $dist, $setup.DirectoryName -Filter '*-setup.exe' |
            Where-Object { $_.Name -ne $setup.Name } | Remove-Item
        Write-Host "    $(Join-Path $dist $setup.Name)"
    }
    if ($restart) {
        Write-Host '    Restart Windows when you can, to finish installing the Build Tools.'
    }
} catch {
    Write-Host ''
    Write-Host "Build failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
