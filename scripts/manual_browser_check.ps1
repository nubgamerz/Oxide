<#
.SYNOPSIS
    The built manual in headless Edge and Chrome (scripts/manual_browser_check.js): search from
    file://, every figure's highlights on their marks, previous/next, dark, narrow, print.

.DESCRIPTION
    Runs the check in each browser installed (Edge, Chrome) with Node.js 22 or later. Prints
    the check's lines and a RESULT line per browser; exits 1 if any browser fails. With no
    Node.js or no browser it prints "RESULT: SKIP ..." and exits 0 (check.ps1 marks the step
    SKIP itself before calling this). Screenshots and the browsers' throwaway profiles go to
    -OutDir.

.EXAMPLE
    cargo run -p manual -- --strict
    pwsh scripts/manual_browser_check.ps1
#>
param(
    [string]$Manual = 'out/manual',
    [string]$OutDir = 'out/manual-check'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# The browsers it drives, where Windows installs them.
function Find-Browsers {
    $candidates = [ordered]@{
        edge   = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe")
        chrome = @("$env:ProgramFiles\Google\Chrome\Application\chrome.exe", "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe", "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe")
    }
    $found = [ordered]@{}
    foreach ($name in $candidates.Keys) {
        $exe = $candidates[$name] | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
        if ($exe) { $found[$name] = $exe }
    }
    $found
}

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Host 'RESULT: SKIP no Node.js on the PATH'
    exit 0
}
$browsers = Find-Browsers
if ($browsers.Count -eq 0) {
    Write-Host 'RESULT: SKIP neither Edge nor Chrome is installed'
    exit 0
}
if (-not (Test-Path (Join-Path $Manual 'index.html'))) {
    Write-Host "FAIL: no manual in $Manual (build it first: cargo run -p manual -- --strict)"
    exit 1
}
New-Item -ItemType Directory -Force $OutDir | Out-Null

$failed = 0
foreach ($name in $browsers.Keys) {
    & $node.Source (Join-Path $PSScriptRoot 'manual_browser_check.js') $browsers[$name] $name $Manual $OutDir
    if ($LASTEXITCODE -ne 0) { $failed++ }
}
exit $(if ($failed) { 1 } else { 0 })
