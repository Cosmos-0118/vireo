# Stages the release binaries and required files into a clean publish folder and validates the layout.
#   pwsh scripts/stage-release.ps1 -Output artifacts/publish

param(
    [string]$ReleaseDir = 'backend/target/release',
    [string]$Output = 'artifacts/publish'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$srcDir = if ([IO.Path]::IsPathRooted($ReleaseDir)) { $ReleaseDir } else { Join-Path $repoRoot $ReleaseDir }
$outDir = if ([IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $repoRoot $Output }

if (Test-Path $outDir) {
    Remove-Item $outDir -Recurse -Force
}
New-Item -ItemType Directory -Force $outDir | Out-Null

$requiredBins = @('vireo.exe', 'vx.exe')
foreach ($bin in $requiredBins) {
    $src = Join-Path $srcDir $bin
    if (-not (Test-Path $src -PathType Leaf)) {
        throw "Missing release binary: $src (build with `cargo build --release --bins --manifest-path backend/Cargo.toml`)"
    }
    Copy-Item $src -Destination (Join-Path $outDir $bin) -Force
}

$license = Join-Path $repoRoot 'LICENSE'
if (-not (Test-Path $license -PathType Leaf)) {
    throw "Missing LICENSE at $license"
}
Copy-Item $license -Destination (Join-Path $outDir 'LICENSE') -Force

# Validate staged layout
foreach ($name in @('vireo.exe', 'vx.exe', 'LICENSE')) {
    $staged = Join-Path $outDir $name
    if (-not (Test-Path $staged -PathType Leaf) -or (Get-Item $staged).Length -eq 0) {
        throw "Staged artifact is missing or empty: $staged"
    }
}

Write-Host "Staged release layout in ${outDir}:"
Get-ChildItem $outDir | Select-Object Name, @{ Name = 'SizeMB'; Expression = { [math]::Round($_.Length / 1MB, 2) } } | Format-Table -AutoSize
