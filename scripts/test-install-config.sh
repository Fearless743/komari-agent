#!/usr/bin/env bash
# shellcheck disable=SC1091
# Config merge tests for install.sh:
#   * jq/python3 path preserves unknown keys in single-line JSON;
#   * malformed JSON is never overwritten;
#   * without jq/python3 an existing non-empty config is left untouched;
#   * without a parser a new config is still created from the template.
set -euo pipefail

ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

run_case() {
    local target="$1" endpoint="$2" token="$3" interval="$4"
    /usr/bin/bash -c '
        source "$1"
        TARGET_DIR="$2"
        CONFIG_FILE="$TARGET_DIR/config.json"
        CONFIG_EXPLICIT=false
        DRY_RUN=false
        ENDPOINT="$3"
        TOKEN="$4"
        INTERVAL="$5"
        install_config
    ' _ "$REPO_ROOT/install.sh" "$target" "$endpoint" "$token" "$interval"
}

# jq/python3 merge: single-line JSON with an unknown key.
case_dir="$ROOT/merge"
mkdir -p "$case_dir"
printf '%s\n' '{"endpoint":"old","token":"old","custom_dns":"1.1.1.1","interval":9}' \
    >"$case_dir/config.json"
run_case "$case_dir" "https://new.example.com" "new-token" "" >/dev/null
python3 - "$case_dir/config.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    data = json.load(handle)
assert data["endpoint"] == "https://new.example.com"
assert data["token"] == "new-token"
assert data["custom_dns"] == "1.1.1.1"
assert data["interval"] == 9
PY
[ "$(stat -c %a "$case_dir/config.json" 2>/dev/null || stat -f %Lp "$case_dir/config.json")" = "600" ] ||
    fail "merged config mode is not 600"

# Malformed JSON: preserve the original file and fail.
case_dir="$ROOT/malformed"
mkdir -p "$case_dir"
printf '{ bad json' >"$case_dir/config.json"
before=$(sha256sum "$case_dir/config.json" | cut -d' ' -f1)
if run_case "$case_dir" "" "x" "" >/dev/null 2>&1; then
    fail "malformed config was accepted"
fi
after=$(sha256sum "$case_dir/config.json" | cut -d' ' -f1)
[ "$before" = "$after" ] || fail "malformed config was modified"

# PATH without jq/python3: existing non-empty config must not be rewritten.
case_dir="$ROOT/no-parser-existing"
bin="$ROOT/bin"
mkdir -p "$case_dir" "$bin"
for cmd in mktemp dirname mkdir cp grep cat chmod mv rm sed head; do
    ln -s "$(command -v "$cmd")" "$bin/$cmd"
done
printf '%s\n' '{"endpoint":"old","token":"old","custom_dns":"8.8.8.8"}' \
    >"$case_dir/config.json"
before=$(sha256sum "$case_dir/config.json" | cut -d' ' -f1)
if PATH="$bin" run_case "$case_dir" "https://new" "new" "" >/dev/null 2>&1; then
    fail "no-parser merge unexpectedly succeeded"
fi
after=$(sha256sum "$case_dir/config.json" | cut -d' ' -f1)
[ "$before" = "$after" ] || fail "no-parser merge modified the original config"

# PATH without jq/python3: multi-line config with an unknown key is also
# preserved (no parser means no safe merge).
case_dir="$ROOT/no-parser-multiline"
mkdir -p "$case_dir"
printf '{\n  "endpoint": "old",\n  "token": "old",\n  "custom_dns": "9.9.9.9"\n}\n' \
    >"$case_dir/config.json"
before=$(sha256sum "$case_dir/config.json" | cut -d' ' -f1)
if PATH="$bin" run_case "$case_dir" "https://new" "new" "" >/dev/null 2>&1; then
    fail "no-parser multi-line merge unexpectedly succeeded"
fi
after=$(sha256sum "$case_dir/config.json" | cut -d' ' -f1)
[ "$before" = "$after" ] || fail "no-parser multi-line merge modified the original config"

# PATH without jq/python3: a new config is still created.
case_dir="$ROOT/no-parser-new"
mkdir -p "$case_dir"
PATH="$bin" run_case "$case_dir" "https://new" "new" "2" >/dev/null
python3 - "$case_dir/config.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    data = json.load(handle)
assert data["endpoint"] == "https://new"
assert data["token"] == "new"
assert data["interval"] == 2
assert data["protocol_version"] == 1
PY

echo "config merge tests passed"
