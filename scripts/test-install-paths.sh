#!/usr/bin/env bash
# shellcheck disable=SC1091
# Verify that path traversal and symlinks cannot redirect chmod to another
# directory or file.
set -euo pipefail

ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

mode() {
    stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"
}

run_ensure() {
    local target="$1"
    /usr/bin/bash -c '
        source "$1"
        TARGET_DIR="$2"
        ensure_target_dir
    ' _ "$REPO_ROOT/install.sh" "$target"
}

run_config() {
    local config="$1"
    /usr/bin/bash -c '
        source "$1"
        CONFIG_FILE="$2"
        CONFIG_EXPLICIT=true
        DRY_RUN=false
        install_config
    ' _ "$REPO_ROOT/install.sh" "$config"
}

chmod 755 "$ROOT"

# 1. A lexical ".." must be rejected before any chmod.
mkdir -p "$ROOT/traversal"
chmod 755 "$ROOT/traversal"
if run_ensure "$ROOT/traversal/.." >/dev/null 2>&1; then
    fail "install directory containing .. was accepted"
fi
[ "$(mode "$ROOT")" = "755" ] || fail ".. traversal changed the parent mode"

# 2. A symlinked install directory must be rejected without chmod'ing its target.
mkdir -p "$ROOT/victim-dir"
chmod 755 "$ROOT/victim-dir"
ln -s "$ROOT/victim-dir" "$ROOT/link-dir"
if run_ensure "$ROOT/link-dir" >/dev/null 2>&1; then
    fail "symlinked install directory was accepted"
fi
[ "$(mode "$ROOT/victim-dir")" = "755" ] || fail "symlink target mode changed"

# 3. A symlinked parent must be rejected.
ln -s "$ROOT/victim-dir" "$ROOT/link-parent"
if run_ensure "$ROOT/link-parent/new" >/dev/null 2>&1; then
    fail "install directory under a symlinked parent was accepted"
fi
[ "$(mode "$ROOT/victim-dir")" = "755" ] || fail "symlinked parent target changed"
[ ! -e "$ROOT/victim-dir/new" ] || fail "directory was created through a symlink"

# 4. auto-discovery.json symlink must not be chmod'ed.
mkdir -p "$ROOT/target"
chmod 755 "$ROOT/target"
printf 'not-a-secret\n' >"$ROOT/victim-file"
chmod 644 "$ROOT/victim-file"
ln -s "$ROOT/victim-file" "$ROOT/target/auto-discovery.json"
if run_ensure "$ROOT/target" >/dev/null 2>&1; then
    fail "symlinked auto-discovery.json was accepted"
fi
[ "$(mode "$ROOT/victim-file")" = "644" ] || fail "auto-discovery symlink target mode changed"

# 5. --config symlink and .. must not be chmod'ed.
if run_config "$ROOT/target/auto-discovery.json" >/dev/null 2>&1; then
    fail "symlinked --config was accepted"
fi
[ "$(mode "$ROOT/victim-file")" = "644" ] || fail "--config symlink target mode changed"

if run_config "$ROOT/target/../victim-file" >/dev/null 2>&1; then
    fail "--config containing .. was accepted"
fi
[ "$(mode "$ROOT/victim-file")" = "644" ] || fail "--config .. changed the target mode"

# 6. A normal new directory is still created with mode 700.
run_ensure "$ROOT/normal" >/dev/null
[ "$(mode "$ROOT/normal")" = "700" ] || fail "normal install directory mode is not 700"

echo "path safety tests passed"
