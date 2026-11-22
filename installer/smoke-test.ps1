# Installs Vireo-Setup silently into a scratch folder, checks that vireo.exe, vx.exe and the
# bundled tools (git, rg, gh) arrived and respond properly, then uninstalls and verifies everything is cleaned up.
#   pwsh installer/smoke-test.ps1 -Setup artifacts/Vireo-Setup-0.0.0.exe

param(
    [Parameter(Mandatory)][string]$Setup
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$dir = Join-Path ([IO.Path]::GetTempPath()) ("vireo-smoke-" + [guid]::NewGuid().ToString('N'))
$workspace = Join-Path ([IO.Path]::GetTempPath()) ("vireo-smoke-ws-" + [guid]::NewGuid().ToString('N'))
$vireoExe = Join-Path $dir 'vireo.exe'
$vxExe = Join-Path $dir 'vx.exe'
$uninstaller = Join-Path $dir 'unins000.exe'
$quiet = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')

function Invoke-Installer([string]$Path, [string[]]$Arguments, [string]$What, [int]$TimeoutSeconds = 300) {
    $p = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru
    if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        throw "$What did not finish within $TimeoutSeconds seconds."
    }
    if ($p.ExitCode -ne 0) {
        throw "$What exited with $($p.ExitCode)."
    }
}

function Wait-Gone([string]$Path, [int]$TimeoutSeconds = 30) {
    for ($i = 0; $i -lt $TimeoutSeconds -and (Test-Path $Path); $i++) {
        Start-Sleep -Seconds 1
    }
    return -not (Test-Path $Path)
}

$server = $null
try {
    New-Item -ItemType Directory -Force $workspace | Out-Null
    Set-Content -Path (Join-Path $workspace 'hello.rs') -Value "fn greet() -> &'static str { `"hi`" }`n" -Encoding utf8

    # /TASKS="" avoids mutating the runner's PATH during the smoke test.
    Invoke-Installer (Resolve-Path $Setup).Path ($quiet + @('/CURRENTUSER', '/TASKS=""', "/DIR=`"$dir`"")) 'Setup'

    foreach ($file in 'vireo.exe', 'vx.exe', 'LICENSE', 'unins000.exe') {
        if (-not (Test-Path (Join-Path $dir $file))) {
            throw "Not installed: $file"
        }
    }
    Write-Host "Installed to $dir"

    # Bundled third-party tools must be present and runnable (a clean machine relies on them).
    foreach ($tool in @(
            @{ Path = 'tools\git\cmd\git.exe'; Expect = 'git version' },
            @{ Path = 'tools\bin\rg.exe'; Expect = 'ripgrep' },
            @{ Path = 'tools\bin\gh.exe'; Expect = 'gh version' }
        )) {
        $exe = Join-Path $dir $tool.Path
        if (-not (Test-Path $exe)) { throw "Bundled tool not installed: $($tool.Path)" }
        # Capture all output before trimming: `| Select-Object -First 1` stops the pipeline early,
        # so $LASTEXITCODE is never set and strict mode throws when it is read.
        $global:LASTEXITCODE = 0
        $verOut = @(& $exe --version 2>&1)
        $ver = $verOut | Select-Object -First 1
        if ($LASTEXITCODE -ne 0 -or "$ver" -notmatch [regex]::Escape($tool.Expect)) {
            throw "Bundled tool $($tool.Path) failed to run: $ver"
        }
        Write-Host "  $($tool.Path): $ver"
    }
    if (-not (Test-Path (Join-Path $dir 'tools\THIRD-PARTY-NOTICES.md'))) { throw 'Not installed: tools\THIRD-PARTY-NOTICES.md' }

    # Verify vx.exe runs cleanly against a scratch workspace.
    $prevWs = $env:VIREO_WORKSPACE
    try {
        $env:VIREO_WORKSPACE = $workspace
        # vx only honors VIREO_WORKSPACE when the shell is inside it (as in the real terminal);
        # otherwise it falls back to the enclosing git repo (the CI checkout).
        Push-Location $workspace
        try {
            $outline = & $vxExe outline 'hello.rs' 2>&1
        } finally {
            Pop-Location
        }
        if ($LASTEXITCODE -ne 0 -or ($outline -join "`n") -notmatch 'fn greet') {
            throw "vx smoke check failed (exit $LASTEXITCODE): $($outline -join ' ')"
        }
        Write-Host "vx.exe outline check passed."

        # Structural search runs in-process (no ast-grep CLI needed).
        Push-Location $workspace
        try {
            $astOut = & $vxExe ast '"hi"' -l rust 2>&1
        } finally {
            Pop-Location
        }
        if ($LASTEXITCODE -ne 0 -or ($astOut -join "`n") -notmatch '1 match') {
            throw "vx ast smoke check failed (exit $LASTEXITCODE): $($astOut -join ' ')"
        }
        Write-Host "vx.exe built-in ast check passed."
    } finally {
        if ($null -eq $prevWs) { Remove-Item Env:VIREO_WORKSPACE -ErrorAction SilentlyContinue }
        else { $env:VIREO_WORKSPACE = $prevWs }
    }

    # Verify vireo.exe starts in server-only mode and serves /api/health and the embedded index.html.
    $port = 14317
    $server = Start-Process -FilePath $vireoExe -ArgumentList @('--server-only', '--port', "$port", '--dir', $workspace) -WorkingDirectory $dir -PassThru
    $healthy = $false
    for ($i = 0; $i -lt 30; $i++) {
        if ($server.HasExited) {
            throw ("vireo.exe exited prematurely (exit code 0x{0:X8})." -f $server.ExitCode)
        }
        try {
            $resp = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/health" -TimeoutSec 2
            if ($resp -eq 'ok') {
                $healthy = $true
                break
            }
        } catch {
            Start-Sleep -Milliseconds 500
        }
    }
    if (-not $healthy) {
        throw "vireo.exe did not respond 'ok' on http://127.0.0.1:$port/api/health within 15 seconds."
    }
    $index = Invoke-WebRequest -Uri "http://127.0.0.1:$port/" -UseBasicParsing -TimeoutSec 5
    if ($index.StatusCode -ne 200 -or $index.Content -notmatch '<html') {
        throw "vireo.exe did not serve embedded frontend index.html (status $($index.StatusCode))."
    }
    Write-Host "vireo.exe health & embedded frontend check passed."

    Stop-Process -Id $server.Id -Force
    $server.WaitForExit(10000) | Out-Null
    $server = $null

    Invoke-Installer $uninstaller $quiet 'Uninstall'
    if (-not (Wait-Gone $vireoExe)) {
        throw 'vireo.exe is still present after uninstalling.'
    }
    Write-Host 'Uninstalled cleanly.'
}
finally {
    if ($server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
        $server.WaitForExit(5000) | Out-Null
    }
    foreach ($proc in Get-Process -Name 'vireo', 'vx' -ErrorAction SilentlyContinue) {
        $p = $null
        try { $p = $proc.Path } catch { }
        if ($p -and $p.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase)) {
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        }
    }
    if (Test-Path $uninstaller) {
        try { Invoke-Installer $uninstaller $quiet 'Uninstall (cleanup after failure)' } catch { Write-Warning "Cleanup: $_" }
        Wait-Gone $vireoExe | Out-Null
    }
    Remove-Item $dir, $workspace -Recurse -Force -ErrorAction SilentlyContinue
}
