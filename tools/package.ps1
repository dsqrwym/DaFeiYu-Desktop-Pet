# Build a runnable folder:
#   dist/feiyu/{feiyu.exe, feiyu-cli.exe, config.json, assets/anim, README.md, NOTICE.md, LICENSE}
# feiyu.exe is the GUI build (double-click, no console); feiyu-cli.exe is the console
# build used for --selftest / --list / --showcase and other diagnostics.
# config.json is a copy of config.example.json; edit it and restart to tune behavior.
# ASCII only: Windows PowerShell 5.1 reads BOM-less files as ANSI.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tools/package.ps1
#   powershell -ExecutionPolicy Bypass -File tools/package.ps1 -Zip
param(
    [string]$Out = 'dist\feiyu',
    [switch]$Zip
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root

try {
    Write-Host '[1/4] cargo build --release -p feiyu-app'
    cargo build --release -p feiyu-app
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    $manifest = Join-Path $root 'assets\anim\manifest.json'
    if (-not (Test-Path $manifest)) {
        throw 'assets/anim/manifest.json not found; run pack-anim first (see README)'
    }

    Write-Host "[2/4] prepare $Out"
    if (Test-Path $Out) {
        # A shell/Explorer window may hold the directory itself as its CWD, which
        # blocks Remove-Item on the directory but not on its contents. Fall back to
        # clearing the contents; copying into the existing directory still works.
        try {
            Remove-Item -Recurse -Force $Out -ErrorAction Stop
        } catch {
            Get-ChildItem $Out -Force | Remove-Item -Recurse -Force
        }
    }
    New-Item -ItemType Directory -Force $Out | Out-Null

    Write-Host '[3/4] copy exe + assets'
    Copy-Item (Join-Path $root 'target\release\feiyu.exe') $Out
    Copy-Item (Join-Path $root 'target\release\feiyu-cli.exe') $Out
    Copy-Item -Recurse (Join-Path $root 'assets\anim') (Join-Path $Out 'assets\anim')
    foreach ($f in 'README.md', 'NOTICE.md', 'LICENSE') {
        if (Test-Path (Join-Path $root $f)) { Copy-Item (Join-Path $root $f) $Out }
    }
    $cfg = Join-Path $root 'config.example.json'
    if (Test-Path $cfg) { Copy-Item $cfg (Join-Path $Out 'config.json') }

    $size = (Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum
    Write-Host ('[4/4] done: {0} ({1:N1} MB)' -f (Resolve-Path $Out), ($size / 1MB))

    if ($Zip) {
        $zip = "$Out.zip"
        if (Test-Path $zip) { Remove-Item -Force $zip }
        Write-Host "zip -> $zip (this can take a few minutes)"
        Compress-Archive -Path (Join-Path $Out '*') -DestinationPath $zip -CompressionLevel Fastest
        $zipSize = (Get-Item $zip).Length
        Write-Host ('zip done: {0} ({1:N1} MB)' -f $zip, ($zipSize / 1MB))
    }
} finally {
    Pop-Location
}
