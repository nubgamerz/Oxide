<#
.SYNOPSIS
    Builds Oxide's Windows installer: out/dist/Oxide-Setup-<version>.exe (dev plan 19.1).

.DESCRIPTION
    1. Builds the release programs (daw.exe, oxide-mcp.exe) with the version and commit
       baked in (OXIDE_VERSION only when -Version overrides the workspace's; OXIDE_COMMIT is
       the short hash, "-modified" when tracked files have changes).
    2. Stages what is installed in <OutDir>/stage: daw.exe as Oxide.exe, oxide-mcp.exe, the
       icon, README.txt, LICENSE.txt and NOTICE.txt (the repository's LICENSE and NOTICE),
       THIRD-PARTY.txt (scripts/third_party.ps1) and the manual (27.3):
       built strictly by crates/manual into stage\manual from the committed screenshots in
       docs/manual/shots (28.1; a missing shot or a broken reference fails the build), or with
       -RetakeShots from shots taken again on the release app just built
       (scripts/manual_shots.ps1, --neutral: no audio device opened). No PDB.
    3. Compiles installer/oxide.iss with Inno Setup's ISCC (G:\Tools\InnoSetup, or on the
       PATH) into <OutDir>, writes its SHA-256 beside it (<installer>.sha256, as sha256sum
       writes it) and prints the installer's path, size and SHA-256.

    A release is built with this script on the maintainer's PC, with the committed shots: it
    never starts the app. It builds with the checked ASIO SDK in out/asio-sdk
    (scripts/asio_sdk.ps1) when CPAL_ASIO_DIR names none.

    The installer is not signed: Windows SmartScreen warns on its first run on a machine
    ("Windows protected your PC": More info > Run anyway). See the dev plan's TD row on signing.

    -Check builds the installer check's variant (the maintainer's install check): its own AppId
    and name ("Oxide Installer Check"), no shortcuts, no file association, no mutex check, and
    a default folder of -DefaultDir. Never give a check build to anyone.

.PARAMETER Version
    The version to build as (major.minor.patch). Defaults to the workspace's (Cargo.toml).
    The manual's footer names it too.
    Anything else is for the check: the app is rebuilt to report it.

.EXAMPLE
    pwsh scripts/installer.ps1
.EXAMPLE
    pwsh scripts/installer.ps1 -RetakeShots   # the manual's shots taken again first
.EXAMPLE
    pwsh scripts/installer.ps1 -Check -Version 0.2.1 -OutDir out/check/installer -DefaultDir <repository>\out\check\install
#>
param(
    [string]$Version = '',
    [switch]$Check,
    [string]$DefaultDir = '',
    [string]$OutDir = 'out/dist',
    # Take the manual's screenshots again (scripts/manual_shots.ps1, into docs/manual/shots)
    # from the program just built, instead of using the committed ones.
    [switch]$RetakeShots,
    # The demo's audio for -RetakeShots: else the maintainer's default (check.ps1's hook, when
    # present), else a generated guitar riff (dawpoc gen --guitar).
    [string]$Audio = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Find-Iscc {
    foreach ($candidate in @('G:\Tools\InnoSetup\ISCC.exe')) {
        if (Test-Path $candidate) { return $candidate }
    }
    $onPath = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

$iscc = Find-Iscc
if (-not $iscc) {
    Write-Host 'FAIL: Inno Setup''s ISCC.exe is neither in G:\Tools\InnoSetup nor on the PATH'
    exit 1
}

$cargoVersion = ((& cargo metadata --no-deps --format-version 1) | ConvertFrom-Json).packages |
    Where-Object name -EQ 'daw-app' | ForEach-Object version
if (-not $Version) { $Version = $cargoVersion }
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    Write-Host "FAIL: the version '$Version' isn't major.minor.patch"
    exit 1
}
if ($Check -and -not $DefaultDir) {
    Write-Host 'FAIL: a check build needs -DefaultDir (its own folder, never G:\Oxide)'
    exit 1
}

$commit = (& git rev-parse --short HEAD).Trim()
if (& git status --porcelain --untracked-files=no) { $commit += '-modified' }

# The checked ASIO SDK (scripts/asio_sdk.ps1, TD235) when CPAL_ASIO_DIR names none, as
# check.ps1 does, so both build the same asio-sys.
if (-not $env:CPAL_ASIO_DIR -and (Test-Path (Join-Path $root 'out/asio-sdk/common/asio.h'))) {
    $env:CPAL_ASIO_DIR = (Resolve-Path (Join-Path $root 'out/asio-sdk')).Path
}
Write-Host "ASIO SDK: $(if ($env:CPAL_ASIO_DIR) { $env:CPAL_ASIO_DIR } else { "asio-sys's own (%TEMP%sio_sdk, or downloaded)" })"

# 1. Build. OXIDE_VERSION only when it differs, so a normal build is the workspace's.
$saved = @{ OXIDE_VERSION = $env:OXIDE_VERSION; OXIDE_COMMIT = $env:OXIDE_COMMIT }
try {
    $env:OXIDE_VERSION = if ($Version -ne $cargoVersion) { $Version } else { $null }
    $env:OXIDE_COMMIT = $commit
    Write-Host "Building Oxide $Version ($commit), release"
    & cargo build --release -p daw-app --bins
    if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: the release build failed'; exit 1 }
} finally {
    $env:OXIDE_VERSION = $saved.OXIDE_VERSION
    $env:OXIDE_COMMIT = $saved.OXIDE_COMMIT
}
# Cargo's target folder: CARGO_TARGET_DIR when set, else target/ here.
$target = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $root 'target' }
$release = Join-Path $target 'release'
$built = (& "$release/daw.exe" --version | Out-String).Trim()
if ($built -ne "Oxide $Version ($commit)") {
    Write-Host "FAIL: the built program says '$built', not 'Oxide $Version ($commit)'"
    exit 1
}

# 2. Stage.
$out = [IO.Path]::GetFullPath((Join-Path $root $OutDir))
$stage = Join-Path $out 'stage'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item "$release/daw.exe" (Join-Path $stage 'Oxide.exe')
Copy-Item "$release/oxide-mcp.exe" $stage
Copy-Item 'crates/daw-app/assets/oxide.ico' $stage
Copy-Item 'installer/README.txt' $stage
# The GPLv3 text and the notice, as .txt so Windows opens them.
Copy-Item 'LICENSE' (Join-Path $stage 'LICENSE.txt')
Copy-Item 'NOTICE' (Join-Path $stage 'NOTICE.txt')
& (Get-Process -Id $PID).Path -NoProfile -File scripts/third_party.ps1 -Out (Join-Path $stage 'THIRD-PARTY.txt') -Version $Version
if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: THIRD-PARTY.txt could not be written'; exit 1 }

# The manual (27.3): screenshots from the program just built (its version in the status bar),
# then the pages built strictly into stage\manual. Help > Manual and F1 open it there.
$watch = [System.Diagnostics.Stopwatch]::StartNew()
& cargo build --release -p manual
if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: the manual''s generator did not build'; exit 1 }
# The shots are committed (docs/manual/shots, 28.1): a build uses them as they are, and only
# -RetakeShots takes them again from the program just built (into the same folder, for review
# and a commit).
$shotDir = Join-Path $root 'docs/manual/shots'
if ($RetakeShots) {
    # The maintainer's defaults kept outside the repository (as check.ps1 reads them): the
    # demo's audio and the app's arguments for the shots. Dot-sourcing only defines them.
    $LocalAudio = ''
    $LocalAppArgs = @()
    $localChecks = Join-Path $PSScriptRoot 'check.local.ps1'
    if (Test-Path $localChecks) { . $localChecks }
    if (-not $Audio) {
        $Audio = if ($LocalAudio -and (Test-Path $LocalAudio)) { $LocalAudio } else {
            # Without a recording: a plucked guitar riff, 6 s at 44.1 kHz mono like a DI take.
            & cargo build --release -p dawpoc
            if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: dawpoc did not build'; exit 1 }
            New-Item -ItemType Directory -Force out/check | Out-Null
            & "$release/dawpoc.exe" gen --guitar --seconds 6 --sample-rate 44100 --output out/check/demo-guitar.wav
            if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: the demo''s guitar was not made'; exit 1 }
            'out/check/demo-guitar.wav'
        }
    }
    # The app's own arguments for every shot, only when there are any (an empty argument is
    # dropped by older argument passing, leaving -AppArgs without a value).
    $appArgs = if ($LocalAppArgs) { @('-AppArgs', ($LocalAppArgs -join "`t")) } else { @() }
    & (Get-Process -Id $PID).Path -NoProfile -File scripts/manual_shots.ps1 -Exe "$release/daw.exe" `
        -ManualExe "$release/manual.exe" -Audio $Audio -ShotDir $shotDir @appArgs
    if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: the manual''s screenshots were not all taken'; exit 1 }
} elseif ($Audio) {
    Write-Host 'FAIL: -Audio is the demo''s audio for -RetakeShots; without it the committed shots are used'
    exit 1
}
$manualDir = Join-Path $stage 'manual'
& "$release/manual.exe" --shots $shotDir --strict --out $manualDir --version $Version
if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: the manual did not build'; exit 1 }
# The generator's marker (a later build may empty the folder) has no use once installed.
Remove-Item (Join-Path $manualDir '.oxide-manual') -ErrorAction SilentlyContinue
foreach ($needed in @('index.html', 'search-index.js', 'search-index.json')) {
    if (-not (Test-Path (Join-Path $manualDir $needed))) {
        Write-Host "FAIL: the manual has no $needed"
        exit 1
    }
}
$manualBytes = (Get-ChildItem $manualDir -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("Manual:    {0:N2} MB in {1} files, built in {2:N0} s" -f ($manualBytes / 1MB),
    @(Get-ChildItem $manualDir -Recurse -File).Count, $watch.Elapsed.TotalSeconds)

# 3. Compile.
$defines = @("-dAppVersion=$Version", "-dAppCommit=$commit", "-dStage=$stage")
if ($Check) { $defines += @('-dCheckBuild=1', "-dDefaultDir=$DefaultDir") }
$name = if ($Check) { "OxideCheck-Setup-$Version.exe" } else { "Oxide-Setup-$Version.exe" }
$setup = Join-Path $out $name
Remove-Item $setup -ErrorAction SilentlyContinue
& $iscc -q @defines "-o$out" (Join-Path $root 'installer/oxide.iss')
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $setup)) {
    Write-Host "FAIL: ISCC failed (exit $LASTEXITCODE)"
    exit 1
}
$file = Get-Item $setup
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash
# Beside it, as sha256sum writes it ("<hash>  <name>"), so `sha256sum -c` and
# `Get-FileHash` can both check a download; a release publishes it with the installer.
Set-Content -Path "$setup.sha256" -Value ("{0}  {1}" -f $hash.ToLowerInvariant(), $name) -NoNewline
Write-Host ("Installer: {0}" -f $file.FullName)
Write-Host ("Size:      {0:N1} MB ({1} bytes)" -f ($file.Length / 1MB), $file.Length)
Write-Host ("SHA-256:   {0} (in {1}.sha256)" -f $hash, $name)
Write-Host ("Version:   {0} ({1}){2}" -f $Version, $commit, $(if ($Check) { ', a check build' } else { '' }))
exit 0
