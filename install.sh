#!/usr/bin/env bash
#
# Komari Agent installer for Linux, macOS and FreeBSD.
#
# The command line surface follows komari-agent/install.sh (Go). The agent is
# configured through a JSON file instead of command line flags so the token is
# never visible in `ps`, the unit file or this script's output.
#
# Usage: ./install.sh --help

set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

SERVICE_NAME="komari-agent"
TARGET_DIR=""
TARGET_DIR_SET=false
GITHUB_PROXY=""
INSTALL_VERSION=""
ENDPOINT=""
TOKEN=""
INTERVAL=""
CONFIG_FILE=""
CONFIG_EXPLICIT=false
DRY_RUN=false
UNINSTALL=false
PURGE=false

OS_NAME=""
ARCH=""
AGENT_ARGS=()
REPO="fearless743/komari-agent"
# Test hooks: prefix service definition paths and skip service commands.
SERVICE_ROOT="${KOMARI_SERVICE_ROOT:-}"
SERVICE_RENDER_ONLY="${KOMARI_SERVICE_RENDER_ONLY:-0}"

# Filled in by detect_platform / parse_args.
RED=""
GREEN=""
YELLOW=""
BLUE=""
CYAN=""
BOLD=""
NC=""

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

setup_colors() {
    if [ -t 1 ] && [ "${NO_COLOR:-}" = "" ]; then
        RED=$'\033[0;31m'
        GREEN=$'\033[0;32m'
        YELLOW=$'\033[0;33m'
        BLUE=$'\033[0;34m'
        CYAN=$'\033[0;36m'
        BOLD=$'\033[1;37m'
        NC=$'\033[0m'
    fi
}

log_info() {
    printf ' %s\n' "$*"
}

log_success() {
    printf '%s%s%s\n' "$GREEN" "$*" "$NC"
}

log_warning() {
    printf '%s[WARNING]%s %s\n' "$YELLOW" "$NC" "$*" >&2
}

log_error() {
    printf '%s[ERROR]%s %s\n' "$RED" "$NC" "$*" >&2
}

log_step() {
    printf '%s%s%s\n' "$BLUE" "$*" "$NC"
}

log_config() {
    printf '%s[CONFIG]%s %s\n' "$CYAN" "$NC" "$*"
}

die() {
    log_error "$*"
    exit 1
}

run_cmd() {
    if [ "$SERVICE_RENDER_ONLY" = "1" ]; then
        log_info "[render-only] $*"
        return 0
    fi
    if [ "$DRY_RUN" = true ]; then
        log_info "[dry-run] $*"
    else
        "$@"
    fi
}

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------

usage() {
    cat <<'EOF'
Komari Agent installer (Rust edition)

Usage:
  ./install.sh [INSTALL OPTIONS] [AGENT OPTIONS...]
  ./install.sh --uninstall [--install-dir DIR] [--install-service-name NAME]

Install options:
  --endpoint URL              Panel endpoint written to config.json
  -e URL
  --token TOKEN               Agent token written to config.json
  -t TOKEN
  --interval SECONDS          Report interval written to config.json
  -i SECONDS
  --install-dir DIR           Installation directory (default: /opt/komari;
                              /usr/local/komari on macOS; C:\komari under MSYS)
  --install-service-name NAME Service name (default: komari-agent)
  --service NAME              Alias for --install-service-name
  --install-ghproxy URL       GitHub proxy prefix, e.g. https://ghproxy.net
  --install-version VERSION   Release tag to install (default: latest)
  --config FILE               Use an existing config file instead of
                              generating <install-dir>/config.json
  --uninstall                 Stop and remove the service and the binary
  --purge                     With --uninstall, also remove config.json and
                              auto-discovery.json
  --dry-run                   Print what would happen without changing anything
  -h, --help                  Show this help

Any other option is passed through to the agent (for example --custom-dns,
--gpu or --disable-auto-update). The token is never printed and is written to
config.json with mode 0600.

Examples:
  sudo ./install.sh --endpoint https://panel.example.com --token SECRET
  sudo ./install.sh --install-version v0.2.0 --install-ghproxy https://ghproxy.net
  sudo ./install.sh --uninstall --purge
EOF
}

# ---------------------------------------------------------------------------
# Platform / architecture detection
# ---------------------------------------------------------------------------

detect_platform() {
    local uname_s
    uname_s=$(uname -s)

    case "$uname_s" in
        Darwin)
            OS_NAME="darwin"
            TARGET_DIR="/usr/local/komari"
            ;;
        Linux)
            OS_NAME="linux"
            TARGET_DIR="/opt/komari"
            ;;
        FreeBSD)
            OS_NAME="freebsd"
            TARGET_DIR="/opt/komari"
            ;;
        MINGW* | MSYS* | CYGWIN*)
            OS_NAME="windows"
            TARGET_DIR="/c/komari"
            ;;
        *)
            die "Unsupported operating system: $uname_s"
            ;;
    esac
}

detect_arch() {
    local machine
    machine=$(uname -m)

    case "$machine" in
        x86_64)
            ARCH="amd64"
            ;;
        aarch64 | arm64)
            ARCH="arm64"
            ;;
        i386 | i686)
            case "$OS_NAME" in
                freebsd | linux | windows) ARCH="386" ;;
                *) die "32-bit x86 architecture is not supported on $OS_NAME" ;;
            esac
            ;;
        arm | armv6* | armv7* | armv8l)
            case "$OS_NAME" in
                freebsd | linux) ARCH="arm" ;;
                *) die "32-bit ARM architecture is not supported on $OS_NAME" ;;
            esac
            ;;
        *)
            die "Unsupported architecture: $machine on $OS_NAME"
            ;;
    esac

    # armv7-unknown-freebsd is a tier 3 Rust target and rustup ships no std
    # for it, so the release matrix does not publish a FreeBSD/arm binary.
    if [ "$OS_NAME" = "freebsd" ] && [ "$ARCH" = "arm" ]; then
        die "FreeBSD/arm binaries are not published (armv7-unknown-freebsd is a tier 3 target; see D-024)."
    fi
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

need_value() {
    [ "$#" -ge 2 ] || die "Option $1 requires a value"
}

parse_args() {
    while [ "$#" -gt 0 ]; do
        case "$1" in
            -h | --help)
                usage
                exit 0
                ;;
            --uninstall)
                UNINSTALL=true
                shift
                ;;
            --purge)
                PURGE=true
                shift
                ;;
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            --install-dir)
                need_value "$@"
                TARGET_DIR="$2"
                TARGET_DIR_SET=true
                shift 2
                ;;
            --install-dir=*)
                TARGET_DIR="${1#*=}"
                TARGET_DIR_SET=true
                shift
                ;;
            --install-service-name | --service)
                need_value "$@"
                SERVICE_NAME="$2"
                shift 2
                ;;
            --install-service-name=* | --service=*)
                SERVICE_NAME="${1#*=}"
                shift
                ;;
            --install-ghproxy)
                need_value "$@"
                GITHUB_PROXY="$2"
                shift 2
                ;;
            --install-ghproxy=*)
                GITHUB_PROXY="${1#*=}"
                shift
                ;;
            --install-version)
                need_value "$@"
                INSTALL_VERSION="$2"
                shift 2
                ;;
            --install-version=*)
                INSTALL_VERSION="${1#*=}"
                shift
                ;;
            --endpoint | -e)
                need_value "$@"
                ENDPOINT="$2"
                shift 2
                ;;
            --token | -t)
                need_value "$@"
                TOKEN="$2"
                shift 2
                ;;
            --token=* | -t=*)
                TOKEN="${1#*=}"
                shift
                ;;
            --endpoint=* | -e=*)
                ENDPOINT="${1#*=}"
                shift
                ;;
            --interval=* | -i=*)
                INTERVAL="${1#*=}"
                shift
                ;;
            --interval | -i)
                need_value "$@"
                INTERVAL="$2"
                shift 2
                ;;
            --config)
                need_value "$@"
                CONFIG_FILE="$2"
                CONFIG_EXPLICIT=true
                shift 2
                ;;
            --config=*)
                CONFIG_FILE="${1#*=}"
                CONFIG_EXPLICIT=true
                shift
                ;;
            --)
                shift
                AGENT_ARGS+=("$@")
                break
                ;;
            --install*)
                log_warning "Unknown install parameter: $1"
                shift
                ;;
            *)
                AGENT_ARGS+=("$1")
                shift
                ;;
        esac
    done

    if [ -z "$TARGET_DIR" ]; then
        detect_platform
    fi

    # Go's installer falls back to a user directory on macOS when /usr/local
    # is not writable and the script runs unprivileged.
    if [ "$OS_NAME" = "darwin" ] && [ "$TARGET_DIR_SET" = false ] &&
        [ "$(id -u)" -ne 0 ] && [ ! -w "/usr/local" ]; then
        TARGET_DIR="$HOME/.komari"
        log_info "No write permission to /usr/local, using user directory: $TARGET_DIR"
    fi

    if [[ "$TARGET_DIR" != /* ]]; then
        die "Install directory must be an absolute path: $TARGET_DIR"
    fi

    if [ -z "$CONFIG_FILE" ]; then
        CONFIG_FILE="$TARGET_DIR/config.json"
    elif [[ "$CONFIG_FILE" != /* ]]; then
        CONFIG_FILE="$TARGET_DIR/$CONFIG_FILE"
    fi

    if ! [[ "$SERVICE_NAME" =~ ^[A-Za-z0-9_.@-]+$ ]]; then
        die "Invalid service name: $SERVICE_NAME (allowed: letters, digits, . _ @ -)"
    fi

    if [ -n "$INTERVAL" ] && ! [[ "$INTERVAL" =~ ^-?[0-9]+([.][0-9]+)?$ ]]; then
        die "Invalid --interval value: $INTERVAL (expected a number of seconds)"
    fi

    if [ "$PURGE" = true ] && [ "$UNINSTALL" = false ]; then
        log_warning "--purge has no effect without --uninstall"
    fi

    if [ "$CONFIG_EXPLICIT" = true ] &&
        { [ -n "$ENDPOINT" ] || [ -n "$TOKEN" ] || [ -n "$INTERVAL" ]; }; then
        log_warning "--config was provided; --endpoint/--token/--interval are not written to it"
    fi
}

check_root() {
    if [ "$DRY_RUN" = true ]; then
        return 0
    fi
    if [ "$(id -u)" -eq 0 ]; then
        return 0
    fi
    # macOS with Homebrew can install as a normal user.
    if [ "$OS_NAME" = "darwin" ] && command -v brew >/dev/null 2>&1; then
        return 0
    fi
    die "Please run as root (or use sudo)"
}

install_dependencies() {
    if [ "$DRY_RUN" = true ]; then
        log_info "[dry-run] skipping dependency installation"
        return 0
    fi

    log_step "Checking dependencies..."
    local missing=()
    command -v curl >/dev/null 2>&1 || missing+=("curl")

    if [ "${#missing[@]}" -eq 0 ]; then
        log_success "Dependencies already satisfied"
        return 0
    fi

    if command -v apt-get >/dev/null 2>&1; then
        log_info "Installing ${missing[*]} with apt-get..."
        apt-get update
        apt-get install -y "${missing[@]}"
    elif command -v dnf >/dev/null 2>&1; then
        log_info "Installing ${missing[*]} with dnf..."
        dnf install -y "${missing[@]}"
    elif command -v yum >/dev/null 2>&1; then
        log_info "Installing ${missing[*]} with yum..."
        yum install -y "${missing[@]}"
    elif command -v apk >/dev/null 2>&1; then
        log_info "Installing ${missing[*]} with apk..."
        apk add "${missing[@]}"
    elif command -v brew >/dev/null 2>&1; then
        log_info "Installing ${missing[*]} with Homebrew..."
        brew install "${missing[@]}"
    else
        die "No supported package manager found (apt-get/dnf/yum/apk/brew)"
    fi

    for cmd in "${missing[@]}"; do
        command -v "$cmd" >/dev/null 2>&1 || die "Failed to install $cmd"
    done
    log_success "Dependencies installed successfully"
}

# ---------------------------------------------------------------------------
# Init system detection and service teardown
# ---------------------------------------------------------------------------

detect_init_system() {
    if [ -f /etc/NIXOS ]; then
        printf 'nixos'
        return
    fi

    # Alpine always uses OpenRC, even when PID 1 is something else (containers).
    if [ -f /etc/alpine-release ] &&
        { command -v rc-service >/dev/null 2>&1 || [ -f /sbin/openrc-run ]; }; then
        printf 'openrc'
        return
    fi

    local pid1=""
    pid1=$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ' || true)

    if [ "$pid1" = "systemd" ] || [ -d /run/systemd/system ]; then
        if command -v systemctl >/dev/null 2>&1 && systemctl list-units >/dev/null 2>&1; then
            printf 'systemd'
            return
        fi
    fi

    if [ "$pid1" = "openrc-init" ] && command -v rc-service >/dev/null 2>&1; then
        printf 'openrc'
        return
    fi

    if [ "$pid1" = "init" ] && [ ! -f /etc/alpine-release ]; then
        if [ -d /run/openrc ] && command -v rc-service >/dev/null 2>&1; then
            printf 'openrc'
            return
        fi
        if [ -f /sbin/openrc ] && command -v rc-service >/dev/null 2>&1; then
            printf 'openrc'
            return
        fi
    fi

    if command -v uci >/dev/null 2>&1 && [ -f /etc/rc.common ]; then
        printf 'procd'
        return
    fi

    # FreeBSD rc.d. The Go installer does not handle this init system, but the
    # build matrix ships FreeBSD binaries, so support it here.
    if [ "$OS_NAME" = "freebsd" ] && [ -f /etc/rc.subr ]; then
        printf 'bsdrc'
        return
    fi

    if [ "$OS_NAME" = "darwin" ] && command -v launchctl >/dev/null 2>&1; then
        printf 'launchd'
        return
    fi

    if command -v systemctl >/dev/null 2>&1 && systemctl list-units >/dev/null 2>&1; then
        printf 'systemd'
        return
    fi

    if command -v rc-service >/dev/null 2>&1 && [ -d /etc/init.d ]; then
        printf 'openrc'
        return
    fi

    if command -v initctl >/dev/null 2>&1 && [ -d /etc/init ]; then
        printf 'upstart'
        return
    fi

    printf 'unknown'
}

# Stop and remove a service previously created by this installer (or by the Go
# installer). Safe to call when nothing is installed.
service_is_running() {
    local init_system="$1"
    local domain="${2:-}"

    case "$init_system" in
        systemd)
            command -v systemctl >/dev/null 2>&1 &&
                systemctl is-active --quiet "${SERVICE_NAME}.service"
            ;;
        openrc)
            command -v rc-service >/dev/null 2>&1 &&
                rc-service "$SERVICE_NAME" status >/dev/null 2>&1
            ;;
        procd)
            local pidfile="/var/run/${SERVICE_NAME}.pid"
            if [ -f "$pidfile" ] && [ -s "$pidfile" ]; then
                kill -0 "$(cat "$pidfile" 2>/dev/null)" 2>/dev/null && return 0
            fi
            command -v pgrep >/dev/null 2>&1 && pgrep -f "$AGENT_PATH" >/dev/null 2>&1
            ;;
        upstart)
            command -v initctl >/dev/null 2>&1 &&
                initctl status "$SERVICE_NAME" 2>/dev/null | grep -q 'start/running'
            ;;
        bsdrc)
            local rc_name="${SERVICE_NAME//-/_}"
            local pidfile="/var/run/${rc_name}.pid"
            [ -f "$pidfile" ] && [ -s "$pidfile" ] &&
                kill -0 "$(cat "$pidfile" 2>/dev/null)" 2>/dev/null
            ;;
        launchd)
            command -v launchctl >/dev/null 2>&1 &&
                launchctl print "${domain}/com.komari.${SERVICE_NAME}" >/dev/null 2>&1
            ;;
        *)
            return 1
            ;;
    esac
}

wait_for_service_stopped() {
    local init_system="$1"
    local domain="${2:-}"
    local timeout="${KOMARI_STOP_TIMEOUT:-15}"
    local waited=0

    while service_is_running "$init_system" "$domain"; do
        waited=$((waited + 1))
        if [ "$waited" -ge "$timeout" ]; then
            return 1
        fi
        sleep 1
    done
    return 0
}

# Stop and remove a service previously created by this installer (or by the Go
# installer). A failed stop aborts the operation so an upgrade never reports
# success while the old process is still running.
stop_previous_service() {
    local init_system="$1"

    log_step "Checking for a previous installation..."

    case "$init_system" in
        systemd)
            local unit="${SERVICE_ROOT}/etc/systemd/system/${SERVICE_NAME}.service"
            if [ -f "$unit" ]; then
                log_info "Stopping and disabling existing systemd service..."
                if service_is_running systemd; then
                    run_cmd systemctl stop "${SERVICE_NAME}.service" ||
                        die "Failed to stop ${SERVICE_NAME}.service"
                    if [ "$DRY_RUN" = false ] && ! wait_for_service_stopped systemd; then
                        die "${SERVICE_NAME}.service is still running after systemctl stop"
                    fi
                fi
                if [ "$DRY_RUN" = false ] &&
                    systemctl is-enabled --quiet "${SERVICE_NAME}.service" 2>/dev/null; then
                    run_cmd systemctl disable "${SERVICE_NAME}.service" ||
                        die "Failed to disable ${SERVICE_NAME}.service"
                fi
                run_cmd rm -f "$unit" || die "Failed to remove $unit"
                run_cmd systemctl daemon-reload || log_warning "systemctl daemon-reload failed"
            fi
            ;;
        openrc)
            if [ -f "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" ]; then
                log_info "Stopping and disabling existing OpenRC service..."
                if service_is_running openrc; then
                    run_cmd rc-service "$SERVICE_NAME" stop ||
                        die "Failed to stop OpenRC service $SERVICE_NAME"
                    if [ "$DRY_RUN" = false ] && ! wait_for_service_stopped openrc; then
                        die "OpenRC service $SERVICE_NAME is still running after stop"
                    fi
                fi
                if [ "$DRY_RUN" = false ] &&
                    rc-update show default 2>/dev/null | grep -qw "$SERVICE_NAME"; then
                    run_cmd rc-update del "$SERVICE_NAME" default ||
                        die "Failed to remove $SERVICE_NAME from the default runlevel"
                fi
                run_cmd rm -f "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" ||
                    die "Failed to remove /etc/init.d/${SERVICE_NAME}"
            fi
            ;;
        procd)
            if [ -f "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" ]; then
                log_info "Stopping and disabling existing procd service..."
                if service_is_running procd; then
                    run_cmd "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" stop ||
                        die "Failed to stop procd service $SERVICE_NAME"
                    if [ "$DRY_RUN" = false ] && ! wait_for_service_stopped procd; then
                        die "procd service $SERVICE_NAME is still running after stop"
                    fi
                fi
                run_cmd "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" disable ||
                    die "Failed to disable procd service $SERVICE_NAME"
                run_cmd rm -f "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" ||
                    die "Failed to remove /etc/init.d/${SERVICE_NAME}"
            fi
            ;;
        upstart)
            if [ -f "${SERVICE_ROOT}/etc/init/${SERVICE_NAME}.conf" ]; then
                log_info "Stopping and removing existing upstart service..."
                if service_is_running upstart; then
                    run_cmd initctl stop "$SERVICE_NAME" ||
                        die "Failed to stop upstart service $SERVICE_NAME"
                    if [ "$DRY_RUN" = false ] && ! wait_for_service_stopped upstart; then
                        die "Upstart service $SERVICE_NAME is still running after stop"
                    fi
                fi
                run_cmd rm -f "${SERVICE_ROOT}/etc/init/${SERVICE_NAME}.conf" ||
                    die "Failed to remove /etc/init/${SERVICE_NAME}.conf"
            fi
            ;;
        bsdrc)
            if [ -f "${SERVICE_ROOT}/usr/local/etc/rc.d/${SERVICE_NAME}" ]; then
                log_info "Stopping and disabling existing FreeBSD rc.d service..."
                if service_is_running bsdrc; then
                    run_cmd service "$SERVICE_NAME" stop ||
                        die "Failed to stop FreeBSD rc.d service $SERVICE_NAME"
                    if [ "$DRY_RUN" = false ] && ! wait_for_service_stopped bsdrc; then
                        die "FreeBSD rc.d service $SERVICE_NAME is still running after stop"
                    fi
                fi
                if [ "$DRY_RUN" = false ] && command -v sysrc >/dev/null 2>&1; then
                    run_cmd sysrc -x "${SERVICE_NAME//-/_}_enable" ||
                        log_warning "Failed to remove ${SERVICE_NAME//-/_}_enable from rc.conf"
                fi
                run_cmd rm -f "${SERVICE_ROOT}/usr/local/etc/rc.d/${SERVICE_NAME}" ||
                    die "Failed to remove /usr/local/etc/rc.d/${SERVICE_NAME}"
            fi
            ;;
        launchd)
            local system_plist="${SERVICE_ROOT}/Library/LaunchDaemons/com.komari.${SERVICE_NAME}.plist"
            local user_plist="${SERVICE_ROOT}$HOME/Library/LaunchAgents/com.komari.${SERVICE_NAME}.plist"
            if [ -f "$system_plist" ]; then
                log_info "Stopping and removing existing system launchd service..."
                if service_is_running launchd system; then
                    run_cmd launchctl bootout system "$system_plist" ||
                        die "Failed to stop launchd service com.komari.${SERVICE_NAME}"
                    if [ "$DRY_RUN" = false ] &&
                        ! wait_for_service_stopped launchd system; then
                        die "launchd service com.komari.${SERVICE_NAME} is still loaded"
                    fi
                fi
                run_cmd rm -f "$system_plist" || die "Failed to remove $system_plist"
            fi
            if [ -f "$user_plist" ]; then
                log_info "Stopping and removing existing user launchd service..."
                if service_is_running launchd "gui/$(id -u)"; then
                    run_cmd launchctl bootout "gui/$(id -u)" "$user_plist" ||
                        die "Failed to stop user launchd service com.komari.${SERVICE_NAME}"
                    if [ "$DRY_RUN" = false ] &&
                        ! wait_for_service_stopped launchd "gui/$(id -u)"; then
                        die "User launchd service com.komari.${SERVICE_NAME} is still loaded"
                    fi
                fi
                run_cmd rm -f "$user_plist" || die "Failed to remove $user_plist"
            fi
            ;;
        nixos)
            log_warning "NixOS detected: services are managed declaratively and cannot be removed by this script."
            ;;
        *)
            log_warning "Unknown init system; only the binary and configuration will be handled."
            ;;
    esac
}

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

json_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/\\t/g' -e 's/\r/\\r/g'
}

config_get_string() {
    local key="$1"
    [ -f "$CONFIG_FILE" ] || return 0
    sed -n 's/^[[:space:]]*"'"$key"'"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$CONFIG_FILE" |
        head -n 1
}

config_get_number() {
    local key="$1"
    [ -f "$CONFIG_FILE" ] || return 0
    sed -n 's/^[[:space:]]*"'"$key"'"[[:space:]]*:[[:space:]]*\(-\{0,1\}[0-9][0-9.]*\).*/\1/p' "$CONFIG_FILE" |
        head -n 1
}

# Replace (or append) one top-level key while preserving every other key.
# The value is passed through the environment so secrets never show up in the
# process argument list.
config_template() {
    cat <<'EOF'
{
  "endpoint": "",
  "token": "",
  "interval": 1.0,
  "ignore_unsafe_cert": false,
  "max_retries": 3,
  "reconnect_interval": 5,
  "info_report_interval": 5,
  "protocol_version": 1
}
EOF
}

config_validate() {
    local file="$1"
    if command -v jq >/dev/null 2>&1; then
        jq empty "$file" >/dev/null 2>&1
        return $?
    fi
    if command -v python3 >/dev/null 2>&1; then
        python3 -m json.tool "$file" >/dev/null 2>&1
        return $?
    fi
    # No JSON parser available; the generator only emits known-good output.
    return 0
}

# Rewrite a config without jq/python3. Existing endpoint/token/interval are
# preserved, but unknown keys cannot be carried over safely.
fallback_rewrite_config() {
    local file="$1"
    local tmp="$2"
    local endpoint token interval

    endpoint=$(config_get_string endpoint)
    token=$(config_get_string token)
    interval=$(config_get_number interval)
    [ -n "$ENDPOINT" ] && endpoint="$ENDPOINT"
    [ -n "$TOKEN" ] && token="$TOKEN"
    [ -n "$INTERVAL" ] && interval="$INTERVAL"
    [ -n "$interval" ] || interval="1.0"

    log_warning "jq/python3 not found; rewriting $file without preserving unknown keys"
    {
        printf '{\n'
        printf '  "endpoint": "%s",\n' "$(json_escape "$endpoint")"
        printf '  "token": "%s",\n' "$(json_escape "$token")"
        printf '  "interval": %s,\n' "$interval"
        printf '  "ignore_unsafe_cert": false,\n'
        printf '  "max_retries": 3,\n'
        printf '  "reconnect_interval": 5,\n'
        printf '  "info_report_interval": 5,\n'
        printf '  "protocol_version": 1\n'
        printf '}\n'
    } >"$tmp"
}

# Merge endpoint/token/interval into an existing (or new) config. The original
# file is never modified in place: the result is validated in a 0600 temp file
# and only then renamed over the target.
config_merge() {
    local file="$1"
    local dir base base_tmp tmp ovtmp backup old_umask
    local overrides sep

    dir=$(dirname "$file")
    if [ ! -d "$dir" ]; then
        mkdir -p "$dir" || die "Failed to create config directory: $dir"
    fi

    old_umask=$(umask)
    umask 077

    tmp=$(mktemp "${file}.tmp.XXXXXX") || {
        umask "$old_umask"
        die "Failed to create a temporary config file next to $file"
    }
    ovtmp=""
    backup=""
    base_tmp=""

    base="$file"
    if [ ! -s "$file" ] || grep -Eq '^[[:space:]]*\{[[:space:]]*\}[[:space:]]*$' "$file"; then
        base_tmp=$(mktemp "${file}.base.XXXXXX") || {
            rm -f "$tmp"
            umask "$old_umask"
            die "Failed to create a temporary base config file"
        }
        config_template >"$base_tmp"
        base="$base_tmp"
    fi

    if [ -f "$file" ]; then
        backup="${file}.bak.$$"
        cp -p "$file" "$backup" || {
            rm -f "$tmp"
            umask "$old_umask"
            die "Failed to back up $file"
        }
    fi

    if command -v jq >/dev/null 2>&1; then
        if [ -n "$ENDPOINT" ] || [ -n "$TOKEN" ] || [ -n "$INTERVAL" ]; then
            ovtmp=$(mktemp "${file}.overrides.XXXXXX") || {
                rm -f "$tmp" "$backup" "$base_tmp"
                umask "$old_umask"
                die "Failed to create a temporary overrides file"
            }
            overrides="{"
            sep=""
            if [ -n "$ENDPOINT" ]; then
                overrides="${overrides}${sep}\"endpoint\":\"$(json_escape "$ENDPOINT")\""
                sep=","
            fi
            if [ -n "$TOKEN" ]; then
                overrides="${overrides}${sep}\"token\":\"$(json_escape "$TOKEN")\""
                sep=","
            fi
            if [ -n "$INTERVAL" ]; then
                overrides="${overrides}${sep}\"interval\":${INTERVAL}"
            fi
            overrides="${overrides}}"
            printf '%s' "$overrides" >"$ovtmp"
            if ! jq --slurpfile ov "$ovtmp" '. * $ov[0]' "$base" >"$tmp"; then
                rm -f "$tmp" "$ovtmp" "$backup" "$base_tmp"
                umask "$old_umask"
                die "Failed to merge $file with jq; the original file was not changed"
            fi
        elif ! jq '.' "$base" >"$tmp"; then
            rm -f "$tmp" "$backup" "$base_tmp"
            umask "$old_umask"
            die "Failed to parse $file with jq; the original file was not changed"
        fi
    elif command -v python3 >/dev/null 2>&1; then
        if ! KOMARI_CFG_ENDPOINT="$ENDPOINT" \
            KOMARI_CFG_TOKEN="$TOKEN" \
            KOMARI_CFG_INTERVAL="$INTERVAL" \
            python3 - "$base" "$tmp" <<'PY'
import json
import os
import sys

src, dst = sys.argv[1], sys.argv[2]
data = {}
if os.path.exists(src) and os.path.getsize(src) > 0:
    with open(src, encoding="utf-8") as handle:
        data = json.load(handle)
if not isinstance(data, dict):
    raise SystemExit("config root must be a JSON object")

for key, env in (
    ("endpoint", "KOMARI_CFG_ENDPOINT"),
    ("token", "KOMARI_CFG_TOKEN"),
):
    value = os.environ.get(env, "")
    if value:
        data[key] = value

interval = os.environ.get("KOMARI_CFG_INTERVAL", "")
if interval:
    data["interval"] = float(interval)

with open(dst, "w", encoding="utf-8") as handle:
    json.dump(data, handle, ensure_ascii=False, indent=2)
    handle.write("\n")
PY
        then
            rm -f "$tmp" "$backup" "$base_tmp"
            umask "$old_umask"
            die "Failed to merge $file with python3; the original file was not changed"
        fi
    else
        if [ -s "$file" ] &&
            ! grep -Eq '^[[:space:]]*\{[[:space:]]*\}[[:space:]]*$' "$file"; then
            rm -f "$tmp" "$backup" "$base_tmp"
            umask "$old_umask"
            die "jq or python3 is required to merge existing config $file; install one and retry"
        fi
        if ! fallback_rewrite_config "$file" "$tmp"; then
            rm -f "$tmp" "$backup" "$base_tmp"
            umask "$old_umask"
            die "Failed to rewrite $file; the original file was not changed"
        fi
    fi

    if ! config_validate "$tmp"; then
        rm -f "$tmp" "$ovtmp" "$backup" "$base_tmp"
        umask "$old_umask"
        die "Generated config failed JSON validation; $file was not changed"
    fi

    chmod 600 "$tmp"
    if ! mv -f "$tmp" "$file"; then
        rm -f "$tmp" "$ovtmp" "$backup" "$base_tmp"
        umask "$old_umask"
        die "Failed to install the new config at $file"
    fi
    chmod 600 "$file"
    rm -f "$ovtmp" "$backup" "$base_tmp"
    umask "$old_umask"
}

install_config() {
    if [ "$CONFIG_EXPLICIT" = true ]; then
        if [ ! -f "$CONFIG_FILE" ]; then
            log_warning "Config file $CONFIG_FILE does not exist; the agent will fail to start until it is created."
        else
            chmod 600 "$CONFIG_FILE" || die "Failed to set permissions on $CONFIG_FILE"
        fi
        log_info "Using existing config file: $CONFIG_FILE"
        return 0
    fi

    if [ "$DRY_RUN" = true ]; then
        log_info "[dry-run] would write config: $CONFIG_FILE (mode 0600)"
        if [ -n "$ENDPOINT" ]; then
            log_info "[dry-run] endpoint: $ENDPOINT"
        fi
        if [ -n "$TOKEN" ]; then
            log_info "[dry-run] token: ******"
        fi
        if [ -n "$INTERVAL" ]; then
            log_info "[dry-run] interval: $INTERVAL"
        fi
        return 0
    fi

    log_step "Writing configuration: $CONFIG_FILE"
    config_merge "$CONFIG_FILE"

    if [ -z "$(config_get_string endpoint)" ] || [ -z "$(config_get_string token)" ]; then
        log_warning "endpoint and/or token are not set; edit $CONFIG_FILE before starting the agent."
    fi
    log_success "Configuration written to $CONFIG_FILE (mode 0600)"
}

# ---------------------------------------------------------------------------
# Service definitions
# ---------------------------------------------------------------------------

# Arguments placed on the agent command line. Secrets stay in config.json.
build_exec_args() {
    EXEC_ARGS=(--config "$CONFIG_FILE")
    if [ "${#AGENT_ARGS[@]}" -gt 0 ]; then
        EXEC_ARGS+=("${AGENT_ARGS[@]}")
    fi
}

# Escape a value for use inside a double-quoted shell string.
shell_dquote_content() {
    printf '%s' "$1" | sed -e 's/[\\"$`]/\\&/g'
}

# POSIX-shell quoting for a single word.
sh_quote() {
    printf '%q' "$1"
}

# Single-quoted word for values that OpenRC/rc.subr later eval.
sh_single_quote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# systemd ExecStart quoting: backslash, double quote and % specifiers.
systemd_quote_arg() {
    local arg="$1"
    arg="${arg//\\/\\\\}"
    arg="${arg//\"/\\\"}"
    arg="${arg//%/%%}"
    arg="${arg//\$/\$\$}"
    if [ -z "$arg" ] || [[ "$arg" == *[[:space:]]* ]] || [[ "$arg" == *\"* ]]; then
        printf '"%s"' "$arg"
    else
        printf '%s' "$arg"
    fi
}

# WorkingDirectory/other path directives: no quoting and no $ expansion.
# systemd still treats % as a specifier, so double it.
systemd_path_escape() {
    printf '%s' "${1//%/%%}"
}

format_exec_start_systemd() {
    local out=""
    local arg
    for arg in "$@"; do
        out="$out $(systemd_quote_arg "$arg")"
    done
    printf '%s' "${out# }"
}

# Quote for embedding in a double-quoted shell assignment (OpenRC command_args).
format_exec_start_double() {
    local out=""
    local arg
    for arg in "$@"; do
        if [ -z "$arg" ] || [[ "$arg" == *[[:space:]]* ]] ||
            [[ "$arg" == *[\"\$\`\\]* ]]; then
            out="$out \\\"$(shell_dquote_content "$arg")\\\""
        else
            out="$out $arg"
        fi
    done
    printf '%s' "${out# }"
}

# Quote for values that are stored and later eval'd (OpenRC command_args,
# rc.subr command_args): single quotes survive the assignment and the eval.
format_exec_start_eval() {
    local out=""
    local arg
    for arg in "$@"; do
        out="$out $(sh_single_quote "$arg")"
    done
    printf '%s' "${out# }"
}

# Quote for direct use in a shell script (upstart/procd exec lines).
format_exec_start_shell() {
    local out=""
    local arg
    for arg in "$@"; do
        out="$out $(sh_quote "$arg")"
    done
    printf '%s' "${out# }"
}

# Escape a value for a Nix double-quoted string.
nix_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\${/\\${/g'
}

xml_escape() {
    local value="$1"
    value="${value//&/&amp;}"
    value="${value//</&lt;}"
    value="${value//>/&gt;}"
    printf '%s' "$value"
}

write_service() {
    local init_system="$1"

    case "$init_system" in
        systemd)
            local unit="${SERVICE_ROOT}/etc/systemd/system/${SERVICE_NAME}.service"
            local exec_start working_directory
            exec_start=$(format_exec_start_systemd "$AGENT_PATH" "${EXEC_ARGS[@]}")
            working_directory=$(systemd_path_escape "$TARGET_DIR")
            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $unit"
                printf '%s\n' "ExecStart=$exec_start"
                return 0
            fi
            log_info "Using systemd for service management"
            cat >"$unit" <<EOF
[Unit]
Description=Komari Agent Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$exec_start
WorkingDirectory=$working_directory
Restart=always
RestartSec=5
User=root
UMask=0077

[Install]
WantedBy=multi-user.target
EOF
            run_cmd systemctl daemon-reload
            run_cmd systemctl enable "${SERVICE_NAME}.service"
            run_cmd systemctl restart "${SERVICE_NAME}.service"
            log_success "Systemd service configured and started"
            ;;
        openrc)
            local service_file="${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}"
            local command_path command_args directory
            command_path=$(shell_dquote_content "$(sh_single_quote "$AGENT_PATH")")
            command_args=$(shell_dquote_content "$(format_exec_start_eval "${EXEC_ARGS[@]}")")
            directory=$(shell_dquote_content "$(sh_single_quote "$TARGET_DIR")")
            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $service_file"
                return 0
            fi
            log_info "Using OpenRC for service management"
            cat >"$service_file" <<EOF
#!/sbin/openrc-run

name="Komari Agent Service"
description="Komari monitoring agent"
command="${command_path}"
command_args="${command_args}"
command_user="root"
directory="${directory}"
pidfile="/run/${SERVICE_NAME}.pid"
retry="SIGTERM/30"
supervisor=supervise-daemon

umask 077

depend() {
    need net
    after network
}
EOF
            chmod 0755 "$service_file"
            run_cmd rc-update add "$SERVICE_NAME" default
            run_cmd rc-service "$SERVICE_NAME" start
            log_success "OpenRC service configured and started"
            ;;
        procd)
            local service_file="${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}"
            local prog procd_command
            prog=$(shell_dquote_content "$AGENT_PATH")
            procd_command=$(format_exec_start_shell "$AGENT_PATH" "${EXEC_ARGS[@]}")
            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $service_file"
                return 0
            fi
            log_info "Using procd for service management"
            cat >"$service_file" <<EOF
#!/bin/sh /etc/rc.common

START=99
STOP=10

USE_PROCD=1

PROG="${prog}"

start_service() {
    procd_open_instance
    procd_set_param command ${procd_command}
    procd_set_param respawn
    procd_set_param stdout 1
    procd_set_param stderr 1
    procd_set_param user root
    procd_set_param umask 0077
    procd_close_instance
}

stop_service() {
    killall \$(basename "\$PROG")
}

reload_service() {
    stop
    start
}
EOF
            chmod 0755 "$service_file"
            run_cmd "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" enable
            run_cmd "${SERVICE_ROOT}/etc/init.d/${SERVICE_NAME}" start
            log_success "procd service configured and started"
            ;;
        launchd)
            local plist_dir plist_file service_user log_dir domain
            if [[ "$TARGET_DIR" =~ ^/Users/.* ]] || [ "$(id -u)" -ne 0 ]; then
                plist_dir="${SERVICE_ROOT}$HOME/Library/LaunchAgents"
                service_user="$(id -un)"
                log_dir="$HOME/Library/Logs"
                domain="gui/$(id -u)"
                log_info "Installing as user-level service (LaunchAgent)"
            else
                plist_dir="${SERVICE_ROOT}/Library/LaunchDaemons"
                service_user="root"
                log_dir="/var/log"
                domain="system"
                log_info "Installing as system-level service (LaunchDaemon)"
            fi
            plist_file="$plist_dir/com.komari.${SERVICE_NAME}.plist"

            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $plist_file"
                return 0
            fi
            mkdir -p "$plist_dir"
            {
                cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.komari.${SERVICE_NAME}</string>
    <key>ProgramArguments</key>
    <array>
        <string>$(xml_escape "$AGENT_PATH")</string>
EOF
                local arg
                for arg in "${EXEC_ARGS[@]}"; do
                    printf '        <string>%s</string>\n' "$(xml_escape "$arg")"
                done
                cat <<EOF
    </array>
    <key>WorkingDirectory</key>
    <string>$(xml_escape "$TARGET_DIR")</string>
    <key>Umask</key>
    <integer>63</integer>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>UserName</key>
    <string>$(xml_escape "$service_user")</string>
    <key>StandardOutPath</key>
    <string>$(xml_escape "$log_dir/${SERVICE_NAME}.log")</string>
    <key>StandardErrorPath</key>
    <string>$(xml_escape "$log_dir/${SERVICE_NAME}.log")</string>
</dict>
</plist>
EOF
            } >"$plist_file"

            run_cmd launchctl bootstrap "$domain" "$plist_file"
            log_success "launchd service configured and started"
            ;;
        upstart)
            local service_file="${SERVICE_ROOT}/etc/init/${SERVICE_NAME}.conf"
            local exec_start chdir_path
            exec_start=$(format_exec_start_shell "$AGENT_PATH" "${EXEC_ARGS[@]}")
            chdir_path=$(shell_dquote_content "$TARGET_DIR")
            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $service_file"
                return 0
            fi
            log_info "Using upstart for service management"
            cat >"$service_file" <<EOF
# KOMARI Agent
description "Komari Agent Service"

chdir "${chdir_path}"
start on filesystem or runlevel [2345]
stop on runlevel [!2345]

respawn
respawn limit 10 5
umask 077

console none

pre-start script
    test -x $(sh_quote "$AGENT_PATH") || { stop; exit 0; }
end script

script
    exec $exec_start
end script
EOF
            run_cmd initctl reload-configuration
            run_cmd initctl start "$SERVICE_NAME"
            log_success "Upstart service configured and started"
            ;;
        bsdrc)
            local service_file="${SERVICE_ROOT}/usr/local/etc/rc.d/${SERVICE_NAME}"
            local rc_name="${SERVICE_NAME//-/_}"
            local daemon_args agent_basename
            daemon_args=$(shell_dquote_content "$(format_exec_start_eval "$AGENT_PATH" "${EXEC_ARGS[@]}")")
            agent_basename=$(basename "$AGENT_PATH")
            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $service_file"
                return 0
            fi
            log_info "Using FreeBSD rc.d for service management"
            cat >"$service_file" <<EOF
#!/bin/sh
# PROVIDE: ${rc_name}
# REQUIRE: NETWORKING
# KEYWORD: shutdown

. /etc/rc.subr

name="${rc_name}"
rcvar="\${name}_enable"
pidfile="/var/run/\${name}.pid"
procname="${agent_basename}"
# komari-agent does not daemonize itself; /usr/sbin/daemon forks and writes
# the pidfile so rc.subr stop/status work.
command="/usr/sbin/daemon"
command_args="-p \${pidfile} ${daemon_args}"

umask 077

load_rc_config \$name
: \${${rc_name}_enable:="YES"}

run_rc_command "\$1"
EOF
            chmod 0755 "$service_file"
            run_cmd sysrc "${rc_name}_enable=YES"
            run_cmd service "$SERVICE_NAME" start
            log_success "FreeBSD rc.d service configured and started"
            ;;
        nixos)
            local exec_line working_directory
            exec_line=$(nix_escape "$(format_exec_start_systemd "$AGENT_PATH" "${EXEC_ARGS[@]}")")
            working_directory=$(nix_escape "$(systemd_path_escape "$TARGET_DIR")")
            log_warning "NixOS detected. System services must be configured declaratively."
            cat <<EOF

Add the following to your NixOS configuration:

systemd.services.${SERVICE_NAME} = {
  description = "Komari Agent Service";
  after = [ "network.target" ];
  wantedBy = [ "multi-user.target" ];
  serviceConfig = {
    Type = "simple";
    ExecStart = "${exec_line}";
    WorkingDirectory = "${working_directory}";
    Restart = "always";
    User = "root";
    UMask = "0077";
  };
};

Then run: sudo nixos-rebuild switch
EOF
            ;;
        *)
            die "Unsupported or unknown init system: $init_system (supported: systemd, openrc, procd, launchd, upstart, bsdrc, nixos)"
            ;;
    esac
}

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------

resolve_download() {
    detect_arch

    local version="$INSTALL_VERSION"
    if [ -z "$version" ]; then
        version="latest"
    fi

    local file_name="komari-agent-${OS_NAME}-${ARCH}"
    if [ "$OS_NAME" = "windows" ]; then
        file_name="${file_name}.exe"
    fi

    local download_path
    if [ "$version" = "latest" ]; then
        download_path="latest/download"
    else
        download_path="download/${version}"
    fi

    local base="https://github.com/${REPO}/releases"
    if [ -n "$GITHUB_PROXY" ]; then
        base="${GITHUB_PROXY%/}/${base}"
    fi

    ASSET_NAME="$file_name"
    DOWNLOAD_URL="${base}/${download_path}/${file_name}"
}

is_protected_dir() {
    local dir="$1"
    [ "$dir" != "/" ] && dir="${dir%/}"
    case "$dir" in
        / | /bin | /boot | /dev | /etc | /home | /lib | /lib64 | /media | /mnt | \
            /opt | /proc | /root | /run | /sbin | /srv | /sys | /tmp | /usr | \
            /usr/local | /var | /Library | /System | /Users)
            return 0
            ;;
    esac
    return 1
}

ensure_target_dir() {
    if [ -d "$TARGET_DIR" ]; then
        is_protected_dir "$TARGET_DIR" &&
            die "Refusing to change permissions on system directory $TARGET_DIR; use a dedicated subdirectory"
        local mode=""
        mode=$(stat -c %a "$TARGET_DIR" 2>/dev/null || stat -f %Lp "$TARGET_DIR" 2>/dev/null || true)
        if [ -n "$mode" ] && [ "$mode" != "700" ]; then
            log_warning "Install directory $TARGET_DIR has mode $mode; changing to 700 because auto-discovery.json contains a token."
            chmod 700 "$TARGET_DIR" || die "Failed to set permissions on $TARGET_DIR"
        fi
        if [ -f "$TARGET_DIR/auto-discovery.json" ]; then
            chmod 600 "$TARGET_DIR/auto-discovery.json" ||
                die "Failed to set permissions on $TARGET_DIR/auto-discovery.json"
        fi
        return 0
    fi

    log_step "Creating installation directory: $TARGET_DIR"
    mkdir -p "$TARGET_DIR" || die "Failed to create $TARGET_DIR"
    chmod 700 "$TARGET_DIR" || die "Failed to set permissions on $TARGET_DIR"
}

download_agent() {
    if [ "$DRY_RUN" = true ]; then
        log_info "[dry-run] would download $DOWNLOAD_URL"
        log_info "[dry-run] would install to $AGENT_PATH"
        return 0
    fi

    ensure_target_dir
    local tmp="${AGENT_PATH}.download.$$"

    log_step "Downloading $ASSET_NAME..."
    log_info "URL: $DOWNLOAD_URL"

    if ! curl -fL --retry 3 --retry-delay 2 --connect-timeout 15 -o "$tmp" "$DOWNLOAD_URL"; then
        rm -f "$tmp"
        die "Download failed: $DOWNLOAD_URL"
    fi

    chmod 0755 "$tmp"
    DOWNLOADED_TMP="$tmp"
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------

do_uninstall() {
    local init_system
    init_system=$(detect_init_system)

    log_info "Detected init system: $init_system"

    stop_previous_service "$init_system"

    if [ -f "$AGENT_PATH" ]; then
        log_info "Removing $AGENT_PATH"
        run_cmd rm -f "$AGENT_PATH"
    else
        log_info "Binary not present: $AGENT_PATH"
    fi

    if [ "$PURGE" = true ]; then
        log_info "Removing configuration and state files"
        run_cmd rm -f "$CONFIG_FILE" "$TARGET_DIR/auto-discovery.json"
        if [ "$DRY_RUN" = false ]; then
            rmdir "$TARGET_DIR" 2>/dev/null || true
        fi
    fi

    log_success "Komari Agent uninstalled"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    setup_colors
    detect_platform
    parse_args "$@"

    # parse_args detects the platform again when --install-dir was given
    # before the platform was known; make sure OS_NAME is always set.
    if [ -z "$OS_NAME" ]; then
        detect_platform
    fi

    AGENT_PATH="$TARGET_DIR/komari-agent"

    printf '%s===========================================%s\n' "$BOLD" "$NC"
    printf '%s    Komari Agent Installation Script     %s\n' "$BOLD" "$NC"
    printf '%s===========================================%s\n\n' "$BOLD" "$NC"

    log_config "Service name: $SERVICE_NAME"
    log_config "Install directory: $TARGET_DIR"
    log_config "Config file: $CONFIG_FILE"
    log_config "GitHub proxy: ${GITHUB_PROXY:-(direct)}"
    if [ -n "$TOKEN" ]; then
        log_config "Token: ******"
    else
        log_config "Token: (unset)"
    fi
    log_config "Binary arguments: ${AGENT_ARGS[*]:-}"
    if [ -n "$INSTALL_VERSION" ]; then
        log_config "Agent version: $INSTALL_VERSION"
    else
        log_config "Agent version: latest"
    fi
    log_config "Dry run: $DRY_RUN"
    echo ""

    if [ "$UNINSTALL" = true ]; then
        check_root
        do_uninstall
        return 0
    fi

    check_root
    install_dependencies

    local init_system
    init_system=$(detect_init_system)
    log_info "Detected OS: $OS_NAME, Architecture: $(uname -m)"
    log_info "Detected init system: $init_system"

    resolve_download
    build_exec_args

    download_agent

    stop_previous_service "$init_system"

    if [ "$DRY_RUN" = false ]; then
        log_step "Installing $AGENT_PATH"
        mv -f "$DOWNLOADED_TMP" "$AGENT_PATH"
        chmod 0755 "$AGENT_PATH"
        log_success "komari-agent installed to $AGENT_PATH"
    fi

    install_config
    write_service "$init_system"

    echo ""
    printf '%s===========================================%s\n' "$BOLD" "$NC"
    if [ "$DRY_RUN" = true ]; then
        log_success "Dry run completed; nothing was changed."
    else
        log_success "komari-agent installation completed!"
    fi
    log_config "Service: $SERVICE_NAME"
    log_config "Binary: $AGENT_PATH"
    log_config "Config: $CONFIG_FILE"
    printf '%s===========================================%s\n' "$BOLD" "$NC"
}

# Allow tests to source the helpers without running the installer.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
