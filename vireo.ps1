param(
    [Parameter(Position = 0)]
    [string]$Dir = (Get-Location).Path
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$ReleaseExe = Join-Path $Root "backend\target\release\vireo.exe"
$DebugExe = Join-Path $Root "backend\target\debug\vireo.exe"

if (Test-Path $ReleaseExe) {
    $Exe = $ReleaseExe
} elseif (Test-Path $DebugExe) {
    $Exe = $DebugExe
} else {
    & (Join-Path $Root "build-and-run.ps1") -Dir $Dir
    exit $LASTEXITCODE
}

Write-Host "Launching Vireo for workspace: $Dir" -ForegroundColor Green
Start-Process -FilePath $Exe -ArgumentList @("--dir", $Dir)
