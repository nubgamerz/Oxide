<#
.SYNOPSIS
    Writes THIRD-PARTY.txt: every library Oxide's programs (daw.exe, oxide-mcp.exe) bundle or
    link, with its licence and the licence texts it ships, for the installer (dev plan 19.1).

.DESCRIPTION
    Nothing is downloaded. The Rust crates are the normal dependencies of daw-app on Windows
    (`cargo tree -e normal`: no build or dev tools), each with the licence its manifest
    declares and the LICENSE/COPYING/NOTICE files found in its source folder (the registry's
    copy, or vendor/ for the patched ones); identical texts are printed once. The C and C++
    libraries compiled into the program (LAME, libvorbis and libogg, Signalsmith Stretch, the
    VST3 and ASIO SDKs) are listed first with the texts their sources carry.

    Prints how many crates were covered and which shipped no licence file (named by licence
    only), so the gap is visible.

.EXAMPLE
    pwsh scripts/third_party.ps1 -Out out/dist/stage/THIRD-PARTY.txt -Version 0.2.0
#>
param(
    [Parameter(Mandatory)] [string]$Out,
    [string]$Version = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$target = 'x86_64-pc-windows-msvc'

# The crates in the programs: "name vX.Y.Z" from cargo tree (normal edges only).
$tree = & cargo tree -p daw-app -e normal --prefix none --format '{p}' --target $target 2>$null
if ($LASTEXITCODE -ne 0) { throw 'cargo tree failed' }
$wanted = @{}
foreach ($line in $tree) {
    if ($line -match '^(\S+) v(\S+)') { $wanted["$($Matches[1]) $($Matches[2])"] = $true }
}

# Where each crate's source is, and its declared licence.
$metadata = (& cargo metadata --format-version 1 --filter-platform $target 2>$null) -join "`n" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$workspace = @($metadata.workspace_members)
$crates = $metadata.packages |
    Where-Object { $wanted.ContainsKey("$($_.name) $($_.version)") -and $workspace -notcontains $_.id } |
    Sort-Object name, version

$licenceName = '^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|UNLICENSE|AUTHORS)([-._].*)?$'
function Read-Licences([string]$Dir, [string]$Pattern = $licenceName) {
    Get-ChildItem -LiteralPath $Dir -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match $Pattern -and $_.Length -lt 200KB } |
        Sort-Object Name |
        ForEach-Object { [pscustomobject]@{ Name = $_.Name; Text = ((Get-Content -LiteralPath $_.FullName -Raw) -replace "`r`n", "`n").TrimEnd() } }
}

$text = [System.Text.StringBuilder]::new()
function Line([string]$s = '') { [void]$text.Append($s).Append("`r`n") }
function Block([string]$s) { foreach ($l in ($s -split "`n")) { Line $l.TrimEnd() } }
$rule = '=' * 78

$title = if ($Version) { "Oxide $Version" } else { 'Oxide' }
Line "${title}: third-party software"
Line $rule
Line
Line 'Oxide itself is free software under the GNU General Public License version 3'
Line '(LICENSE.txt; NOTICE.txt has its one added term, on the name "Oxide" and its logo). Its'
Line 'complete source is at https://github.com/nubgamerz/Oxide.'
Line
Line 'Oxide includes the libraries below. Each is used under the licence named; the texts'
Line 'their sources carry follow. Trademarks: VST and ASIO are trademarks of Steinberg Media'
Line 'Technologies GmbH.'
Line

# ---------------------------------------------------------------- C and C++
function Find-Crate([string]$Name) {
    $crate = $crates | Where-Object name -EQ $Name | Select-Object -First 1
    if ($crate) { Split-Path -Parent $crate.manifest_path }
}

Line 'Libraries in C and C++ compiled into Oxide'
Line $rule
$native = @(
    @{ Name = 'LAME 3.100 (MP3 encoding), through mp3lame-sys'; Licence = 'LGPL (GNU Library General Public License 2; the crate declares LGPL-3.0)'; Dir = { $d = Find-Crate 'mp3lame-sys'; if ($d) { Join-Path $d 'lame-3.100' } }; Pattern = '^(COPYING|LICENSE)$'
       Note = 'LAME is statically linked. Its source is in the mp3lame-sys crate (crates.io, version 0.1.11, folder lame-3.100); see https://lame.sourceforge.io. Oxide''s complete source is public under the GPL version 3, so Oxide can be rebuilt with a changed LAME.' },
    @{ Name = 'libvorbis with aoTuV and Lancer (Ogg Vorbis encoding), through aotuv_lancer_vorbis_sys'; Licence = 'BSD-3-Clause'; Dir = { $d = Find-Crate 'aotuv_lancer_vorbis_sys'; if ($d) { Join-Path $d 'vorbis_vendor' } }; Pattern = '^COPYING$' },
    @{ Name = 'libogg, through aotuv_lancer_vorbis_sys'; Licence = 'BSD-3-Clause'; Dir = { $d = Find-Crate 'aotuv_lancer_vorbis_sys'; if ($d) { Join-Path $d 'ogg_vendor' } }; Pattern = '^COPYING$' },
    @{ Name = 'Signalsmith Stretch (time-stretch and pitch-shift), vendored with a fixed seed'; Licence = 'MIT'; Dir = { Join-Path $root 'vendor/signalsmith-stretch' }; Pattern = '^LICENSE.*' },
    @{ Name = 'Steinberg ASIO SDK 2.3 (ASIO devices), through asio-sys'; Licence = 'GNU General Public License version 3 (the SDK is dual-licensed: the Steinberg ASIO License, or alternatively the GPL version 3; Oxide uses the GPL option)'; Dir = { $null }
       Note = 'ASIO SDK 2.3.4 or later (the first with the GPL option), fetched by asio-sys from www.steinberg.net/asiosdk when Oxide is built and compiled in. ASIO is a trademark of Steinberg Media Technologies GmbH. Its host helpers (asiolist.cpp, ASIOConvertSamples.cpp) carry the licence below.'
       Text = @'
(c) 2025, Steinberg Media Technologies GmbH, All Rights Reserved

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

  * Redistributions of source code must retain the above copyright notice,
    this list of conditions and the following disclaimer.
  * Redistributions in binary form must reproduce the above copyright notice,
    this list of conditions and the following disclaimer in the documentation
    and/or other materials provided with the distribution.
  * Neither the name of the Steinberg Media Technologies nor the names of its
    contributors may be used to endorse or promote products derived from this
    software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE
OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED
OF THE POSSIBILITY OF SUCH DAMAGE.
'@ }
)
foreach ($n in $native) {
    Line
    Line "$($n.Name)"
    Line "Licence: $($n.Licence)"
    if ($n.Note) { Line $n.Note }
    if ($n.Text) {
        Line
        Block $n.Text.TrimEnd()
    }
    $dir = & $n.Dir
    if ($dir -and (Test-Path $dir)) {
        foreach ($f in @(Read-Licences $dir $n.Pattern)) {
            Line
            Line "--- $($f.Name)"
            Block $f.Text
        }
    }
    Line
    Line ('-' * 78)
}

# ---------------------------------------------------------------- Rust
Line
Line 'Rust crates'
Line $rule
Line
foreach ($c in $crates) {
    $licence = if ($c.license) { $c.license } elseif ($c.license_file) { "see $($c.license_file)" } else { 'not declared' }
    Line ("{0,-40} {1,-22} {2}" -f $c.name, $c.version, $licence)
}
Line

# Each distinct text once, after the crates that carry it.
$groups = [ordered]@{}
$bare = [System.Collections.Generic.List[string]]::new()
foreach ($c in $crates) {
    $dir = Split-Path -Parent $c.manifest_path
    $files = @(Read-Licences $dir)
    if (-not $files.Count) {
        # Fonts carry theirs beside them (epaint_default_fonts: OFL, the Ubuntu font licence).
        $files = @(Get-ChildItem -LiteralPath $dir -Directory -ErrorAction SilentlyContinue |
            Where-Object Name -Match '^(fonts|licenses?)$' |
            ForEach-Object { Read-Licences $_.FullName '\.(txt|md)$|^(LICEN[CS]E|COPYING)' })
    }
    if ($c.license_file) {
        $path = Join-Path $dir $c.license_file
        if ((Test-Path -LiteralPath $path) -and -not ($files | Where-Object Name -EQ (Split-Path -Leaf $path))) {
            $files += [pscustomobject]@{ Name = (Split-Path -Leaf $path); Text = ((Get-Content -LiteralPath $path -Raw) -replace "`r`n", "`n").TrimEnd() }
        }
    }
    if (-not $files.Count) { $bare.Add("$($c.name) $($c.version) ($($c.license))"); continue }
    foreach ($f in $files) {
        if (-not $groups.Contains($f.Text)) { $groups[$f.Text] = [System.Collections.Generic.List[string]]::new() }
        $groups[$f.Text].Add("$($c.name) $($c.version) ($($f.Name))")
    }
}
Line 'Licence texts'
Line $rule
foreach ($body in $groups.Keys) {
    Line
    Line "Carried by: $($groups[$body] -join ', ')"
    Line ('-' * 78)
    Block $body
    Line ('-' * 78)
}
if ($bare.Count) {
    Line
    Line 'These crates ship no licence file; they are used under the licence they declare, whose'
    Line 'standard text is among those above:'
    foreach ($b in $bare) { Line "  $b" }
}

New-Item -ItemType Directory -Force (Split-Path -Parent ([IO.Path]::GetFullPath($Out))) | Out-Null
[IO.File]::WriteAllText([IO.Path]::GetFullPath($Out), $text.ToString(), [Text.UTF8Encoding]::new($false))
Write-Host ("THIRD-PARTY.txt: {0} crates, {1} distinct licence texts, {2} crates with no licence file{3}" -f `
    $crates.Count, $groups.Count, $bare.Count, $(if ($bare.Count) { ": $($bare -join '; ')" } else { '' }))
