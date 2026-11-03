# Komari Agent installer for Windows.
#
# Mirrors komari-agent/install.ps1 (Go): nssm-based service management, the
# same --install-* options, architecture detection and GitHub proxy support.
# The agent is configured through config.json so the token is never placed on
# the service command line or printed by this script.
#
# Usage: powershell -ExecutionPolicy Bypass -File .\install.ps1 --help

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

function Write-Info { param([string]$Message) Write-Host $Message -ForegroundColor Cyan }
function Write-Success { param([string]$Message) Write-Host $Message -ForegroundColor Green }
function Write-WarningLog { param([string]$Message) Write-Host "[WARNING] $Message" -ForegroundColor Yellow }
function Write-ErrorLog { param([string]$Message) Write-Host "[ERROR] $Message" -ForegroundColor Red }
function Write-Step { param([string]$Message) Write-Host $Message -ForegroundColor Magenta }
function Write-Config { param([string]$Message) Write-Host "[CONFIG] $Message" -ForegroundColor White }

function Fail {
    param([string]$Message)
    Write-ErrorLog $Message
    exit 1
}

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------

function Show-Usage {
    @'
Komari Agent installer (Rust edition, Windows)

Usage:
  .\install.ps1 [INSTALL OPTIONS] [AGENT OPTIONS...]
  .\install.ps1 --uninstall [--install-dir DIR] [--install-service-name NAME]

Install options:
  --endpoint URL              Panel endpoint written to config.json
  --token TOKEN               Agent token written to config.json
  --interval SECONDS          Report interval written to config.json
  --install-dir DIR           Installation directory (default: %ProgramFiles%\Komari)
  --install-service-name NAME Service name (default: komari-agent)
  --service NAME              Alias for --install-service-name
  --install-ghproxy URL       GitHub proxy prefix, e.g. https://ghproxy.net
  --install-version VERSION   Release tag to install (default: latest)
  --config FILE               Use an existing config file instead of
                              generating <install-dir>\config.json
  --uninstall                 Stop and remove the service and the binary
  --purge                     With --uninstall, also remove config.json and
                              auto-discovery.json
  --dry-run                   Print what would happen without changing anything
  -h, --help                  Show this help

Any other option is passed through to the agent (for example --custom-dns,
--gpu or --disable-auto-update). The token is never printed and config.json is
restricted to SYSTEM and Administrators.

Examples:
  .\install.ps1 --endpoint https://panel.example.com --token SECRET
  .\install.ps1 --install-version v0.2.0 --install-ghproxy https://ghproxy.net
  .\install.ps1 --uninstall --purge
'@ | Write-Host
}

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

$ServiceName = 'komari-agent'
$InstallDir = if ($env:ProgramFiles) { Join-Path $env:ProgramFiles 'Komari' } else { 'C:\Program Files\Komari' }
$GitHubProxy = ''
$InstallVersion = ''
$Endpoint = ''
$Token = ''
$Interval = ''
$ConfigPath = ''
$ConfigExplicit = $false
$DryRun = $false
$Uninstall = $false
$Purge = $false
$AgentArgs = @()
$Repo = 'fearless743/komari-agent'
$NssmPath = $null

# ---------------------------------------------------------------------------
# Argument parsing (Go-compatible --install-* options)
# ---------------------------------------------------------------------------

for ($i = 0; $i -lt $args.Count; $i++) {
    $arg = [string]$args[$i]

    if ($arg -eq '-h' -or $arg -eq '--help') {
        Show-Usage
        exit 0
    }
    elseif ($arg -eq '--uninstall') { $Uninstall = $true }
    elseif ($arg -eq '--purge') { $Purge = $true }
    elseif ($arg -eq '--dry-run') { $DryRun = $true }
    elseif ($arg -eq '--install-dir' -or $arg -eq '--install-service-name' -or
        $arg -eq '--service' -or $arg -eq '--install-ghproxy' -or
        $arg -eq '--install-version' -or $arg -eq '--endpoint' -or
        $arg -eq '--token' -or $arg -eq '--interval' -or $arg -eq '--config') {
        if ($i + 1 -ge $args.Count) {
            Fail "Option $arg requires a value"
        }
        $i++
        $value = [string]$args[$i]
        switch ($arg) {
            '--install-dir' { $InstallDir = $value }
            '--install-service-name' { $ServiceName = $value }
            '--service' { $ServiceName = $value }
            '--install-ghproxy' { $GitHubProxy = $value }
            '--install-version' { $InstallVersion = $value }
            '--endpoint' { $Endpoint = $value }
            '--token' { $Token = $value }
            '--interval' { $Interval = $value }
            '--config' { $ConfigPath = $value; $ConfigExplicit = $true }
        }
    }
    elseif ($arg.StartsWith('--install-dir=')) { $InstallDir = $arg.Substring(14) }
    elseif ($arg.StartsWith('--install-service-name=')) { $ServiceName = $arg.Substring(23) }
    elseif ($arg.StartsWith('--service=')) { $ServiceName = $arg.Substring(10) }
    elseif ($arg.StartsWith('--install-ghproxy=')) { $GitHubProxy = $arg.Substring(18) }
    elseif ($arg.StartsWith('--install-version=')) { $InstallVersion = $arg.Substring(18) }
    elseif ($arg.StartsWith('--endpoint=')) { $Endpoint = $arg.Substring(11) }
    elseif ($arg.StartsWith('--token=')) { $Token = $arg.Substring(8) }
    elseif ($arg.StartsWith('--interval=')) { $Interval = $arg.Substring(11) }
    elseif ($arg.StartsWith('--config=')) { $ConfigPath = $arg.Substring(9); $ConfigExplicit = $true }
    elseif ($arg.StartsWith('--install')) { Write-WarningLog "Unknown install parameter: $arg" }
    else { $AgentArgs += $arg }
}

if (-not $ConfigPath) {
    $ConfigPath = Join-Path $InstallDir 'config.json'
}

if ($ServiceName -notmatch '^[A-Za-z0-9_.@-]+$') {
    Fail "Invalid service name: $ServiceName (allowed: letters, digits, . _ @ -)"
}

if ($Interval -ne '' -and $Interval -notmatch '^-?[0-9]+(\.[0-9]+)?$') {
    Fail "Invalid --interval value: $Interval (expected a number of seconds)"
}

$AgentPath = Join-Path $InstallDir 'komari-agent.exe'

# ---------------------------------------------------------------------------
# Elevation / architecture
# ---------------------------------------------------------------------------

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltinRole]::Administrator)

if (-not $isAdmin -and -not $DryRun) {
    Fail 'Please run this script as Administrator.'
}

$archRaw = $env:PROCESSOR_ARCHITECTURE
if ($env:PROCESSOR_ARCHITEW6432) { $archRaw = $env:PROCESSOR_ARCHITEW6432 }

$arch = switch ($archRaw) {
    'AMD64' { 'amd64' }
    'ARM64' { 'arm64' }
    'x86' { '386' }
    default { $null }
}

if (-not $arch) {
    Fail "Unsupported architecture: $archRaw"
}

# ---------------------------------------------------------------------------
# Configuration file
# ---------------------------------------------------------------------------

function Write-AgentConfig {
    param(
        [string]$EndpointValue,
        [string]$TokenValue,
        [string]$IntervalValue
    )

    $config = [ordered]@{
        endpoint             = ''
        token                = ''
        interval             = 1.0
        ignore_unsafe_cert   = $false
        max_retries          = 3
        reconnect_interval   = 5
        info_report_interval = 5
        protocol_version     = 1
    }

    if (Test-Path $ConfigPath) {
        try {
            $existing = Get-Content -Raw -Path $ConfigPath | ConvertFrom-Json
            foreach ($property in $existing.PSObject.Properties) {
                $config[$property.Name] = $property.Value
            }
        }
        catch {
            Write-WarningLog "Could not parse $ConfigPath; a new configuration will be written."
        }
    }

    if ($EndpointValue) { $config['endpoint'] = $EndpointValue }
    if ($TokenValue) { $config['token'] = $TokenValue }
    if ($IntervalValue) { $config['interval'] = [double]$IntervalValue }

    $json = $config | ConvertTo-Json -Depth 10
    $utf8 = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllText($ConfigPath, $json, $utf8)

    # Restrict the file to SYSTEM and Administrators. SIDs are used so the
    # command works on non-English Windows installations.
    & icacls $ConfigPath /inheritance:r /grant:r '*S-1-5-18:(F)' '*S-1-5-32-544:(F)' 2>&1 | Out-Null

    if (-not $config['endpoint'] -or -not $config['token']) {
        Write-WarningLog "endpoint and/or token are not set; edit $ConfigPath before starting the agent."
    }
    Write-Success "Configuration written to $ConfigPath"
}

# ---------------------------------------------------------------------------
# nssm handling
# ---------------------------------------------------------------------------

function Get-NssmIfAvailable {
    $cmd = Get-Command nssm -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    $local = Join-Path $InstallDir 'nssm.exe'
    if (Test-Path $local) { return $local }

    return $null
}

function Install-Nssm {
    $existing = Get-NssmIfAvailable
    if ($existing) {
        Write-Info "Using nssm at $existing"
        return $existing
    }

    $local = Join-Path $InstallDir 'nssm.exe'
    Write-Info "nssm not found. Downloading to $local..."

    $nssmVersion = '2.24'
    $zipUrl = "https://nssm.cc/release/nssm-$nssmVersion.zip"
    $zipPath = Join-Path $env:TEMP "nssm-$nssmVersion.zip"
    $extractDir = Join-Path $env:TEMP "nssm-extract-$PID"

    try {
        Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing
        if (Test-Path $extractDir) { Remove-Item -Recurse -Force $extractDir }
        New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
        Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

        $source = Join-Path $extractDir "nssm-$nssmVersion\win32\nssm.exe"
        if (-not (Test-Path $source)) {
            $fallback = Get-ChildItem -Path $extractDir -Recurse -Filter 'nssm.exe' |
                Select-Object -First 1
            if ($fallback) { $source = $fallback.FullName }
        }
        if (-not (Test-Path $source)) {
            Fail "Could not find nssm.exe in $extractDir. Install nssm from https://nssm.cc and retry."
        }

        Copy-Item -Path $source -Destination $local -Force
        Write-Success "nssm installed to $local"
        return $local
    }
    catch {
        Fail "Failed to download nssm: $_`nInstall nssm manually from https://nssm.cc and retry."
    }
    finally {
        if (Test-Path $zipPath) { Remove-Item -Force $zipPath -ErrorAction SilentlyContinue }
        if (Test-Path $extractDir) { Remove-Item -Recurse -Force $extractDir -ErrorAction SilentlyContinue }
    }
}

# ---------------------------------------------------------------------------
# Service management
# ---------------------------------------------------------------------------

function Stop-PreviousService {
    param([string]$Nssm)

    Write-Step 'Checking for a previous installation...'

    if ($Nssm -and (Test-Path $Nssm)) {
        & $Nssm status $ServiceName 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Write-Info "Stopping and removing service $ServiceName..."
            & $Nssm stop $ServiceName 2>&1 | Out-Null
            & $Nssm remove $ServiceName confirm 2>&1 | Out-Null
        }
    }
    else {
        $service = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
        if ($service) {
            Write-Info "Stopping and removing service $ServiceName with sc.exe..."
            Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
            & sc.exe delete $ServiceName 2>&1 | Out-Null
        }
    }

    if (Test-Path $AgentPath) {
        Write-Info "Removing old binary $AgentPath"
        Remove-Item -Path $AgentPath -Force
    }
}

function Install-AgentService {
    param([string]$Nssm)

    Write-Step 'Configuring Windows service with nssm...'

    $parameterString = "--config `"$ConfigPath`""
    if ($AgentArgs.Count -gt 0) {
        $parameterString = "$parameterString $($AgentArgs -join ' ')"
    }

    & $Nssm install $ServiceName $AgentPath
    if ($LASTEXITCODE -ne 0) { Fail "nssm install failed with exit code $LASTEXITCODE" }

    & $Nssm set $ServiceName AppDirectory $InstallDir | Out-Null
    & $Nssm set $ServiceName AppParameters $parameterString | Out-Null
    & $Nssm set $ServiceName DisplayName 'Komari Agent Service' | Out-Null
    & $Nssm set $ServiceName Start SERVICE_AUTO_START | Out-Null
    & $Nssm set $ServiceName AppExit Default Restart | Out-Null
    & $Nssm set $ServiceName AppRestartDelay 5000 | Out-Null
    & $Nssm set $ServiceName AppStdout (Join-Path $InstallDir 'komari-agent.log') | Out-Null
    & $Nssm set $ServiceName AppStderr (Join-Path $InstallDir 'komari-agent.log') | Out-Null
    & $Nssm set $ServiceName AppRotateFiles 1 | Out-Null
    & $Nssm set $ServiceName AppRotateOnline 1 | Out-Null
    & $Nssm set $ServiceName AppRotateBytes 10485760 | Out-Null
    & $Nssm start $ServiceName
    if ($LASTEXITCODE -ne 0) { Fail "nssm start failed with exit code $LASTEXITCODE" }

    Write-Success "Service $ServiceName installed and started."
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------

function Uninstall-Agent {
    param([string]$Nssm)

    if ($Nssm -and (Test-Path $Nssm)) {
        Write-Info "Using nssm at $Nssm"
    }
    else {
        Write-Info 'nssm not found; falling back to sc.exe for service removal.'
    }

    Stop-PreviousService -Nssm $Nssm

    if ($Purge) {
        Write-Info 'Removing configuration and state files...'
        if (Test-Path $ConfigPath) { Remove-Item -Path $ConfigPath -Force }
        $discovery = Join-Path $InstallDir 'auto-discovery.json'
        if (Test-Path $discovery) { Remove-Item -Path $discovery -Force }
    }

    Write-Success 'Komari Agent uninstalled.'
}

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------

function Resolve-ReleaseVersion {
    if ($InstallVersion) {
        return $InstallVersion
    }

    Write-Step 'Fetching the latest release version from GitHub...'
    $headers = @{ 'User-Agent' = 'komari-agent-installer' }
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
            -Headers $headers -UseBasicParsing
        return $release.tag_name
    }
    catch {
        Fail "Failed to fetch the latest release: $_"
    }
}

function Get-DownloadUrl {
    param([string]$Version)

    $asset = "komari-agent-windows-$arch.exe"
    $base = "https://github.com/$Repo/releases/download/$Version/$asset"
    if ($GitHubProxy) {
        $base = "$($GitHubProxy.TrimEnd('/'))/$base"
    }
    return $base
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

Write-Host '==========================================='
Write-Host '    Komari Agent Installation Script'
Write-Host '==========================================='
Write-Host ''

Write-Config "Service name: $ServiceName"
Write-Config "Install directory: $InstallDir"
Write-Config "Config file: $ConfigPath"
Write-Config "GitHub proxy: $(if ($GitHubProxy) { $GitHubProxy } else { '(direct)' })"
Write-Config "Architecture: $arch"
Write-Config "Token: $(if ($Token) { '******' } else { '(unset)' })"
Write-Config "Agent arguments: $($AgentArgs -join ' ')"
Write-Config "Agent version: $(if ($InstallVersion) { $InstallVersion } else { 'latest' })"
Write-Config "Dry run: $DryRun"
Write-Host ''

if ($Uninstall) {
    $NssmPath = Get-NssmIfAvailable
    if ($DryRun) {
        Write-Info "[dry-run] would stop and remove service $ServiceName"
        Write-Info "[dry-run] would remove $AgentPath"
        if ($Purge) {
            Write-Info "[dry-run] would remove $ConfigPath and auto-discovery.json"
        }
        Write-Success 'Dry run completed; nothing was changed.'
        exit 0
    }
    Uninstall-Agent -Nssm $NssmPath
    exit 0
}

if (-not $DryRun) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

if ($DryRun) {
    Write-Step 'Dry run plan'
    Write-Info "[dry-run] would ensure installation directory: $InstallDir"
    Write-Info '[dry-run] would download nssm if it is not available'
    if ($InstallVersion) {
        $version = $InstallVersion
    }
    else {
        $version = '<latest release tag>'
    }
    Write-Info "[dry-run] would install komari-agent $version from https://github.com/$Repo/releases"
    Write-Info "[dry-run] would write config: $ConfigPath"
    if ($Token) { Write-Info '[dry-run] token: ******' }
    Write-Info "[dry-run] would register service $ServiceName with nssm"
    Write-Success 'Dry run completed; nothing was changed.'
    exit 0
}

$NssmPath = Install-Nssm

$version = Resolve-ReleaseVersion
$downloadUrl = Get-DownloadUrl -Version $version
Write-Info "Installing Komari Agent version: $version"
Write-Info "URL: $downloadUrl"

$tempPath = "$AgentPath.download"
try {
    Invoke-WebRequest -Uri $downloadUrl -OutFile $tempPath -UseBasicParsing
}
catch {
    if (Test-Path $tempPath) { Remove-Item -Force $tempPath -ErrorAction SilentlyContinue }
    Fail "Download failed: $_"
}

Stop-PreviousService -Nssm $NssmPath

Move-Item -Path $tempPath -Destination $AgentPath -Force
Write-Success "komari-agent installed to $AgentPath"

if ($ConfigExplicit) {
    if (-not (Test-Path $ConfigPath)) {
        Write-WarningLog "Config file $ConfigPath does not exist; the agent will fail to start until it is created."
    }
    Write-Info "Using existing config file: $ConfigPath"
}
else {
    Write-AgentConfig -EndpointValue $Endpoint -TokenValue $Token -IntervalValue $Interval
}

Install-AgentService -Nssm $NssmPath

Write-Host ''
Write-Success 'Komari Agent installation completed!'
Write-Config "Service: $ServiceName"
Write-Config "Binary: $AgentPath"
Write-Config "Config: $ConfigPath"
