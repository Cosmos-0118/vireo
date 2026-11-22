# Makes the MSVC toolchain (cl.exe / link.exe) available in the current PowerShell session.
# Rust crates that compile C code (ring, vswhom-sys, ...) and the MSVC linker need it.
#
#   . .\msvc-env.ps1; Enter-MsvcEnv    # then plain `cargo build` / `cargo check` just works
#
# No-op when cl.exe is already on PATH (e.g. inside a Developer PowerShell).

function Find-Vcvars64 {
    $pf86 = ${env:ProgramFiles(x86)}

    # Preferred: ask the Visual Studio installer for the newest install that has the C++ tools.
    $vswhere = Join-Path $pf86 "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $install = & $vswhere -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>$null | Select-Object -First 1
        if ($install) {
            $bat = Join-Path $install "VC\Auxiliary\Build\vcvars64.bat"
            if (Test-Path $bat) { return $bat }
        }
    }

    # Fallback: scan the usual locations (any VS version / edition), newest first.
    $roots = @($env:ProgramFiles, $pf86) | Where-Object { $_ } |
        ForEach-Object { Join-Path $_ "Microsoft Visual Studio" }
    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        $hit = Get-ChildItem -Path $root -Filter vcvars64.bat -Recurse -Depth 5 -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | Select-Object -First 1
        if ($hit) { return $hit.FullName }
    }
    return $null
}

function Enter-MsvcEnv {
    if (Get-Command cl.exe -ErrorAction SilentlyContinue) { return }

    $vcvars = Find-Vcvars64
    if (-not $vcvars) {
        throw @"
MSVC C++ build tools were not found.
Install Microsoft C++ Build Tools: https://visualstudio.microsoft.com/visual-cpp-build-tools/
In the installer, select 'Desktop development with C++', including the MSVC x64/x86 tools and a Windows SDK.
If Visual Studio is already installed, use Visual Studio Installer > Modify to add that workload.
Then open a new PowerShell window and retry. See README.md > Windows prerequisites.
"@
    }

    # Run vcvars64 in cmd, dump the resulting environment, and import it into this session.
    $dump = cmd.exe /c "`"$vcvars`" >nul 2>&1 && set"
    foreach ($line in $dump) {
        if ($line -match '^([^=]+)=(.*)$') {
            Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2]
        }
    }

    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        throw "vcvars64.bat ran but cl.exe is still not on PATH ($vcvars)."
    }
}
