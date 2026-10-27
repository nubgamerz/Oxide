<#
.SYNOPSIS
    Keeps the ASIO SDK that Oxide builds with: a checked copy of Steinberg's SDK 2.3.4 or later,
    host files only, in out/asio-sdk (TD235).

.DESCRIPTION
    cpal's ASIO backend (asio-sys) compiles the SDK found at CPAL_ASIO_DIR, or else whatever it
    finds cached in %TEMP%\asio_sdk, or else downloads Steinberg's current SDK there. Oxide is
    GPLv3 and uses the SDK under its GPLv3 option, which only SDK 2.3.4 (October 2025) and later
    offer; an older SDK cached on a machine would quietly build under the proprietary licence.
    asio-sys also compiles every .cpp in the SDK's common folder, including the driver-side
    combase.cpp and dllentry.cpp (Microsoft's, with no GPL option), which a host never calls.

    This script makes the copy the builds use:
      -From <folder or .zip>  copies an SDK (an unpacked folder, such as %TEMP%\asio_sdk, or the
                              zip from steinberg.net/asiosdk) into out/asio-sdk, host side only;
      -Download               downloads steinberg.net/asiosdk into out/asio-sdk-download first,
                              then does the same;
      (neither)               checks the copy at -Path (default: CPAL_ASIO_DIR, else
                              out/asio-sdk) and exits 1 if it fails.

    The checks: changes.txt's newest entry is 2.3.4 or later and names the dual licensing,
    LICENSE.txt offers the GPL version 3, and none of the driver-side sources is there
    (combase.cpp, dllentry.cpp, register.cpp, debugmessage.cpp). The copy keeps the headers,
    the host sources (common/asio.cpp, host/*.cpp, host/pc/*.cpp) but not Microsoft's driver
    headers (combase.h, wxdebug.h) and the SDK's LICENSE.txt,
    README.md and changes.txt; it leaves out the driver sources, the sample driver, the PDFs
    and the logo artwork.

    Then point the builds at it, once per machine (a user environment variable, so every build,
    debug or release, uses the same SDK and none rebuilds asio-sys for a different one):
      [Environment]::SetEnvironmentVariable('CPAL_ASIO_DIR', '<repo>\out\asio-sdk', 'User')
    scripts/check.ps1 and scripts/installer.ps1 use out/asio-sdk when CPAL_ASIO_DIR isn't set;
    a release is built only with a copy that passes these checks. asio-sys keeps old objects in
    its library when the SDK changes, so after switching run `cargo clean -p asio-sys` (and with
    --release) once. Writes only inside the repository (out/asio-sdk, out/asio-sdk-download).

.EXAMPLE
    pwsh -NoProfile -File scripts/asio_sdk.ps1 -From $env:TEMP\asio_sdk
.EXAMPLE
    pwsh -NoProfile -File scripts/asio_sdk.ps1          # check the copy in use
#>
param(
    [string]$From,
    [switch]$Download,
    [string]$Path
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$target = Join-Path $root 'out/asio-sdk'
$minimum = [version]'2.3.4'
# What asio-sys would compile that a host never calls: COM server and DLL entry code for
# drivers (Microsoft's, no GPL option) and Steinberg's driver registration and debug output.
$driverSide = @('combase.cpp', 'dllentry.cpp', 'register.cpp', 'debugmessage.cpp', 'combase.h', 'wxdebug.h')

# The reasons an SDK folder isn't one to build with; none means it is.
function Get-SdkProblems {
    param([string]$Dir, [switch]$Source)
    $problems = [System.Collections.Generic.List[string]]::new()
    if (-not (Test-Path (Join-Path $Dir 'common/asio.h'))) {
        $problems.Add("$Dir has no common/asio.h: not an ASIO SDK")
        return $problems
    }
    $changes = Join-Path $Dir 'changes.txt'
    if (-not (Test-Path $changes)) {
        $problems.Add("$Dir has no changes.txt to read its version from")
    } else {
        $text = Get-Content $changes -Raw
        if ($text -match 'Changes in ASIO (\d+\.\d+(?:\.\d+)?)') {
            $found = [version]$Matches[1]
            if ($found -lt $minimum) {
                $problems.Add("$Dir is ASIO SDK $found; the GPLv3 option needs $minimum or later")
            }
        } else {
            $problems.Add("$Dir's changes.txt names no version")
        }
        if ($text -notmatch 'DUAL licensing') {
            $problems.Add("$Dir's changes.txt doesn't mention the dual licensing")
        }
    }
    $license = Join-Path $Dir 'LICENSE.txt'
    if (-not (Test-Path $license) -or (Get-Content $license -Raw) -notmatch 'General Public License \(GPL\) Version 3') {
        $problems.Add("$Dir's LICENSE.txt doesn't offer the GPL version 3")
    }
    if (-not $Source) {
        foreach ($name in $driverSide) {
            if (Test-Path (Join-Path $Dir "common/$name")) {
                $problems.Add("$Dir holds common/$name (driver side, would be compiled): make the copy with -From")
            }
        }
    }
    return $problems
}

function Get-SdkVersion([string]$Dir) {
    if ((Get-Content (Join-Path $Dir 'changes.txt') -Raw) -match 'Changes in ASIO (\d+\.\d+(?:\.\d+)?)') { $Matches[1] }
}

if ($Download) {
    if ($From) { throw '-Download and -From together: give one.' }
    $work = Join-Path $root 'out/asio-sdk-download'
    if (Test-Path $work) { Remove-Item -Recurse -Force $work }
    New-Item -ItemType Directory -Force $work | Out-Null
    $zip = Join-Path $work 'asiosdk.zip'
    Write-Host 'Downloading https://www.steinberg.net/asiosdk ...'
    Invoke-WebRequest -Uri 'https://www.steinberg.net/asiosdk' -OutFile $zip -UseBasicParsing
    $From = $zip
}

if ($From) {
    $source = (Resolve-Path $From).Path
    if ($source -like '*.zip') {
        $unpacked = Join-Path $root 'out/asio-sdk-download/unpacked'
        if (Test-Path $unpacked) { Remove-Item -Recurse -Force $unpacked }
        Expand-Archive -Path $source -DestinationPath $unpacked
        # The zip holds one folder (asiosdk_2.3.4_2025-10-15 or similar) with the SDK in it.
        $inner = Get-ChildItem $unpacked -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'common/asio.h') } | Select-Object -First 1
        $source = if ($inner) { $inner.FullName } else { $unpacked }
    }
    $problems = @(Get-SdkProblems $source -Source)
    if ($problems) {
        $problems | ForEach-Object { Write-Host "REFUSED: $_" -ForegroundColor Red }
        exit 1
    }
    if (Test-Path $target) { Remove-Item -Recurse -Force $target }
    foreach ($dir in @('common', 'host', 'host/pc')) {
        New-Item -ItemType Directory -Force (Join-Path $target $dir) | Out-Null
    }
    foreach ($name in @('LICENSE.txt', 'README.md', 'changes.txt')) {
        $file = Join-Path $source $name
        if (Test-Path $file) { Copy-Item $file (Join-Path $target $name) }
    }
    # The headers, and the host's sources: common/asio.cpp, host/*.cpp, host/pc/*.cpp (asio-sys
    # reads these three folders one level deep, so host/sample is never compiled anyway).
    # combase.h and wxdebug.h are Microsoft's, for the driver side only; nothing a host compiles
    # includes them.
    Get-ChildItem (Join-Path $source 'common') -File -Filter '*.h' | Where-Object Name -notin 'combase.h', 'wxdebug.h' |
        Copy-Item -Destination (Join-Path $target 'common')
    Copy-Item (Join-Path $source 'common/asio.cpp') (Join-Path $target 'common')
    foreach ($dir in @('host', 'host/pc')) {
        Get-ChildItem (Join-Path $source $dir) -File | Where-Object { $_.Extension -in '.h', '.cpp' } |
            Copy-Item -Destination (Join-Path $target $dir)
    }
    $left = @(Get-ChildItem (Join-Path $source 'common') -File -Filter '*.cpp' | Where-Object Name -ne 'asio.cpp' | ForEach-Object Name)
    Write-Host "Copied ASIO SDK $(Get-SdkVersion $target) into $target (host side; left out: $($left -join ', '), the sample driver, the PDFs and logos)."
    $Path = $target
}

if (-not $Path) { $Path = if ($env:CPAL_ASIO_DIR) { $env:CPAL_ASIO_DIR } else { $target } }
if (-not (Test-Path $Path)) {
    Write-Host "FAIL: no ASIO SDK at $Path. Make one: scripts/asio_sdk.ps1 -From <SDK folder or zip> (or -Download)." -ForegroundColor Red
    exit 1
}
$problems = @(Get-SdkProblems $Path)
if ($problems) {
    $problems | ForEach-Object { Write-Host "FAIL: $_" -ForegroundColor Red }
    exit 1
}
Write-Host "RESULT: PASS ASIO SDK $(Get-SdkVersion $Path) at ${Path}: GPLv3 offered, host files only."
$using = $env:CPAL_ASIO_DIR
if (-not $using) {
    Write-Host "CPAL_ASIO_DIR isn't set: builds use asio-sys's %TEMP%\asio_sdk instead. Set it once:" -ForegroundColor Yellow
    Write-Host "  [Environment]::SetEnvironmentVariable('CPAL_ASIO_DIR', '$((Resolve-Path $Path).Path)', 'User')"
} elseif ((Resolve-Path $using).Path -ne (Resolve-Path $Path).Path) {
    Write-Host "Note: CPAL_ASIO_DIR is $using, not this copy." -ForegroundColor Yellow
}
