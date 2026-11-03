#!/usr/bin/env bash
# Render service definitions for a path containing spaces and verify that the
# generated shell files parse. This runs without root by redirecting every
# service path under a temporary directory.
set -euo pipefail

ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

export KOMARI_SERVICE_ROOT="$ROOT/root"
export KOMARI_SERVICE_RENDER_ONLY=1
mkdir -p \
    "$KOMARI_SERVICE_ROOT/etc/systemd/system" \
    "$KOMARI_SERVICE_ROOT/etc/init.d" \
    "$KOMARI_SERVICE_ROOT/etc/init" \
    "$KOMARI_SERVICE_ROOT/usr/local/etc/rc.d"

# shellcheck disable=SC1091
# shellcheck source=../install.sh
source "$(cd "$(dirname "$0")/.." && pwd)/install.sh"

SERVICE_NAME="komari-agent-test"
TARGET_DIR="$KOMARI_SERVICE_ROOT/opt/a b"
AGENT_PATH="$TARGET_DIR/komari-agent"
CONFIG_FILE="$TARGET_DIR/config.json"
mkdir -p "$TARGET_DIR"
chmod 700 "$TARGET_DIR"
# shellcheck disable=SC2034 # consumed by write_service from install.sh
EXEC_ARGS=(--config "$CONFIG_FILE" "--name=a b" "--pct=100%" "--dollar=\$HOME")
# shellcheck disable=SC2034 # consumed by write_service from install.sh
DRY_RUN=false

# systemd
write_service systemd
unit="$KOMARI_SERVICE_ROOT/etc/systemd/system/${SERVICE_NAME}.service"
grep -Fq 'UMask=0077' "$unit"
grep -Fq 'WorkingDirectory="'"$TARGET_DIR"'"' "$unit"
grep -Fq 'ExecStart="'"$AGENT_PATH"'" --config "'"$CONFIG_FILE"'"' "$unit"
# shellcheck disable=SC2016 # literal systemd escaping
grep -Fq -- '--pct=100%%' "$unit"
# shellcheck disable=SC2016 # literal systemd escaping
grep -Fq -- '--dollar=$$HOME' "$unit"

# OpenRC
write_service openrc
openrc="$KOMARI_SERVICE_ROOT/etc/init.d/${SERVICE_NAME}"
bash -n "$openrc"
grep -Fq 'umask 077' "$openrc"
grep -Fq 'command_args="--config \"'"$CONFIG_FILE"'\"' "$openrc" || {
    echo "OpenRC command_args quoting is wrong:" >&2
    sed -n '/command_args=/p' "$openrc" >&2
    exit 1
}

# procd overwrites the same /etc/init.d path.
write_service procd
procd="$KOMARI_SERVICE_ROOT/etc/init.d/${SERVICE_NAME}"
bash -n "$procd"
grep -Fq 'procd_set_param umask 0077' "$procd"
grep -Fq 'procd_set_param command '"${TARGET_DIR// /\\ }"'/komari-agent' "$procd"

# upstart
write_service upstart
upstart="$KOMARI_SERVICE_ROOT/etc/init/${SERVICE_NAME}.conf"
grep -Fq 'umask 077' "$upstart"
grep -Fq 'test -x '"${TARGET_DIR// /\\ }"'/komari-agent' "$upstart"
grep -Fq 'exec '"${TARGET_DIR// /\\ }"'/komari-agent' "$upstart"

# FreeBSD rc.d
write_service bsdrc
bsdrc="$KOMARI_SERVICE_ROOT/usr/local/etc/rc.d/${SERVICE_NAME}"
bash -n "$bsdrc"
grep -Fq 'command="/usr/sbin/daemon"' "$bsdrc"
# shellcheck disable=SC2016 # ${pidfile} must stay literal in the rc script
grep -Fq 'command_args="-p ${pidfile}' "$bsdrc"
grep -Fq 'umask 077' "$bsdrc"

# NixOS is printed, not written.
nix_output=$(write_service nixos)
printf '%s\n' "$nix_output" | grep -Fq 'UMask = "0077";'
printf '%s\n' "$nix_output" | grep -Fq 'ExecStart = "'

echo "service rendering tests passed"
