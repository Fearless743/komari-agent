# Build the Komari Agent release matrix with Cargo (Windows PowerShell 5.1+).
#
# Asset names match the Go agent and the self-updater:
#   komari-agent-{goos}-{goarch}[.exe]
#
# Version injection uses KOMARI_AGENT_VERSION / KOMARI_AGENT_VERSION_HASH.
# agent-cli is cleaned before each target because option_env! values are not
# tracked by Cargo.
#
# Usage: powershell -ExecutionPolicy Bypass -File .\build_all.ps1 [--list]

$ErrorActionPreference = 'Continue'

$BuildDir = Join-Path (Get-Location) 'build'
$Only = if ($env:KOMARI_BUILD_TARGETS) { $env:KOMARI_BUILD_TARGETS } else { '' }
$BuildTool = if ($env:KOMARI_BUILD_TOOL) { $env:KOMARI_BUILD_TOOL } else { 'auto' }
$InstallTargets = if ($env:KOMARI_INSTALL_TARGETS) { $env:KOMARI_INSTALL_TARGETS } else { '1' }
$DryRun = if ($env:KOMARI_BUILD_DRY_RUN) { $env:KOMARI_BUILD_DRY_RUN } else { '0' }

$Targets = @(
    @{ Goos = 'linux'; Goarch = 'amd64'; Target = 'x86_64-unknown-linux-musl' },
    @{ Goos = 'linux'; Goarch = 'arm64'; Target = 'aarch64-unknown-linux-musl' },
    @{ Goos = 'linux'; Goarch = '386'; Target = 'i686-unknown-linux-musl' },
    @{ Goos = 'linux'; Goarch = 'arm'; Target = 'armv7-unknown-linux-musleabihf' },
    @{ Goos = 'windows'; Goarch = 'amd64'; Target = 'x86_64-pc-windows-msvc' },
    @{ Goos = 'windows'; Goarch = 'arm64'; Target = 'aarch64-pc-windows-msvc' },
    @{ Goos = 'windows'; Goarch = '386'; Target = 'i686-pc-windows-msvc' },
    @{ Goos = 'darwin'; Goarch = 'amd64'; Target = 'x86_64-apple-darwin' },
    @{ Goos = 'darwin'; Goarch = 'arm64'; Target = 'aarch64-apple-darwin' },
    @{ Goos = 'freebsd'; Goarch = 'amd64'; Target = 'x86_64-unknown-freebsd' },
    @{ Goos = 'freebsd'; Goarch = 'arm64'; Target = 'aarch64-unknown-freebsd' },
    @{ Goos = 'freebsd'; Goarch = '386'; Target = 'i686-unknown-freebsd' },
    @{ Goos = 'freebsd'; Goarch = 'arm'; Target = 'armv7-unknown-freebsd' }
)

function Show-Usage {
    @'
Build the Komari Agent release matrix.

Usage:
  .\build_all.ps1 [--list] [--help]

Environment:
  KOMARI_AGENT_VERSION        Version string (default: latest git tag or "dev")
  KOMARI_AGENT_VERSION_HASH   Revision string (default: short git SHA)
  KOMARI_BUILD_TARGETS        Comma-separated filter, e.g. linux-amd64,darwin-arm64
  KOMARI_BUILD_TOOL           auto (default), cargo, zigbuild or cross
  KOMARI_INSTALL_TARGETS      1 (default) to run rustup target add when missing
  KOMARI_BUILD_DRY_RUN        1 to print the build commands without running them

Output: .\build\komari-agent-{goos}-{goarch}[.exe]
'@ | Write-Host
}

function Show-Targets {
    foreach ($t in $Targets) {
        '{0,-8} {1,-6} {2}' -f $t.Goos, $t.Goarch, $t.Target | Write-Host
    }
}

foreach ($arg in $args) {
    switch ([string]$arg) {
        '-h' { Show-Usage; exit 0 }
        '--help' { Show-Usage; exit 0 }
        '--list' { Show-Targets; exit 0 }
        default {
            Write-Host "Unknown argument: $arg" -ForegroundColor Red
            Show-Usage
            exit 2
        }
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host 'cargo not found in PATH' -ForegroundColor Red
    exit 1
}

New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null

$version = $env:KOMARI_AGENT_VERSION
if (-not $version) {
    $version = (git describe --tags --abbrev=0 2>$null)
    if (-not $version) { $version = 'dev' }
    $version = $version.Trim()
}

$hash = $env:KOMARI_AGENT_VERSION_HASH
if (-not $hash) {
    $hash = (git rev-parse --short HEAD 2>$null)
    if ($hash) { $hash = $hash.Trim() }
}

$env:KOMARI_AGENT_VERSION = $version
$env:KOMARI_AGENT_VERSION_HASH = $hash

$hostLine = (& rustc -vV | Select-String '^host: ').Line
$hostTarget = $hostLine -replace '^host: ', ''

Write-Host "Version: $version$(if ($hash) { " ($hash)" })"
Write-Host "Host target: $hostTarget"
Write-Host ''

$hasZigbuild = $false
if ($BuildTool -eq 'auto') {
    & cargo zigbuild --version 2>$null | Out-Null
    $hasZigbuild = ($LASTEXITCODE -eq 0)
}
$hasCross = [bool](Get-Command cross -ErrorAction SilentlyContinue)

function Test-Selected {
    param([string]$Name)
    if (-not $Only) { return $true }
    return (",$Only," -like "*,$Name,*")
}

function Get-ToolForTarget {
    param([string]$Target)
    if ($BuildTool -ne 'auto') { return $BuildTool }
    if ($Target -eq $hostTarget) { return 'cargo' }
    if ($hasZigbuild) { return 'zigbuild' }
    if ($hasCross) { return 'cross' }
    return 'cargo'
}

function Install-TargetIfNeeded {
    param([string]$Target)
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { return $true }

    $installed = & rustup target list --installed 2>$null
    if ($installed -contains $Target) { return $true }

    if ($InstallTargets -ne '1') {
        Write-Host "  target $Target is not installed (run: rustup target add $Target)" -ForegroundColor Yellow
        return $false
    }

    Write-Host "  installing rust target $Target..."
    & rustup target add $Target
    return ($LASTEXITCODE -eq 0)
}

$failed = @()
$built = 0

foreach ($t in $Targets) {
    $name = "$($t.Goos)-$($t.Goarch)"
    if (-not (Test-Selected $name)) { continue }

    $asset = "komari-agent-$name"
    if ($t.Goos -eq 'windows') { $asset = "$asset.exe" }

    $tool = Get-ToolForTarget $t.Target
    Write-Host "Building $($t.Goos)/$($t.Goarch) ($($t.Target)) with $tool..."

    if ($DryRun -eq '1') {
        Write-Host "  [dry-run] KOMARI_AGENT_VERSION=$version KOMARI_AGENT_VERSION_HASH=$hash"
        Write-Host "  [dry-run] $tool build --release --locked --target $($t.Target) -p agent-cli"
        Write-Host "  [dry-run] would produce $(Join-Path $BuildDir $asset)"
        $built++
        continue
    }

    if (-not (Install-TargetIfNeeded $t.Target)) {
        $failed += $name
        continue
    }

    & cargo clean -p agent-cli --release --target $t.Target 2>$null | Out-Null

    switch ($tool) {
        'cargo' { & cargo build --release --locked --target $t.Target -p agent-cli }
        'zigbuild' { & cargo zigbuild --release --locked --target $t.Target -p agent-cli }
        'cross' { & cross build --release --locked --target $t.Target -p agent-cli }
        default {
            Write-Host "Unknown build tool: $tool" -ForegroundColor Red
            $failed += $name
            continue
        }
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Host "Failed to build for $($t.Goos)/$($t.Goarch)" -ForegroundColor Red
        $failed += $name
        continue
    }

    $binary = Join-Path (Join-Path (Join-Path 'target' $t.Target) 'release') 'komari-agent'
    if ($t.Goos -eq 'windows') { $binary = "$binary.exe" }

    if (-not (Test-Path $binary)) {
        Write-Host "Build succeeded but $binary is missing" -ForegroundColor Red
        $failed += $name
        continue
    }

    Copy-Item -Path $binary -Destination (Join-Path $BuildDir $asset) -Force
    Write-Host "Successfully built $(Join-Path $BuildDir $asset)" -ForegroundColor Green
    $built++
}

Write-Host ''
if ($failed.Count -gt 0) {
    Write-Host 'The following builds failed:' -ForegroundColor Red
    foreach ($item in $failed) { Write-Host "  - $item" -ForegroundColor Red }
    Write-Host "`n$built build(s) succeeded, $($failed.Count) failed. Binaries are in $BuildDir." -ForegroundColor Red
    exit 1
}

Write-Host "All $built builds completed successfully. Binaries are in $BuildDir." -ForegroundColor Green
