<#
.SYNOPSIS
    Takes the manual's screenshots (docs/manual/shots.toml) from the release app, with their
    marks, into -ShotDir (docs/manual/shots by default, where they are committed; dev plan 27.2,
    27.3, 28.1). Prints a line per shot and `RESULT: PASS`, or `FAIL:` lines and exits 1.

.DESCRIPTION
    Run by check.ps1's manual tier, and by installer.ps1 -RetakeShots; a build otherwise uses the
    committed shots as they are. Retake only what a change alters (`-Only`), and commit the new
    shots with the change: `git status` shows which pictures moved. Each shot runs the app once
    on the demo (`--demo <audio>`; a copy of -Audio named Guitar.wav in -WorkDir, so clips and
    tabs show a neutral name and the original is never touched), with `--neutral`:
    no audio device is opened (a virtual one runs the engine), and what is drawn names no
    machine's devices or folders (`daw-app/src/neutral.rs`); MCP serves on port 7896, shown as
    the default 7878. Each shot's output goes to <WorkDir>/logs/<name>.log. The pictures taken
    are then packed (`manual --pack-shots`): re-encoded without loss, so their marks still fit.

.EXAMPLE
    pwsh scripts/manual_shots.ps1 ... -Only mixer,export     # just those two, the rest kept

.EXAMPLE
    pwsh scripts/manual_shots.ps1 -Exe target/release/daw.exe -ManualExe target/release/manual.exe -Audio out/check/demo-guitar.wav
#>
param(
    [Parameter(Mandatory)] [string]$Exe,
    [Parameter(Mandatory)] [string]$ManualExe,
    [Parameter(Mandatory)] [string]$Audio,
    # Where the shots go: the committed folder by default.
    [string]$ShotDir = 'docs/manual/shots',
    # The demo's copy of the audio and each shot's log (never committed).
    [string]$WorkDir = 'out/manual-shots-work',
    [int]$TimeoutSec = 120,
    # Take only these shots, keeping the others already in -ShotDir (while writing pages).
    [string[]]$Only = @(),
    # More of the app's arguments for every shot, tab-separated (one string, so it survives
    # `pwsh -File`): e.g. the VST3 a scenario's instrument role uses (`--vst3 synth=<name>`);
    # without one, those scenarios go without.
    [string]$AppArgs = ''
)

$ErrorActionPreference = 'Stop'
# `-Only a,b` from another shell arrives as one string.
$Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$exe = (Resolve-Path $Exe).Path
$audio = (Resolve-Path $Audio).Path
# Relative to the repository, or absolute (installer.ps1 passes its own).
$shots = [IO.Path]::GetFullPath([IO.Path]::Combine($root, $ShotDir))
$work = [IO.Path]::GetFullPath([IO.Path]::Combine($root, $WorkDir))
$logs = Join-Path $work 'logs'
New-Item -ItemType Directory -Force $shots, $logs | Out-Null
if (-not $Only) {
    Get-ChildItem (Join-Path $shots '*') -File -Include *.png, *.json -ErrorAction SilentlyContinue | Remove-Item
    Get-ChildItem $logs -File -ErrorAction SilentlyContinue | Remove-Item
}
# The demo's audio under a neutral name (its clips are named after the file): a copy, so the
# recording given is only read (27.7).
$neutralAudio = Join-Path $work 'Guitar.wav'
if ($audio -ne $neutralAudio) {
    Copy-Item -LiteralPath $audio -Destination $neutralAudio -Force
}
$audio = $neutralAudio

# shots.toml as command lines: a name, then the app's arguments, tab-separated.
$list = & (Resolve-Path $ManualExe).Path --list-shots
if ($LASTEXITCODE -ne 0) {
    Write-Host "FAIL: manual --list-shots exited $LASTEXITCODE"
    exit 1
}
$extra = @($AppArgs -split "`t" | Where-Object { $_ })
$failed = 0
$taken = 0
foreach ($line in ($list | Where-Object { $_.Trim() })) {
    $fields = @($line -split "`t")
    $name = $fields[0]
    if ($Only -and $Only -notcontains $name) { continue }
    $png = Join-Path $shots "$name.png"
    $json = Join-Path $shots "$name.json"
    # Muted, unless the shot asks for --silent (the master runs and is measured, zeros are
    # sent: the loudness panel, an export's report, a mixdown); no device is opened either way.
    $own = @($fields | Select-Object -Skip 1)
    $mute = if ($own -contains '--silent') { @() } else { @('--mute') }
    $arguments = @('--demo', $audio) + $mute + @('--neutral', '--mcp-port', '7896') +
        $own + $extra + @('--screenshot', $png, '--marks', $json)
    $psi = [System.Diagnostics.ProcessStartInfo]::new($exe)
    foreach ($a in $arguments) { $psi.ArgumentList.Add($a) }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $root
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $p = [System.Diagnostics.Process]::Start($psi)
    $stdout = $p.StandardOutput.ReadToEndAsync()
    $stderr = $p.StandardError.ReadToEndAsync()
    $exited = $p.WaitForExit($TimeoutSec * 1000)
    if (-not $exited) { $p.Kill($true) }
    $p.WaitForExit()
    Set-Content -Path (Join-Path $logs "$name.log") -Value ($stdout.Result + $stderr.Result)
    $seconds = $watch.Elapsed.TotalSeconds
    if (-not $exited) {
        Write-Host ("FAIL: shot {0} timed out after {1} s" -f $name, $TimeoutSec)
        $failed++
    } elseif ($p.ExitCode -ne 0) {
        Write-Host ("FAIL: shot {0} exited {1} (log: {2})" -f $name, $p.ExitCode, (Join-Path $logs "$name.log"))
        $failed++
    } elseif (-not ((Test-Path $png) -and (Test-Path $json))) {
        Write-Host "FAIL: shot $name wrote no screenshot or marks file"
        $failed++
    } else {
        Write-Host ("shot {0}: {1:N1} s" -f $name, $seconds)
        $taken++
    }
}
if ($failed) { exit 1 }
# Packed without loss (the app writes them quickly, not small).
$pngs = @(Get-ChildItem (Join-Path $shots '*.png') | Where-Object { -not $Only -or $Only -contains $_.BaseName } |
    ForEach-Object FullName)
if ($pngs) {
    & (Resolve-Path $ManualExe).Path @($pngs | ForEach-Object { '--pack-shots', $_ })
    if ($LASTEXITCODE -ne 0) { Write-Host 'FAIL: the shots could not be packed'; exit 1 }
}
Write-Host "RESULT: PASS $taken shots in $shots"
exit 0
