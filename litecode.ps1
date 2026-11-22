param(
    [string]$Dir = (Get-Location).Path,
    [int]$Port = 4317,
    [switch]$NoBrowser
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$ReleaseExe = Join-Path $Root "backend\target\release\litecode-server.exe"
$DebugExe = Join-Path $Root "backend\target\debug\litecode-server.exe"
$DistDir = Join-Path $Root "frontend\dist"

if (Test-Path $ReleaseExe) {
    $Exe = $ReleaseExe
} elseif (Test-Path $DebugExe) {
    $Exe = $DebugExe
} else {
    Write-Host "Building LiteCode server..." -ForegroundColor Cyan
    $vcvars = "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat"
    $manifest = Join-Path $Root "backend\Cargo.toml"
    if (-not (Get-Command link.exe -ErrorAction SilentlyContinue) -and (Test-Path $vcvars)) {
        cmd.exe /c "`"$vcvars`" && cargo build --release --manifest-path `"$manifest`""
    } else {
        cargo build --release --manifest-path $manifest
    }
    $Exe = $ReleaseExe
}

$Url = "http://127.0.0.1:$Port"
Write-Host "Starting LiteCode at $Url for workspace: $Dir" -ForegroundColor Green

if (-not $NoBrowser) {
    $edge = Get-Command msedge.exe -ErrorAction SilentlyContinue
    if (-not $edge -and (Test-Path "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")) {
        $edge = "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
    }
    Start-Job -ScriptBlock {
        param($u, $e)
        Start-Sleep -Milliseconds 800
        if ($e) {
            Start-Process $e "--app=$u"
        } else {
            Start-Process $u
        }
    } -ArgumentList $Url, $edge | Out-Null
}

& $Exe --port $Port --dir $Dir --static $DistDir
