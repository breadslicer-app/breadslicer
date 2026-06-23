<#
.SYNOPSIS
    Build the SoulCrafted Slicer Windows installer.

.DESCRIPTION
    Builds both release binaries for the x64 MSVC target, stages them
    next to the app-local VC runtime DLLs and the bundled profiles in the
    layout the runtime expects, sanity-checks the binaries' imports, and
    runs the Inno Setup compiler to emit a single setup.exe.

    Everything lands under .cache next to this script; nothing is
    installed. Requires the Visual Studio Build Tools (for the MSVC
    target and the redistributable CRT DLLs) and Inno Setup 6.3 or later.
#>

$ErrorActionPreference = 'Stop'

$ScriptDir = $PSScriptRoot
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$Cache = Join-Path $ScriptDir '.cache'
$Stage = Join-Path $Cache 'stage'
$Target = 'x86_64-pc-windows-msvc'
$ReleaseDir = Join-Path $RepoRoot "target\$Target\release"
$Iss = Join-Path $ScriptDir 'it.soulcrafted.Slicer.iss'

# The three CRT DLLs shipped beside the executables. The Universal CRT
# (ucrtbase.dll, api-ms-win-*) is part of Windows 10/11 and is not bundled.
$CrtDlls = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll')

# Run a console tool and fail on a non-zero exit code. The error
# preference is relaxed for the call because tools like cargo write
# progress to stderr, which a 'Stop' preference would otherwise turn into
# a terminating error even on success.
function Invoke-Tool {
    param(
        [Parameter(Mandatory = $true)][string]$What,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Action
    }
    finally {
        $ErrorActionPreference = $previous
    }
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed with exit code $LASTEXITCODE."
    }
}

function Read-AppVersion {
    $cargoToml = Get-Content (Join-Path $RepoRoot 'apps\slicer-gui\Cargo.toml') -Raw
    if ($cargoToml -match '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }
    throw 'Could not read the version from apps\slicer-gui\Cargo.toml.'
}

function Find-VsInstall {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        throw "vswhere.exe not found at $vswhere. Install the Visual Studio Build Tools."
    }
    $path = & $vswhere -latest -products * -property installationPath
    if (-not $path) {
        throw 'vswhere found no Visual Studio installation.'
    }
    return $path.Trim()
}

# Locate the redistributable CRT DLLs under the VS install. The redist
# version directory and the Microsoft.VC<n>.CRT folder both move with the
# toolset, so glob for them and take the newest.
function Find-CrtDir([string]$VsInstall) {
    $redistRoot = Join-Path $VsInstall 'VC\Redist\MSVC'
    if (-not (Test-Path $redistRoot)) {
        throw "VC redistributable directory not found at $redistRoot."
    }
    $versions = Get-ChildItem $redistRoot -Directory | Sort-Object Name -Descending
    foreach ($v in $versions) {
        $crt = Get-ChildItem (Join-Path $v.FullName 'x64') -Directory -Filter 'Microsoft.VC*.CRT' -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending | Select-Object -First 1
        if ($crt) {
            return $crt.FullName
        }
    }
    throw "No Microsoft.VC*.CRT folder found under $redistRoot."
}

# Best-effort import check: dumpbin lists the DLLs an executable imports.
# Flag any C/C++ runtime DLL that is not in the shipped set, which would
# mean the install is not self-contained.
function Test-Imports([string]$VsInstall, [string[]]$Exes) {
    $dumpbin = Get-ChildItem (Join-Path $VsInstall 'VC\Tools\MSVC') -Recurse -Filter 'dumpbin.exe' -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\Hostx64\\x64\\' } | Select-Object -First 1
    if (-not $dumpbin) {
        Write-Warning 'dumpbin.exe not found; skipping the import self-containment check.'
        return
    }
    $allowed = $CrtDlls | ForEach-Object { $_.ToLower() }
    foreach ($exe in $Exes) {
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $output = & $dumpbin.FullName /dependents $exe
        }
        finally {
            $ErrorActionPreference = $previous
        }
        foreach ($line in $output) {
            $name = $line.Trim()
            if ($name -match '(?i)^(msvcp\d|vcruntime\d|concrt\d|mfc\d).*\.dll$') {
                if ($allowed -notcontains $name.ToLower()) {
                    throw "$(Split-Path $exe -Leaf) imports $name, which is not in the shipped CRT set."
                }
            }
        }
    }
    Write-Host 'Import check passed: the executables import no CRT DLL outside the shipped set.'
}

function Find-Iscc {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) {
            return $c
        }
    }
    return $null
}

# Guard against a truncated installer. A short write, most often caused by
# real-time antivirus scanning modifying setup.exe as it is produced,
# still leaves a file on disk, but Inno Setup's loader rejects it at run
# time with an opaque "setup files are corrupted" dialog. Native
# executables do not compress below roughly a third of their size with
# LZMA2, so an installer under a quarter of the staged payload signals an
# incomplete write rather than good compression; fail loudly at build time
# instead of letting the broken installer ship.
function Assert-InstallerComplete {
    param(
        [Parameter(Mandatory = $true)][string]$Installer,
        [Parameter(Mandatory = $true)][string]$StageDir,
        [Parameter(Mandatory = $true)][string]$OutputDir
    )
    if (-not (Test-Path $Installer)) {
        throw "Inno Setup reported success but no installer was found at $Installer."
    }
    $payloadBytes = (Get-ChildItem $StageDir -Recurse -File | Measure-Object -Property Length -Sum).Sum
    $installerBytes = (Get-Item $Installer).Length
    $minBytes = [int64]($payloadBytes / 4)
    if ($installerBytes -lt $minBytes) {
        throw ("Installer is {0:N0} bytes for a {1:N0} byte payload, too small to be complete; " -f $installerBytes, $payloadBytes) +
        'it was likely truncated while being written (often by real-time antivirus scanning). ' +
        "Exclude '$OutputDir' from antivirus scanning and rebuild."
    }
    Write-Host ("Installer size check passed: {0:N0} bytes from a {1:N0} byte payload." -f $installerBytes, $payloadBytes)
}

# --- Build -------------------------------------------------------------

$Version = Read-AppVersion
Write-Host "Building SoulCrafted Slicer $Version for $Target ..."

Push-Location $RepoRoot
try {
    Invoke-Tool 'cargo build' { cargo build --release --target $Target -p sc-slicer-gui -p sc-slicer-cli }
}
finally {
    Pop-Location
}

# --- Stage -------------------------------------------------------------

Write-Host 'Staging the install payload ...'
if (Test-Path $Stage) {
    Remove-Item $Stage -Recurse -Force
}
New-Item -ItemType Directory -Path $Stage -Force | Out-Null

$VsInstall = Find-VsInstall
$CrtDir = Find-CrtDir $VsInstall

$GuiExe = Join-Path $ReleaseDir 'sc-slicer-gui.exe'
$CliExe = Join-Path $ReleaseDir 'sc-slicer-cli.exe'
foreach ($exe in @($GuiExe, $CliExe)) {
    if (-not (Test-Path $exe)) {
        throw "Expected binary not found: $exe"
    }
    Copy-Item $exe $Stage
}

foreach ($dll in $CrtDlls) {
    $src = Join-Path $CrtDir $dll
    if (-not (Test-Path $src)) {
        throw "Redistributable DLL not found: $src"
    }
    Copy-Item $src $Stage
}

# The runtime resolves its read-only data root as a `slicer` directory
# beside the binaries; the bundled profiles live under it.
$ProfilesSrc = Join-Path $RepoRoot 'assets\profiles'
$ProfilesDst = Join-Path $Stage 'slicer\profiles'
New-Item -ItemType Directory -Path $ProfilesDst -Force | Out-Null
Copy-Item (Join-Path $ProfilesSrc '*') $ProfilesDst -Recurse -Force

Test-Imports $VsInstall @($GuiExe, $CliExe)

# --- Package -----------------------------------------------------------

$Iscc = Find-Iscc
if (-not $Iscc) {
    Write-Host ''
    Write-Host 'Staging is complete under:'
    Write-Host "  $Stage"
    Write-Host ''
    Write-Host 'Inno Setup was not found, so the installer was not built.'
    Write-Host 'Install Inno Setup 6.3 or later from https://jrsoftware.org/isdl.php'
    Write-Host 'and re-run this script to produce the setup.exe.'
    return
}

Write-Host 'Building the installer with Inno Setup ...'
Invoke-Tool 'ISCC' { & $Iscc "/DAppVersion=$Version" "/DStageDir=$Stage" "/DOutputDir=$Cache" $Iss }

$Installer = Join-Path $Cache "sc-slicer-setup-$Version-x64.exe"
Assert-InstallerComplete -Installer $Installer -StageDir $Stage -OutputDir $Cache

Write-Host ''
Write-Host "Built: $Installer"
