# Builds Vireo-Setup-<version>.exe from a staged publish folder.
#   pwsh installer/build.ps1 -Version 0.2.0 -PublishDir artifacts/publish
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup, or choco install innosetup --version=6.7.1, the version CI uses).
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$PublishDir,
    [string]$OutDir = 'artifacts'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Inno Setup stamps VersionInfoVersion as <Version>.0, which requires three numeric parts.
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Version '$Version' must be three numbers, such as 0.2.0."
}

$candidates = @(
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
)
$iscc = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) {
    $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($cmd) { $iscc = $cmd.Source }
}
if (-not $iscc) {
    throw 'Inno Setup 6 (ISCC.exe) was not found.'
}

$publish = (Resolve-Path $PublishDir).Path
foreach ($path in 'vireo.exe', 'vx.exe', 'LICENSE', 'tools\git\cmd\git.exe', 'tools\bin\rg.exe', 'tools\bin\gh.exe') {
    if (-not (Test-Path (Join-Path $publish $path))) {
        throw "The publish folder is missing $path."
    }
}

New-Item -ItemType Directory -Force $OutDir | Out-Null
$out = (Resolve-Path $OutDir).Path

& $iscc /Qp "/DMyAppVersion=$Version" "/DBuildOutput=$publish" "/O$out" (Join-Path $PSScriptRoot 'Vireo.iss')
if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE."
}

$setup = Join-Path $out "Vireo-Setup-$Version.exe"
if (-not (Test-Path $setup)) {
    throw "Expected $setup was not produced."
}

Write-Host "Built installer: $setup"
$setup
