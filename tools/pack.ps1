# Packs the Godot game + GDExtension DLL into ONE self-extracting exe.
#
#   [launcher.exe][payload.zip][u64 LE payload length]
#
# Usage:  pwsh -ExecutionPolicy Bypass -File tools\pack.ps1 -Arch x64
#         pwsh -ExecutionPolicy Bypass -File tools\pack.ps1 -Arch arm64
param(
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64',
    [string]$Root,
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem

if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
if (-not $OutDir) { $OutDir = Join-Path $Root 'dist' }

$godotBuild = Join-Path $Root 'godot\build'

# The launcher must match the target architecture too, otherwise an ARM64 build
# would ship an x64 stub that only runs under Windows' emulation layer.
if ($Arch -eq 'x64') {
    $launcherExe = Join-Path $Root 'target\release\logiclab-launcher.exe'
    $gameSrc = Join-Path $godotBuild 'LogicLab_win64.exe'
    $dllSrc = Join-Path $Root 'godot\logiclab_bridge.dll'
} else {
    $launcherExe = Join-Path $Root 'target\aarch64-pc-windows-msvc\release\logiclab-launcher.exe'
    $gameSrc = Join-Path $godotBuild 'LogicLab_arm64.exe'
    $dllSrc = Join-Path $Root 'godot\logiclab_bridge_arm64.dll'
}

foreach ($p in @($launcherExe, $gameSrc, $dllSrc)) {
    if (-not (Test-Path $p)) { throw "missing required file: $p" }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$stage = Join-Path $env:TEMP ('logiclab-pack-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null

try {
    # The launcher expects the game to be named LogicLab.exe, and the DLL name
    # must match what .gdextension declares (res://logiclab_bridge*.dll).
    Copy-Item $gameSrc (Join-Path $stage 'LogicLab.exe') -Force
    Copy-Item $dllSrc (Join-Path $stage (Split-Path -Leaf $dllSrc)) -Force
    # version.txt drives the launcher's "already unpacked" fast path
    $stamp = (Get-Date -Format 'yyyyMMdd-HHmmss')
    Set-Content -Path (Join-Path $stage 'version.txt') -Value $stamp -NoNewline -Encoding ascii

    $zipPath = Join-Path $env:TEMP ('logiclab-payload-' + [guid]::NewGuid().ToString('N') + '.zip')
    [System.IO.Compression.ZipFile]::CreateFromDirectory(
        $stage, $zipPath, [System.IO.Compression.CompressionLevel]::Optimal, $false)

    $outExe = Join-Path $OutDir ('LogicLab-' + $Arch + '.exe')
    $fs = [System.IO.File]::Create($outExe)
    try {
        $lb = [System.IO.File]::ReadAllBytes($launcherExe)
        $fs.Write($lb, 0, $lb.Length)
        $zb = [System.IO.File]::ReadAllBytes($zipPath)
        $fs.Write($zb, 0, $zb.Length)
        $lenBytes = [System.BitConverter]::GetBytes([uint64]$zb.Length)
        $fs.Write($lenBytes, 0, 8)
    } finally { $fs.Close() }

    $mb = [math]::Round((Get-Item $outExe).Length / 1MB, 1)
    Write-Output "packed: $outExe ($mb MB)"
} finally {
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    if ($zipPath) { Remove-Item $zipPath -Force -ErrorAction SilentlyContinue }
}