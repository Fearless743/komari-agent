#!/usr/bin/env bash
# shellcheck disable=SC2154
# shellcheck disable=SC2034
# Render service definitions for a path containing spaces, %, $ and verify:
#   * systemd unit syntax with systemd-analyze verify (when available);
#   * OpenRC command/directory/command_args survive eval;
#   * rc.d command_args survives eval;
#   * upstart/NixOS escaping and umask settings.
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
TARGET_DIR="$KOMARI_SERVICE_ROOT/opt/a b/100%/\$HOME/\`tick\`"
AGENT_PATH="$TARGET_DIR/komari-agent"
CONFIG_FILE="$TARGET_DIR/config.json"
mkdir -p "$TARGET_DIR"
chmod 700 "$TARGET_DIR"
printf '#!/bin/sh\nexit 0\n' >"$AGENT_PATH"
chmod 0755 "$AGENT_PATH"
# shellcheck disable=SC2034 # consumed by write_service from install.sh
EXEC_ARGS=(--config "$CONFIG_FILE" "--name=a b" "--pct=100%" "--dollar=\$HOME")
# shellcheck disable=SC2034 # consumed by write_service from install.sh
DRY_RUN=false

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# systemd
write_service systemd
unit="$KOMARI_SERVICE_ROOT/etc/systemd/system/${SERVICE_NAME}.service"
expected_wd=$(systemd_path_escape "$TARGET_DIR")
grep -Fq "WorkingDirectory=$expected_wd" "$unit" ||
    fail "systemd WorkingDirectory is not the unquoted, %-escaped path"
if grep -Fq 'WorkingDirectory="' "$unit"; then
    fail "systemd WorkingDirectory must not be quoted"
fi
expected_exec=$(format_exec_start_systemd "$AGENT_PATH" "${EXEC_ARGS[@]}")
grep -Fq "ExecStart=$expected_exec" "$unit" || fail "systemd ExecStart mismatch"
# shellcheck disable=SC2016 # literal systemd escaping
grep -Fq -- '--pct=100%%' "$unit" || fail "systemd % was not doubled"
# shellcheck disable=SC2016 # literal systemd escaping
grep -Fq -- '--dollar=$$HOME' "$unit" || fail "systemd \$ was not doubled"
grep -Fq 'UMask=0077' "$unit" || fail "systemd UMask missing"

if command -v systemd-analyze >/dev/null 2>&1; then
    # verify() does not collapse $$ to $, so provide the literal path it
    # inspects; systemd itself expands $$ to $ at runtime.
    literal_agent="${AGENT_PATH//\$/\$\$}"
    mkdir -p "$(dirname "$literal_agent")"
    cp "$AGENT_PATH" "$literal_agent"
    if ! systemd-analyze verify "$unit" >"$ROOT/systemd-verify.log" 2>&1; then
        cat "$ROOT/systemd-verify.log" >&2
        fail "systemd-analyze verify rejected the generated unit"
    fi
else
    echo "note: systemd-analyze not available; skipping unit verification"
fi

# OpenRC: command/directory/command_args are eval'd by openrc-run.
write_service openrc
openrc="$KOMARI_SERVICE_ROOT/etc/init.d/${SERVICE_NAME}"
bash -n "$openrc"
grep -Fq 'umask 077' "$openrc" || fail "OpenRC umask missing"
eval "$(grep '^command=' "$openrc")"
eval "set -- $command"
[ "$1" = "$AGENT_PATH" ] || fail "OpenRC command did not survive eval: $1"
eval "$(grep '^directory=' "$openrc")"
eval "set -- $directory"
[ "$1" = "$TARGET_DIR" ] || fail "OpenRC directory did not survive eval: $1"
eval "$(grep '^command_args=' "$openrc")"
eval "set -- $command_args"
[ "$1" = "--config" ] || fail "OpenRC command_args[1] mismatch"
[ "$2" = "$CONFIG_FILE" ] || fail "OpenRC command_args[2] mismatch"
[ "$3" = "--name=a b" ] || fail "OpenRC command_args[3] mismatch"
[ "$5" = "--dollar=\$HOME" ] || fail "OpenRC command_args[5] mismatch"

# procd overwrites the same /etc/init.d path.
write_service procd
procd="$KOMARI_SERVICE_ROOT/etc/init.d/${SERVICE_NAME}"
bash -n "$procd"
grep -Fq 'procd_set_param umask 0077' "$procd" || fail "procd umask missing"
expected_procd=$(format_exec_start_shell "$AGENT_PATH" "${EXEC_ARGS[@]}")
grep -Fq "procd_set_param command $expected_procd" "$procd" ||
    fail "procd command mismatch"

# upstart
write_service upstart
upstart="$KOMARI_SERVICE_ROOT/etc/init/${SERVICE_NAME}.conf"
grep -Fq 'umask 077' "$upstart" || fail "upstart umask missing"
grep -Fq "test -x $(sh_quote "$AGENT_PATH")" "$upstart" ||
    fail "upstart test -x is not shell-quoted"
grep -Fq "exec $(format_exec_start_shell "$AGENT_PATH" "${EXEC_ARGS[@]}")" "$upstart" ||
    fail "upstart exec line mismatch"

# FreeBSD rc.d: command_args is eval'd by rc.subr.
write_service bsdrc
bsdrc="$KOMARI_SERVICE_ROOT/usr/local/etc/rc.d/${SERVICE_NAME}"
bash -n "$bsdrc"
grep -Fq 'command="/usr/sbin/daemon"' "$bsdrc" || fail "rc.d daemon wrapper missing"
grep -Fq 'umask 077' "$bsdrc" || fail "rc.d umask missing"
name=komari_agent_test
eval "$(grep '^pidfile=' "$bsdrc")"
eval "$(grep '^command_args=' "$bsdrc")"
eval "set -- $command_args"
[ "$1" = "-p" ] || fail "rc.d command_args[1] mismatch"
[ "$2" = "$pidfile" ] || fail "rc.d command_args[2] mismatch"
[ "$3" = "$AGENT_PATH" ] || fail "rc.d command_args[3] mismatch"
[ "$5" = "$CONFIG_FILE" ] || fail "rc.d command_args[5] mismatch"

# NixOS is printed, not written.
nix_output=$(write_service nixos)
expected_nix_wd=$(nix_escape "$(systemd_path_escape "$TARGET_DIR")")
printf '%s\n' "$nix_output" | grep -Fq "WorkingDirectory = \"$expected_nix_wd\";" ||
    fail "NixOS WorkingDirectory escaping mismatch"
printf '%s\n' "$nix_output" | grep -Fq 'UMask = "0077";' ||
    fail "NixOS UMask missing"

echo "service rendering tests passed"
