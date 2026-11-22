@echo off
setlocal enabledelayedexpansion

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

set "TARGET_DIR=%CD%"
set "PORT=4317"
set "CLEAN_ALL=0"
set "NO_BROWSER=0"
set "DIR_SET=0"

:parse_args
if "%~1"=="" goto args_done
if /I "%~1"=="--port" (
    set "PORT=%~2"
    shift
    shift
    goto parse_args
)
if /I "%~1"=="-p" (
    set "PORT=%~2"
    shift
    shift
    goto parse_args
)
if /I "%~1"=="--clean-all" (
    set "CLEAN_ALL=1"
    shift
    goto parse_args
)
if /I "%~1"=="--no-browser" (
    set "NO_BROWSER=1"
    shift
    goto parse_args
)
if "%DIR_SET%"=="0" (
    set "TARGET_DIR=%~1"
    set "DIR_SET=1"
)
shift
goto parse_args

:args_done
echo ============================================================
echo  Vireo - Clean Production Build ^& Launch
echo  Workspace : %TARGET_DIR%
echo  Port      : %PORT%
echo ============================================================

echo.
echo [1/5] Stopping any running vireo-server instance...
taskkill /F /IM vireo-server.exe >nul 2>&1

echo [2/5] Cleaning previous build artifacts...
if exist "%ROOT%\frontend\dist" (
    rmdir /S /Q "%ROOT%\frontend\dist"
)
if "%CLEAN_ALL%"=="1" (
    echo       Running full cargo clean...
    cargo clean --manifest-path "%ROOT%\backend\Cargo.toml"
    if errorlevel 1 exit /b 1
) else (
    del /F /Q "%ROOT%\backend\target\release\vireo-server.*" >nul 2>&1
    del /F /Q "%ROOT%\backend\target\release\deps\vireo_server*" >nul 2>&1
)

echo [3/5] Building frontend production bundle...
if not exist "%ROOT%\frontend\node_modules" (
    call npm.cmd --prefix "%ROOT%\frontend" install
    if errorlevel 1 (
        echo ERROR: npm install failed.
        exit /b 1
    )
)
call npm.cmd --prefix "%ROOT%\frontend" run build
if errorlevel 1 (
    echo ERROR: Frontend build failed.
    exit /b 1
)

echo [4/5] Building Rust backend (release)...
where link.exe >nul 2>&1
if errorlevel 1 (
    set "VCVARS="
    for %%V in (18 2022 2019) do (
        for %%E in (Community Professional Enterprise BuildTools) do (
            if not defined VCVARS (
                if exist "C:\Program Files\Microsoft Visual Studio\%%V\%%E\VC\Auxiliary\Build\vcvars64.bat" (
                    set "VCVARS=C:\Program Files\Microsoft Visual Studio\%%V\%%E\VC\Auxiliary\Build\vcvars64.bat"
                )
            )
            if not defined VCVARS (
                if exist "C:\Program Files (x86)\Microsoft Visual Studio\%%V\%%E\VC\Auxiliary\Build\vcvars64.bat" (
                    set "VCVARS=C:\Program Files (x86)\Microsoft Visual Studio\%%V\%%E\VC\Auxiliary\Build\vcvars64.bat"
                )
            )
        )
    )
    if defined VCVARS (
        call "!VCVARS!" >nul
    )
)
cargo build --release --manifest-path "%ROOT%\backend\Cargo.toml"
if errorlevel 1 (
    echo ERROR: Rust backend build failed.
    exit /b 1
)

set "URL=http://127.0.0.1:%PORT%"
echo.
echo [5/5] Starting Vireo at %URL% ...

if "%NO_BROWSER%"=="0" (
    set "EDGE="
    if exist "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe" (
        set "EDGE=C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
    ) else if exist "C:\Program Files\Microsoft\Edge\Application\msedge.exe" (
        set "EDGE=C:\Program Files\Microsoft\Edge\Application\msedge.exe"
    )
    if defined EDGE (
        start "" /B powershell -NoProfile -Command "Start-Sleep -Milliseconds 900; Start-Process '!EDGE!' '--app=%URL%'"
    ) else (
        start "" /B powershell -NoProfile -Command "Start-Sleep -Milliseconds 900; Start-Process '%URL%'"
    )
)

"%ROOT%\backend\target\release\vireo-server.exe" --port %PORT% --dir "%TARGET_DIR%" --static "%ROOT%\frontend\dist"
