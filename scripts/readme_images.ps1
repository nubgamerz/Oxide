<#
.SYNOPSIS
    Makes the README's images (docs/images/*.png) from the manual's committed screenshots, as
    docs/images/images.toml lists them (dev plan 28.3). Prints a line per image and
    `RESULT: PASS`, or `FAIL:` and exits 1.

.DESCRIPTION
    Each image is a shot from docs/manual/shots (neutral, taken with --neutral by
    scripts/manual_shots.ps1), cut to marks or a rect, scaled down past its max_width and
    compressed by the manual's own encoder (`manual --readme-images`). It never runs the app or
    takes a shot: after a UI change that alters what a README image shows, retake that shot
    first (scripts/manual_shots.ps1 -Only <shot>, per the shots rule), then run this. The same
    shots give the same bytes; a file that would not change is left untouched, so `git status`
    shows only the pictures that moved. Over about 1.5 MB in all fails.

.EXAMPLE
    pwsh -NoProfile -File scripts/readme_images.ps1
#>
param(
    # The list; the images are written beside it.
    [string]$List = 'docs/images/images.toml'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    # The debug build the quick tier's `manual --check` already made: its dependencies (image,
    # png, color_quant) are optimised in dev builds, so this takes seconds.
    & cargo run -q -p manual -- --readme-images $List
    $code = $LASTEXITCODE
} finally {
    Pop-Location
}
if ($code -ne 0) {
    Write-Host "FAIL: manual --readme-images exited $code"
    exit 1
}
exit 0
