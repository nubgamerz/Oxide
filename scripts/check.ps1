<#
.SYNOPSIS
    Automated checks for the DAW. Writes out/check/report.md plus one log per step in
    out/check/logs, prints a summary, and exits 1 if any step failed.

.DESCRIPTION
    Two tiers anyone can run:

    quick  Format, build, the manual's reference check (crates/manual --check), unit and
           integration tests, clippy. No plug-ins, no audio device.
    manual The manual with real screenshots (27.2): each shot in docs/manual/shots.toml taken
           from the release app (scripts/manual_shots.ps1: --neutral, no audio device opened
           and no machine's names drawn; MCP on port 7896) with its marks into
           docs/manual/shots (committed, 28.1: review and commit what moved), the manual built from them strictly (out/manual: crops, highlights,
           compressed images, sizes reported), then checked in headless Edge and Chrome
           (scripts/manual_browser_check.ps1; SKIP without Node.js or a browser).

    full, live and ui are the maintainer's own tiers: they need commercial plug-ins, audio
    devices and recordings that aren't in the repository, and their steps are kept outside it
    too. Without them, asking for one of those tiers says so and exits 1.

.PARAMETER Audio
    Test WAV for the manual's demo. Defaults to a generated guitar riff (dawpoc gen --guitar).

.EXAMPLE
    pwsh scripts/check.ps1 -Tier quick
    pwsh scripts/check.ps1 -Tier manual -Audio my-guitar.wav
#>
param(
    [ValidateSet('quick', 'full', 'live', 'ui', 'manual')]
    [string]$Tier = 'quick',
    [string]$Audio = '',
    [string]$Asio = '',
    [int]$AsioBuffer = 64,
    [string]$Loopback = '',
    [string]$LoopbackHost = 'ASIO',
    [int]$LoopbackInput = 0
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# The maintainer's own tiers (full, live, ui), kept outside the repository: when their file is
# here, dot-sourcing it defines $LocalTiers (run after the quick tier, in this scope) and may
# set defaults ($LocalAudio, the demo's audio; $LocalAppArgs, the app's arguments for the
# shots). Nothing in it runs yet.
$LocalTiers = $null
$LocalAudio = ''
$LocalAppArgs = @()
$localChecks = Join-Path $PSScriptRoot 'check.local.ps1'
if (Test-Path $localChecks) { . $localChecks }
if ($Tier -in 'full', 'live', 'ui' -and -not $LocalTiers) {
    Write-Host "FAIL: the $Tier tier is the maintainer's own: it needs commercial plug-ins, audio devices and recordings that aren't in the repository, and its steps are kept outside it. Run -Tier quick (every change) or -Tier manual."
    exit 1
}
if (-not $Audio -and $LocalAudio -and (Test-Path $LocalAudio)) { $Audio = $LocalAudio }
$outDir = Join-Path $root 'out/check'
$logDir = Join-Path $outDir 'logs'
New-Item -ItemType Directory -Force $logDir | Out-Null
Get-ChildItem $logDir -File | Remove-Item
$cargo = (Get-Command cargo).Source
# The checked ASIO SDK (scripts/asio_sdk.ps1, TD235) when CPAL_ASIO_DIR names none, as
# installer.ps1 does, so the checks build the asio-sys a release does.
if (-not $env:CPAL_ASIO_DIR -and (Test-Path (Join-Path $root 'out/asio-sdk/common/asio.h'))) {
    $env:CPAL_ASIO_DIR = (Resolve-Path (Join-Path $root 'out/asio-sdk')).Path
}
# Cargo's target folder: CARGO_TARGET_DIR when set, else target/ here.
$target = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $root 'target' }
$release = Join-Path $target 'release'
$results = [System.Collections.Generic.List[object]]::new()
$started = Get-Date

# On GitHub Actions each step's output is shown as it arrives, in a collapsible group, as
# well as logged: a cold build is silent for many minutes otherwise. The repository has no
# workflows now (Actions are off, 28.6); this stays for a CI that might run it again.
# Locally a step prints only its summary line, as before. The lines are gathered by handlers
# on the process's own reader threads, which can't run PowerShell, hence the C#.
$stream = $env:GITHUB_ACTIONS -eq 'true'
if ($stream) {
    Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Text;

public sealed class StepOutput {
    private readonly StringBuilder text = new StringBuilder();
    private readonly object gate = new object();

    public StepOutput(Process p) {
        p.OutputDataReceived += (s, e) => Add(e.Data);
        p.ErrorDataReceived += (s, e) => Add(e.Data);
        p.BeginOutputReadLine();
        p.BeginErrorReadLine();
    }

    private void Add(string line) {
        if (line == null) return;
        lock (gate) {
            text.AppendLine(line);
            Console.Out.WriteLine(line);
            Console.Out.Flush();
        }
    }

    public string Text { get { lock (gate) { return text.ToString(); } } }
}
'@
}

function Add-Result($Name, $Status, $Seconds, $Detail) {
    $results.Add([pscustomobject]@{ Step = $Name; Status = $Status; Seconds = $Seconds; Detail = $Detail })
    $color = @{ PASS = 'Green'; FAIL = 'Red'; SKIP = 'Yellow' }[$Status]
    Write-Host ("{0,-5} {1,-22} {2,6:N1}s  {3}" -f $Status, $Name, $Seconds, $Detail) -ForegroundColor $color
}

# Run one step with a timeout, log its output, and judge it by exit code.
function Invoke-Step {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$Arguments = @(),
        [int]$TimeoutSec = 900,
        [string]$Skip = ''
    )
    if ($Skip) {
        Add-Result $Name 'SKIP' 0 $Skip
        return $null
    }
    # GitHub's runners (4 cores, often a cold cache) take many times this machine's time: the
    # quick tier's cold debug build alone ran past 20 minutes there (28.1). Their limits are
    # four times as long; the job's own timeout still bounds a real hang.
    if ($stream) { $TimeoutSec *= 4 }
    $psi = [System.Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in $Arguments) { $psi.ArgumentList.Add($a) }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $root
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    if ($stream) { Write-Host "::group::$Name" }
    $p = [System.Diagnostics.Process]::Start($psi)
    if ($stream) {
        $output = [StepOutput]::new($p)
    } else {
        $stdout = $p.StandardOutput.ReadToEndAsync()
        $stderr = $p.StandardError.ReadToEndAsync()
    }
    $exited = $p.WaitForExit($TimeoutSec * 1000)
    if (-not $exited) { $p.Kill($true) }
    # Without a timeout this also waits for the redirected output to be read to its end.
    $p.WaitForExit()
    if ($stream) {
        $text = $output.Text
        Write-Host '::endgroup::'
    } else {
        $text = $stdout.Result + $stderr.Result
    }
    Set-Content -Path (Join-Path $logDir "$Name.log") -Value $text
    $seconds = $watch.Elapsed.TotalSeconds

    $lines = $text -split "`r?`n"
    $tests = $lines | Select-String -Pattern 'test result: \w+\. (\d+) passed; (\d+) failed'
    $detail = if ($tests) {
        $passed = ($tests | ForEach-Object { [int]$_.Matches[0].Groups[1].Value } | Measure-Object -Sum).Sum
        $failed = ($tests | ForEach-Object { [int]$_.Matches[0].Groups[2].Value } | Measure-Object -Sum).Sum
        "$passed tests passed, $failed failed"
    } else {
        ($lines | Select-String -Pattern '^(RESULT:|FAIL:|error(\[|:)|warning:)' |
            Select-Object -First 3 | ForEach-Object { $_.Line.Trim() }) -join ' | '
    }
    if (-not $exited) {
        Add-Result $Name 'FAIL' $seconds "timed out after $TimeoutSec s"
    } elseif ($p.ExitCode -ne 0) {
        Add-Result $Name 'FAIL' $seconds "exit $($p.ExitCode): $detail"
    } else {
        Add-Result $Name 'PASS' $seconds $detail
    }
    return $text
}


# A nearly full drive fails the build mid-link (LNK1318) after many minutes: stop at once instead.
$drive = Get-PSDrive -Name (Split-Path -Qualifier $target).TrimEnd(':') -ErrorAction SilentlyContinue
if ($drive -and $drive.Free -lt 20GB) {
    $free = [math]::Round($drive.Free / 1GB, 1)
    Write-Host "FAIL: only $free GB free on $($drive.Name): (builds need about 20 GB). Free space, or run 'cargo clean --profile dev' (debug builds only)."
    exit 1
}

# ---------------------------------------------------------------- quick
if ($Tier -notin 'ui', 'manual') {
    Invoke-Step 'format' $cargo @('fmt', '--all', '--check') 120 | Out-Null
    Invoke-Step 'build' $cargo @('build', '--workspace', '--all-targets') 1200 | Out-Null
    # The manual (docs/manual): every page, heading, screenshot, highlight and action it links
    # to must exist. Writes nothing; `cargo run -p manual` builds out/manual.
    Invoke-Step 'manual' $cargo @('run', '-q', '-p', 'manual', '--', '--check', '--require-examples') 300 | Out-Null
    Invoke-Step 'tests' $cargo @('test', '--workspace') 900 | Out-Null
    Invoke-Step 'clippy' $cargo @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings') 900 | Out-Null
}

# ---------------------------------------------------------------- full, live, ui
# The maintainer's own tiers, kept outside the repository (see the hook above).
$shots = @()
if ($LocalTiers) { . $LocalTiers }

# ---------------------------------------------------------------- manual
if ($Tier -eq 'manual') {
    Invoke-Step 'build-app' $cargo @('build', '--release', '-p', 'daw-app') 1800 | Out-Null
    Invoke-Step 'build-manual' $cargo @('build', '--release', '-p', 'manual') 900 | Out-Null
    if (-not $Audio) {
        # Without audio, a generated plucked guitar (dawpoc gen --guitar, 28.1).
        Invoke-Step 'build-dawpoc' $cargo @('build', '--release', '-p', 'dawpoc') 900 | Out-Null
        Invoke-Step 'test-signal' "$release/dawpoc.exe" @('gen', '--guitar', '--seconds', '6', '--sample-rate', '44100',
            '--output', 'out/check/demo-guitar.wav') 120 | Out-Null
        $Audio = 'out/check/demo-guitar.wav'
    }
    # The shots: docs/manual/shots.toml, each from the release app with --neutral (no device
    # opened, no machine's names drawn) into docs/manual/shots, where they are committed (28.1:
    # builds and the release use them as they are; a retake shows in git status for review).
    # scripts/manual_shots.ps1; each shot's log in out/manual-shots-work/logs.
    $manualExe = "$release/manual.exe"
    $shotDir = Join-Path $root 'docs/manual/shots'
    $pwsh = (Get-Process -Id $PID).Path
    $shotArgs = @('-NoProfile', '-File', 'scripts/manual_shots.ps1', '-Exe', "$release/daw.exe",
        '-ManualExe', $manualExe, '-Audio', $Audio, '-ShotDir', 'docs/manual/shots')
    # The app's own arguments for every shot (the instrument a scenario needs), when given.
    if ($LocalAppArgs) { $shotArgs += @('-AppArgs', ($LocalAppArgs -join "`t")) }
    Invoke-Step 'manual-shots' $pwsh $shotArgs 900 | Out-Null
    # The manual from those shots: a missing shot, or a highlight on a mark the app didn't draw,
    # fails here.
    Invoke-Step 'manual-build' $manualExe @('--shots', $shotDir, '--strict', '--require-examples') 600 | Out-Null
    $node = Get-Command node -ErrorAction SilentlyContinue
    $browsers = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
        "$env:ProgramFiles\Google\Chrome\Application\chrome.exe", "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
        "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe") | Where-Object { Test-Path $_ }
    $skip = if (-not $node) { 'no Node.js on the PATH' } elseif (-not $browsers) { 'neither Edge nor Chrome is installed' } else { '' }
    $pwsh = (Get-Process -Id $PID).Path
    Invoke-Step 'manual-browser' $pwsh @('-NoProfile', '-File', 'scripts/manual_browser_check.ps1', '-Manual', 'out/manual', '-OutDir', 'out/manual-check') 600 -Skip $skip | Out-Null
}

# ---------------------------------------------------------------- report
$failed = @($results | Where-Object Status -eq 'FAIL')
$overall = if ($failed.Count) { 'FAIL' } else { 'PASS' }
$commit = (git rev-parse --short HEAD 2>$null)
$dirty = if (git status --porcelain 2>$null) { ' + uncommitted changes' } else { '' }
$report = @(
    "# Check report",
    "",
    "- Tier: **$Tier**",
    "- Result: **$overall** ($($failed.Count) failed of $($results.Count) steps)",
    "- Code: ``$commit``$dirty",
    "- Started: $($started.ToString('yyyy-MM-dd HH:mm:ss')), took $([int]((Get-Date) - $started).TotalSeconds) s",
    "",
    "| step | status | seconds | detail |",
    "|---|---|---|---|"
)
$report += $results | ForEach-Object {
    "| $($_.Step) | $($_.Status) | $('{0:N1}' -f $_.Seconds) | $($_.Detail -replace '\|', '/') |"
}
if ($shots) {
    $report += '', '## Screenshots', ''
    $report += $shots | ForEach-Object { "- ``$((Resolve-Path -Relative $_))``" }
}
$report += '', "Logs: ``out/check/logs/<step>.log``"
Set-Content -Path (Join-Path $outDir 'report.md') -Value ($report -join "`n")

Write-Host ""
Write-Host "Overall: $overall  (report: out/check/report.md)" -ForegroundColor $(if ($failed.Count) { 'Red' } else { 'Green' })
exit $(if ($failed.Count) { 1 } else { 0 })
