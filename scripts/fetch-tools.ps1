# Downloads the pinned third-party tools in scripts/bundled-tools.json, verifies each SHA256, and
# lays them out under <Output>/tools:
#   tools/git/...                MinGit (vireo/vx put tools/git/cmd on PATH)
#   tools/bin/rg.exe, gh.exe     single-file tools (tools/bin on PATH)
#   tools/licenses/<name>/...    upstream license files
#   tools/THIRD-PARTY-NOTICES.md versions, licenses and source links
#
#   pwsh scripts/fetch-tools.ps1 -Output artifacts/publish
#   pwsh scripts/fetch-tools.ps1 -Output backend/target/release   # use bundled tools in a dev build
#
# Downloads are cached in -Cache (default artifacts/tools-cache) and re-verified on every run.

param(
    [Parameter(Mandatory)][string]$Output,
    [string]$Cache = 'artifacts/tools-cache',
    [string]$Manifest = (Join-Path $PSScriptRoot 'bundled-tools.json')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is many times slower with the progress bar

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
function Resolve-RepoPath([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return $Path }
    return Join-Path $repoRoot $Path
}

$outDir = Resolve-RepoPath $Output
$cacheDir = Resolve-RepoPath $Cache
$toolsDir = Join-Path $outDir 'tools'
New-Item -ItemType Directory -Force $cacheDir | Out-Null
if (Test-Path $toolsDir) { Remove-Item $toolsDir -Recurse -Force }
New-Item -ItemType Directory -Force $toolsDir | Out-Null

$spec = Get-Content $Manifest -Raw | ConvertFrom-Json

function Get-Sha256([string]$Path) {
    (Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Copies $From to $To, refusing any path that would land outside $Root (zip-slip style entries).
function Copy-Inside([string]$From, [string]$Root, [string]$Relative) {
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $dest = [IO.Path]::GetFullPath((Join-Path $Root $Relative))
    if (-not $dest.StartsWith($rootFull, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to write outside ${Root}: $Relative"
    }
    if (-not (Test-Path $From)) { throw "Archive is missing expected entry: $From" }
    New-Item -ItemType Directory -Force (Split-Path $dest -Parent) | Out-Null
    Copy-Item $From -Destination $dest -Recurse -Force
}

$notes = @(
    '# Third-party tools bundled with Vireo',
    '',
    'Vireo ships these unmodified upstream builds so it works without extra installs. A copy of the same tool already on your PATH is used first.',
    'Each tool is distributed under its own license; the full texts are in `licenses/<name>/`.',
    ''
)

foreach ($tool in $spec.tools) {
    $name = $tool.name
    $archive = Join-Path $cacheDir ([IO.Path]::GetFileName(([Uri]$tool.url).AbsolutePath))
    $want = $tool.sha256.ToLowerInvariant()

    if ((Test-Path $archive) -and (Get-Sha256 $archive) -ne $want) {
        Write-Host "Cached $name archive is stale or corrupt; downloading again."
        Remove-Item $archive -Force
    }
    if (-not (Test-Path $archive)) {
        Write-Host "Downloading $name $($tool.version)..."
        $partial = "$archive.partial"
        Invoke-WebRequest -Uri $tool.url -OutFile $partial -UseBasicParsing -MaximumRetryCount 3 -RetryIntervalSec 5
        $got = Get-Sha256 $partial
        if ($got -ne $want) {
            Remove-Item $partial -Force
            throw "SHA256 mismatch for $name ($($tool.url)): expected $want, got $got"
        }
        Move-Item $partial $archive -Force
    }

    $extract = Join-Path ([IO.Path]::GetTempPath()) ("vireo-tool-$name-" + [guid]::NewGuid().ToString('N'))
    try {
        Expand-Archive -Path $archive -DestinationPath $extract -Force

        if ($tool.PSObject.Properties['tree']) {
            # Whole archive becomes tools/<tree>, minus files Vireo never needs.
            $treeDir = Join-Path $toolsDir $tool.tree
            Copy-Inside $extract $toolsDir $tool.tree
            $remove = if ($tool.PSObject.Properties['remove']) { @($tool.remove) } else { @() }
            foreach ($rel in $remove) {
                $victim = Join-Path $treeDir $rel
                if (Test-Path $victim) { Remove-Item $victim -Recurse -Force }
            }
        }
        if ($tool.PSObject.Properties['files']) {
            foreach ($entry in $tool.files.PSObject.Properties) {
                Copy-Inside (Join-Path $extract $entry.Name) $toolsDir $entry.Value
            }
        }
        foreach ($lic in @($tool.licenses)) {
            Copy-Inside (Join-Path $extract $lic) $toolsDir (Join-Path "licenses/$name" (Split-Path $lic -Leaf))
        }
    } finally {
        Remove-Item $extract -Recurse -Force -ErrorAction SilentlyContinue
    }

    # The staged tool must actually start (catches a wrong layout before it reaches a release).
    $exe = Join-Path $toolsDir $tool.check[0]
    $checkArgs = @($tool.check | Select-Object -Skip 1)
    $version = (& $exe @checkArgs 2>&1 | Select-Object -First 1)
    if ($LASTEXITCODE -ne 0) { throw "$name failed its self-check ($exe): $version" }
    Write-Host "  $name OK: $version"

    $notes += "## $($tool.title) $($tool.version)"
    $notes += ''
    $notes += "- License: $($tool.license)"
    $notes += "- Source: $($tool.source)"
    $notes += "- Binary: $($tool.url) (SHA256 ``$want``)"
    $notes += ''
}

Set-Content -Path (Join-Path $toolsDir 'THIRD-PARTY-NOTICES.md') -Value ($notes -join "`n") -Encoding utf8

$bytes = (Get-ChildItem $toolsDir -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("Bundled tools staged in {0} ({1:N1} MB on disk)" -f $toolsDir, ($bytes / 1MB))
