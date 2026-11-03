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
        armv7* | armv6*)
            case "$OS_NAME" in
                freebsd | linux) ARCH="arm" ;;
                *) die "32-bit ARM architecture is not supported on $OS_NAME" ;;
            esac
            ;;
        *)
            die "Unsupported architecture: $machine on $OS_NAME"
            ;;
    esac
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
            --endpoint=*)
                ENDPOINT="${1#*=}"
                shift
                ;;
            --token | -t)
                need_value "$@"
                TOKEN="$2"
                shift 2
                ;;
            --token=*)
                TOKEN="${1#*=}"
                shift
                ;;
            --interval | -i)
                need_value "$@"
                INTERVAL="$2"
                shift 2
                ;;
            --interval=*)
                INTERVAL="${1#*=}"
                shift
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
stop_previous_service() {
    local init_system="$1"

    log_step "Checking for a previous installation..."

    case "$init_system" in
        systemd)
            local unit="/etc/systemd/system/${SERVICE_NAME}.service"
            if [ -f "$unit" ]; then
                log_info "Stopping and disabling existing systemd service..."
                run_cmd systemctl stop "${SERVICE_NAME}.service" || true
                run_cmd systemctl disable "${SERVICE_NAME}.service" || true
                run_cmd rm -f "$unit"
                run_cmd systemctl daemon-reload
            fi
            ;;
        openrc)
            if [ -f "/etc/init.d/${SERVICE_NAME}" ]; then
                log_info "Stopping and disabling existing OpenRC service..."
                run_cmd rc-service "$SERVICE_NAME" stop || true
                run_cmd rc-update del "$SERVICE_NAME" default || true
                run_cmd rm -f "/etc/init.d/${SERVICE_NAME}"
            fi
            ;;
        procd)
            if [ -f "/etc/init.d/${SERVICE_NAME}" ]; then
                log_info "Stopping and disabling existing procd service..."
                run_cmd "/etc/init.d/${SERVICE_NAME}" stop || true
                run_cmd "/etc/init.d/${SERVICE_NAME}" disable || true
                run_cmd rm -f "/etc/init.d/${SERVICE_NAME}"
            fi
            ;;
        upstart)
            if [ -f "/etc/init/${SERVICE_NAME}.conf" ]; then
                log_info "Stopping and removing existing upstart service..."
                run_cmd initctl stop "$SERVICE_NAME" || true
                run_cmd rm -f "/etc/init/${SERVICE_NAME}.conf"
            fi
            ;;
        bsdrc)
            if [ -f "/usr/local/etc/rc.d/${SERVICE_NAME}" ]; then
                log_info "Stopping and disabling existing FreeBSD rc.d service..."
                run_cmd service "$SERVICE_NAME" stop || true
                run_cmd sysrc -x "${SERVICE_NAME//-/_}_enable" || true
                run_cmd rm -f "/usr/local/etc/rc.d/${SERVICE_NAME}"
            fi
            ;;
        launchd)
            local system_plist="/Library/LaunchDaemons/com.komari.${SERVICE_NAME}.plist"
            local user_plist="$HOME/Library/LaunchAgents/com.komari.${SERVICE_NAME}.plist"
            if [ -f "$system_plist" ]; then
                log_info "Stopping and removing existing system launchd service..."
                run_cmd launchctl bootout system "$system_plist" || true
                run_cmd rm -f "$system_plist"
            fi
            if [ -f "$user_plist" ]; then
                log_info "Stopping and removing existing user launchd service..."
                run_cmd launchctl bootout "gui/$(id -u)" "$user_plist" || true
                run_cmd rm -f "$user_plist"
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
config_set() {
    local key="$1"
    local value="$2"
    local quoted="$3"
    local tmp="${CONFIG_FILE}.tmp.$$"

    KEY="$key" VALUE="$value" QUOTED="$quoted" awk '
        BEGIN {
            key = ENVIRON["KEY"]
            value = ENVIRON["VALUE"]
            quoted = ENVIRON["QUOTED"] == "1"
            found = 0
            n = 0
        }
        {
            if (!found && $0 ~ ("^[[:space:]]*\"" key "\"[[:space:]]*:")) {
                indent = $0
                sub(/[^ \t].*/, "", indent)
                comma = ($0 ~ /,[[:space:]]*$/) ? "," : ""
                if (quoted) {
                    out[++n] = indent "\"" key "\": \"" value "\"" comma
                } else {
                    out[++n] = indent "\"" key "\": " value comma
                }
                found = 1
                next
            }
            out[++n] = $0
        }
        END {
            if (!found && n > 0 && out[n] ~ /^[[:space:]]*}[[:space:]]*$/) {
                # Insert the new key before the closing brace and make sure
                # the previous entry is comma-terminated.
                last_entry = 0
                for (i = 1; i < n; i++) {
                    if (out[i] !~ /^[[:space:]]*$/ && out[i] !~ /^[[:space:]]*\{[[:space:]]*$/) {
                        last_entry = i
                    }
                }
                for (i = 1; i < n; i++) {
                    line = out[i]
                    if (i == last_entry && line !~ /,[[:space:]]*$/) {
                        line = line ","
                    }
                    print line
                }
                if (quoted) {
                    print "  \"" key "\": \"" value "\""
                } else {
                    print "  \"" key "\": " value
                }
                print out[n]
            } else {
                if (!found) {
                    if (quoted) {
                        out[++n] = "  \"" key "\": \"" value "\","
                    } else {
                        out[++n] = "  \"" key "\": " value ","
                    }
                }
                for (i = 1; i <= n; i++) {
                    print out[i]
                }
            }
        }
    ' "$CONFIG_FILE" >"$tmp"

    mv -f "$tmp" "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
}

write_default_config() {
    local old_umask
    old_umask=$(umask)
    umask 077
    cat >"$CONFIG_FILE" <<'EOF'
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
    umask "$old_umask"
    chmod 600 "$CONFIG_FILE"
}

install_config() {
    if [ "$CONFIG_EXPLICIT" = true ]; then
        if [ ! -f "$CONFIG_FILE" ]; then
            log_warning "Config file $CONFIG_FILE does not exist; the agent will fail to start until it is created."
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

    local endpoint="${ENDPOINT}"
    local token="${TOKEN}"
    local interval="${INTERVAL}"

    if [ ! -f "$CONFIG_FILE" ]; then
        log_step "Creating configuration: $CONFIG_FILE"
        write_default_config
    else
        log_step "Updating configuration: $CONFIG_FILE"
        # Keep existing values for keys the caller did not override.
        [ -n "$endpoint" ] || endpoint=$(config_get_string endpoint)
        [ -n "$token" ] || token=$(config_get_string token)
        [ -n "$interval" ] || interval=$(config_get_number interval)
    fi

    if [ -n "$endpoint" ]; then
        config_set endpoint "$(json_escape "$endpoint")" 1
    fi
    if [ -n "$token" ]; then
        config_set token "$(json_escape "$token")" 1
    fi
    if [ -n "$interval" ]; then
        config_set interval "$interval" 0
    fi

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

format_exec_start() {
    local out="$1"
    local arg
    shift
    for arg in "$@"; do
        if [ -z "$arg" ] || [[ "$arg" == *[[:space:]]* ]] || [[ "$arg" == *\"* ]]; then
            arg="${arg//\"/\\\"}"
            out="$out \"$arg\""
        else
            out="$out $arg"
        fi
    done
    printf '%s' "$out"
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
            local unit="/etc/systemd/system/${SERVICE_NAME}.service"
            local exec_start
            exec_start=$(format_exec_start "$AGENT_PATH" "${EXEC_ARGS[@]}")
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
WorkingDirectory=$TARGET_DIR
Restart=always
RestartSec=5
User=root

[Install]
WantedBy=multi-user.target
EOF
            run_cmd systemctl daemon-reload
            run_cmd systemctl enable "${SERVICE_NAME}.service"
            run_cmd systemctl restart "${SERVICE_NAME}.service"
            log_success "Systemd service configured and started"
            ;;
        openrc)
            local service_file="/etc/init.d/${SERVICE_NAME}"
            local args
            args=$(format_exec_start "" "${EXEC_ARGS[@]}")
            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $service_file"
                return 0
            fi
            log_info "Using OpenRC for service management"
            cat >"$service_file" <<EOF
#!/sbin/openrc-run

name="Komari Agent Service"
description="Komari monitoring agent"
command="${AGENT_PATH}"
command_args="${args# }"
command_user="root"
directory="${TARGET_DIR}"
pidfile="/run/${SERVICE_NAME}.pid"
retry="SIGTERM/30"
supervisor=supervise-daemon

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
            local service_file="/etc/init.d/${SERVICE_NAME}"
            local args
            args=$(format_exec_start "" "${EXEC_ARGS[@]}")
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

PROG="${AGENT_PATH}"
ARGS="${args# }"

start_service() {
    procd_open_instance
    procd_set_param command "\$PROG" \$ARGS
    procd_set_param respawn
    procd_set_param stdout 1
    procd_set_param stderr 1
    procd_set_param user root
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
            run_cmd "/etc/init.d/${SERVICE_NAME}" enable
            run_cmd "/etc/init.d/${SERVICE_NAME}" start
            log_success "procd service configured and started"
            ;;
        launchd)
            local plist_dir plist_file service_user log_dir domain
            if [[ "$TARGET_DIR" =~ ^/Users/.* ]] || [ "$(id -u)" -ne 0 ]; then
                plist_dir="$HOME/Library/LaunchAgents"
                service_user="$(id -un)"
                log_dir="$HOME/Library/Logs"
                domain="gui/$(id -u)"
                log_info "Installing as user-level service (LaunchAgent)"
            else
                plist_dir="/Library/LaunchDaemons"
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
            local service_file="/etc/init/${SERVICE_NAME}.conf"
            local exec_start
            exec_start=$(format_exec_start "$AGENT_PATH" "${EXEC_ARGS[@]}")
            if [ "$DRY_RUN" = true ]; then
                log_info "[dry-run] would write $service_file"
                return 0
            fi
            log_info "Using upstart for service management"
            cat >"$service_file" <<EOF
# KOMARI Agent
description "Komari Agent Service"

chdir $TARGET_DIR
start on filesystem or runlevel [2345]
stop on runlevel [!2345]

respawn
respawn limit 10 5
umask 022

console none

pre-start script
    test -x $AGENT_PATH || { stop; exit 0; }
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
            local service_file="/usr/local/etc/rc.d/${SERVICE_NAME}"
            local rc_name="${SERVICE_NAME//-/_}"
            local args
            args=$(format_exec_start "" "${EXEC_ARGS[@]}")
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
command="${AGENT_PATH}"
command_args="${args# }"
pidfile="/var/run/\${name}.pid"

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
            log_warning "NixOS detected. System services must be configured declaratively."
            cat <<EOF

Add the following to your NixOS configuration:

systemd.services.${SERVICE_NAME} = {
  description = "Komari Agent Service";
  after = [ "network.target" ];
  wantedBy = [ "multi-user.target" ];
  serviceConfig = {
    Type = "simple";
    ExecStart = "$(format_exec_start "$AGENT_PATH" "${EXEC_ARGS[@]}")";
    WorkingDirectory = "$TARGET_DIR";
    Restart = "always";
    User = "root";
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

download_agent() {
    mkdir -p "$TARGET_DIR"
    local tmp="${AGENT_PATH}.download.$$"

    if [ "$DRY_RUN" = true ]; then
        log_info "[dry-run] would download $DOWNLOAD_URL"
        log_info "[dry-run] would install to $AGENT_PATH"
        return 0
    fi

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
