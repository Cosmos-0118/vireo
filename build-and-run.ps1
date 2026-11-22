param(
    [Parameter(Position = 0)]
    [string]$Dir = (Get-Location).Path,
    [switch]$CleanAll,
    [switch]$BuildOnly
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$FrontendDir = Join-Path $Root "frontend"
$BackendDir = Join-Path $Root "backend"
$ManifestPath = Join-Path $BackendDir "Cargo.toml"
$DistDir = Join-Path $FrontendDir "dist"
$ReleaseDir = Join-Path $BackendDir "target\release"
$ReleaseExe = Join-Path $ReleaseDir "vireo.exe"

Write-Host "============================================================" -ForegroundColor DarkCyan
Write-Host " Vireo - Native Desktop Build & Launch" -ForegroundColor Cyan
Write-Host " Workspace : $Dir" -ForegroundColor Gray
Write-Host "============================================================" -ForegroundColor DarkCyan

Write-Host "`n[1/5] Stopping any running vireo.exe instance..." -ForegroundColor Yellow
Get-Process -Name "vireo", "vireo-server" -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Host "[2/5] Cleaning previous build artifacts..." -ForegroundColor Yellow
if (Test-Path $DistDir) {
    Remove-Item -Recurse -Force $DistDir
}
if ($CleanAll) {
    Write-Host "      Running full cargo clean..." -ForegroundColor DarkGray
    cargo clean --manifest-path $ManifestPath
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} else {
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $ReleaseDir "vireo.*")
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $ReleaseDir "deps\vireo-*")
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $ReleaseDir "vireo-server.*")
}

Write-Host "[3/5] Building frontend production bundle..." -ForegroundColor Yellow
$FontPkg = Join-Path $FrontendDir "node_modules\@fontsource-variable\inter"
if (-not (Test-Path $FontPkg)) {
    & npm.cmd --prefix $FrontendDir install
    if ($LASTEXITCODE -ne 0) {
        Write-Error "npm install failed."
        exit $LASTEXITCODE
    }
}
& npm.cmd --prefix $FrontendDir run build
if ($LASTEXITCODE -ne 0) {
    Write-Error "Frontend build failed."
    exit $LASTEXITCODE
}

Write-Host "[4/5] Building native Tauri desktop binary (release)..." -ForegroundColor Yellow
$vcVars = $null
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    $candidates = @(
        "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\18\Professional\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
    )
    $vcVars = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}

if ($vcVars) {
    cmd.exe /c "call `"$vcVars`" >nul && cargo build --release --manifest-path `"$ManifestPath`""
} else {
    cargo build --release --manifest-path $ManifestPath
}
if ($LASTEXITCODE -ne 0) {
    Write-Error "Rust release build failed."
    exit $LASTEXITCODE
}

if ($BuildOnly) {
    Write-Host "`nBuild complete: $ReleaseExe" -ForegroundColor Green
    exit 0
}

Write-Host "`n[5/5] Launching Vireo desktop app..." -ForegroundColor Green
Start-Process -FilePath $ReleaseExe -ArgumentList @("--dir", $Dir)
