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
$Iss = Join-Path $ScriptDir 'it.soulcrafted.slicer.iss'

# The three CRT DLLs shipped beside the executables. The Universal CRT
# (ucrtbase.dll, api-ms-win-*) is part of Windows 10/11 and is not bundled.
$CrtDlls = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll')

# Written into $LASTEXITCODE before running a console tool. Only a native
# command that actually started writes that variable, so without a marker
# a command that fails to launch leaves the previous command's exit code
# in place and its failure reads as that command's success. Int32.MinValue
# is not a value any of the tools here can return.
$ExitCodeNotRun = -2147483648

# Run a console tool and fail on a non-zero exit code. The error
# preference is relaxed for the call because tools like cargo write
# progress to stderr, which a 'Stop' preference would otherwise turn into
# a terminating error even on success.
function Invoke-Tool {
    param(
        [Parameter(Mandatory = $true)][string]$What,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )
    $global:LASTEXITCODE = $ExitCodeNotRun
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Action
    }
    finally {
        $ErrorActionPreference = $previous
    }
    if ($LASTEXITCODE -eq $ExitCodeNotRun) {
        throw "$What could not be started, so nothing was built."
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

# The icon the installer and the application window both use is generated
# from the shared logo by `cargo run -p sc-icon-gen` and committed, with
# nothing tying the copy back to its source. Catch the case where the logo
# moved on and the icon was never regenerated.
#
# Ordering comes from the commit timestamps rather than the files' own
# modification times. A checkout stamps every file with the time it was
# written, which is identical for both files down to the sub-second tick
# and says nothing about which was edited first, so a file-time
# comparison here reports whichever way the checkout happened to land.
function Get-CommitTime([string]$Path) {
    $seconds = & git -C $RepoRoot log -1 --no-show-signature --format=%ct -- $Path 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $seconds) {
        return $null
    }
    return [int64]$seconds.Trim()
}

function Test-Modified([string]$Path) {
    $status = & git -C $RepoRoot status --porcelain -- $Path 2>$null
    return [bool]$status
}

function Assert-IconCurrent {
    $svg = 'packages/shared/it.soulcrafted.slicer.svg'
    $ico = 'apps/slicer-gui/assets/icons/it.soulcrafted.slicer.ico'
    foreach ($file in @($svg, $ico)) {
        if (-not (Test-Path (Join-Path $RepoRoot $file))) {
            throw "Expected icon file not found: $file"
        }
    }

    $svgTime = Get-CommitTime $svg
    $icoTime = Get-CommitTime $ico
    if ($null -eq $svgTime -or $null -eq $icoTime) {
        Write-Warning "Could not read commit times for $svg and $ico; skipping the icon freshness check."
        return
    }

    $stale = 'The icon may not reflect the current logo. Regenerate it with ' +
    '`cargo run -p sc-icon-gen`, commit the result, then re-run this script.'

    # An edit sitting in the working tree has no commit time yet, so the
    # comparison below cannot see it. A modified logo alongside an
    # untouched icon is the drift this guards against, whether or not
    # either change has been committed.
    if ((Test-Modified $svg) -and -not (Test-Modified $ico)) {
        throw "$svg has uncommitted changes but $ico does not. $stale"
    }
    if ($icoTime -lt $svgTime) {
        throw "$ico was last committed before $svg. $stale"
    }
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
        # A dumpbin that fails prints no import list, which looks exactly
        # like an executable with nothing to flag; passing its output on
        # unchecked would announce a clean result having inspected
        # nothing. Both ways it can fail, refusing to start and exiting
        # non-zero, become the same loud skip as a missing dumpbin above.
        # This check stays advisory, so neither is fatal.
        $global:LASTEXITCODE = $ExitCodeNotRun
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $output = & $dumpbin.FullName /dependents $exe
        }
        catch {
            $global:LASTEXITCODE = $ExitCodeNotRun
        }
        finally {
            $ErrorActionPreference = $previous
        }
        if ($LASTEXITCODE -ne 0) {
            $why = if ($LASTEXITCODE -eq $ExitCodeNotRun) {
                'could not be run'
            }
            else {
                "exited with code $LASTEXITCODE"
            }
            Write-Warning "dumpbin $why for $(Split-Path $exe -Leaf); skipping the import self-containment check."
            return
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
Assert-IconCurrent
Write-Host "Building SoulCrafted Slicer $Version for $Target ..."

# Keeps the builder's profile directory out of the panic locations.
if (-not $env:USERPROFILE) {
    throw 'USERPROFILE is not set, so the build paths cannot be remapped.'
}
if ($env:RUSTFLAGS) {
    Write-Warning 'RUSTFLAGS is ignored by this build.'
}
$env:CARGO_ENCODED_RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE=~"

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

$Installer = Join-Path $Cache "sc-slicer-setup-$Version-x86_64.exe"

# The output name is derived from a version that changes rarely and the
# output directory is never cleaned, so an installer from an earlier run
# is usually still sitting there under the name this run is about to
# write. Delete it first: then anything found afterwards can only be the
# product of this run, and a failed run leaves no installer to mistake
# for a current one.
if (Test-Path $Installer) {
    Remove-Item $Installer -Force
}

Write-Host 'Building the installer with Inno Setup ...'
Invoke-Tool 'ISCC' { & $Iscc "/DAppVersion=$Version" "/DStageDir=$Stage" "/DOutputDir=$Cache" $Iss }

Assert-InstallerComplete -Installer $Installer -StageDir $Stage -OutputDir $Cache

Write-Host ''
Write-Host "Built: $Installer"
