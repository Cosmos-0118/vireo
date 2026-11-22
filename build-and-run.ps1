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

# MSVC toolchain (cl.exe / link.exe) is required by the Rust build, including `cargo clean`.
. (Join-Path $Root "msvc-env.ps1")
Enter-MsvcEnv

# Check Rust before stopping the app or removing any build artifacts.
# Existing terminals may still have the PATH from before rustup was installed.
if (-not (Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue) -or
    -not (Get-Command rustc -CommandType Application -ErrorAction SilentlyContinue)) {
    $CargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $CargoBin = Join-Path $CargoHome 'bin'
    if (Test-Path -LiteralPath (Join-Path $CargoBin 'cargo.exe') -PathType Leaf) {
        $env:PATH = "$CargoBin;$env:PATH"
    }
}

foreach ($tool in @('cargo', 'rustc')) {
    if (-not (Get-Command $tool -CommandType Application -ErrorAction SilentlyContinue)) {
        throw "Rust tool '$tool' was not found on PATH. Install Rust using https://rustup.rs/ (the MSVC toolchain), then open a new PowerShell window and retry. See README.md > Windows prerequisites."
    }
}

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
# Reinstall only when package-lock.json is newer than what node_modules was installed from.
$NpmStamp = Join-Path $FrontendDir "node_modules\.package-lock.json"
$NpmLock = Join-Path $FrontendDir "package-lock.json"
if (-not (Test-Path $NpmStamp) -or (Get-Item $NpmLock).LastWriteTime -gt (Get-Item $NpmStamp).LastWriteTime) {
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
cargo build --release --manifest-path $ManifestPath
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
