#!/usr/bin/env bash
#
# Build the release matrix with Cargo.
#
# Asset names match the Go agent and the self-updater:
#   komari-agent-{goos}-{goarch}[.exe]
#
# The version is injected at compile time through KOMARI_AGENT_VERSION and
# KOMARI_AGENT_VERSION_HASH (see crates/agent-cli/src/lib.rs). Because those
# values are read with option_env!, agent-cli is cleaned before each build so
# a stale artifact cannot keep an old version.
#
# Cross-compilation prerequisites:
#   * rustup targets for every matrix entry;
#   * a linker for each target (cargo-zigbuild is used automatically when
#     installed, otherwise the platform linker must be configured);
#   * macOS targets require a macOS host or an osxcross toolchain.
#
# Usage: ./build_all.sh [--help] [--list]

set -uo pipefail

BUILD_DIR="build"
ONLY="${KOMARI_BUILD_TARGETS:-}"
BUILD_TOOL="${KOMARI_BUILD_TOOL:-auto}"
DRY_RUN="${KOMARI_BUILD_DRY_RUN:-0}"
INSTALL_TARGETS="${KOMARI_INSTALL_TARGETS:-1}"

# goos goarch rust-target
TARGETS=(
    "linux amd64 x86_64-unknown-linux-musl"
    "linux arm64 aarch64-unknown-linux-musl"
    "linux 386 i686-unknown-linux-musl"
    "linux arm armv7-unknown-linux-musleabihf"
    "windows amd64 x86_64-pc-windows-gnu"
    "windows arm64 aarch64-pc-windows-gnullvm"
    "windows 386 i686-pc-windows-gnu"
    "darwin amd64 x86_64-apple-darwin"
    "darwin arm64 aarch64-apple-darwin"
    "freebsd amd64 x86_64-unknown-freebsd"
    "freebsd arm64 aarch64-unknown-freebsd"
    "freebsd 386 i686-unknown-freebsd"
)

usage() {
    cat <<'EOF'
Build the Komari Agent release matrix.

Usage:
  ./build_all.sh [--list] [--help]

Environment:
  KOMARI_AGENT_VERSION        Version string (default: latest git tag or "dev")
  KOMARI_AGENT_VERSION_HASH   Revision string (default: short git SHA)
  KOMARI_BUILD_TARGETS        Comma-separated filter, e.g. linux-amd64,darwin-arm64
  KOMARI_BUILD_TOOL           auto (default), cargo, zigbuild or cross
  KOMARI_INSTALL_TARGETS      1 (default) to run rustup target add when missing
  KOMARI_BUILD_DRY_RUN        1 to print the build commands without running them

Output: ./build/komari-agent-{goos}-{goarch}[.exe]
EOF
}

list_targets() {
    local entry goos goarch target
    for entry in "${TARGETS[@]}"; do
        read -r goos goarch target <<<"$entry"
        printf '%-8s %-6s %s\n' "$goos" "$goarch" "$target"
    done
}

for arg in "$@"; do
    case "$arg" in
        -h | --help)
            usage
            exit 0
            ;;
        --list)
            list_targets
            exit 0
            ;;
        *)
            printf 'Unknown argument: %s\n' "$arg" >&2
            usage >&2
            exit 2
            ;;
    esac
done

command -v cargo >/dev/null 2>&1 || {
    printf 'cargo not found in PATH\n' >&2
    exit 1
}

case "$BUILD_TOOL" in
    auto | cargo | zigbuild | cross) ;;
    *)
        printf 'Unknown KOMARI_BUILD_TOOL: %s (expected auto, cargo, zigbuild or cross)\n' "$BUILD_TOOL" >&2
        exit 2
        ;;
esac

VERSION="${KOMARI_AGENT_VERSION:-}"
if [ -z "$VERSION" ]; then
    VERSION=$(git describe --tags --abbrev=0 2>/dev/null || printf 'dev')
fi

HASH="${KOMARI_AGENT_VERSION_HASH:-}"
if [ -z "$HASH" ]; then
    HASH=$(git rev-parse --short HEAD 2>/dev/null || true)
fi

export KOMARI_AGENT_VERSION="$VERSION"
export KOMARI_AGENT_VERSION_HASH="$HASH"

HOST_TARGET=$(rustc -vV | sed -n 's/^host: //p')

printf 'Version: %s' "$VERSION"
if [ -n "$HASH" ]; then
    printf ' (%s)' "$HASH"
fi
printf '\nHost target: %s\n\n' "$HOST_TARGET"

if [ "$DRY_RUN" != "1" ]; then
    mkdir -p "$BUILD_DIR" || exit 1
fi

FAILED_BUILDS=()
BUILT=0

target_selected() {
    local wanted="$1"
    [ -z "$ONLY" ] && return 0
    case ",$ONLY," in
        *",$wanted,"*) return 0 ;;
        *) return 1 ;;
    esac
}

tool_available() {
    command -v "$1" >/dev/null 2>&1
}

ensure_target() {
    local target="$1"
    if ! tool_available rustup; then
        return 0
    fi
    if rustup target list --installed 2>/dev/null | grep -qx "$target"; then
        return 0
    fi
    if [ "$INSTALL_TARGETS" != "1" ]; then
        printf '  target %s is not installed (run: rustup target add %s)\n' "$target" "$target" >&2
        return 1
    fi
    printf '  installing rust target %s...\n' "$target"
    rustup target add "$target"
}

choose_tool() {
    local target="$1"
    case "$BUILD_TOOL" in
        cargo | zigbuild | cross)
            printf '%s' "$BUILD_TOOL"
            return 0
            ;;
        auto) ;;
        *)
            printf 'Unknown KOMARI_BUILD_TOOL: %s\n' "$BUILD_TOOL" >&2
            return 1
            ;;
    esac

    if [ "$target" = "$HOST_TARGET" ]; then
        printf 'cargo'
    elif tool_available cargo-zigbuild || cargo zigbuild --version >/dev/null 2>&1; then
        printf 'zigbuild'
    elif tool_available cross; then
        printf 'cross'
    else
        printf 'cargo'
    fi
}

build_one() {
    local goos="$1" goarch="$2" target="$3"
    local asset="komari-agent-${goos}-${goarch}"
    if [ "$goos" = "windows" ]; then
        asset="${asset}.exe"
    fi

    local tool
    tool=$(choose_tool "$target") || return 1

    printf 'Building %s/%s (%s) with %s...\n' "$goos" "$goarch" "$target" "$tool"

    if [ "$DRY_RUN" = "1" ]; then
        printf '  [dry-run] KOMARI_AGENT_VERSION=%s KOMARI_AGENT_VERSION_HASH=%s\n' "$VERSION" "$HASH"
        printf '  [dry-run] %s build --release --locked --target %s -p agent-cli\n' "$tool" "$target"
        printf '  [dry-run] would produce %s/%s\n' "$BUILD_DIR" "$asset"
        return 0
    fi

    ensure_target "$target" || return 1

    # option_env! values are not tracked by Cargo; force agent-cli to rebuild.
    cargo clean -p agent-cli --release --target "$target" >/dev/null 2>&1 || true

    local status=0
    case "$tool" in
        cargo)
            cargo build --release --locked --target "$target" -p agent-cli || status=$?
            ;;
        zigbuild)
            cargo zigbuild --release --locked --target "$target" -p agent-cli || status=$?
            ;;
        cross)
            cross build --release --locked --target "$target" -p agent-cli || status=$?
            ;;
    esac

    if [ "$status" -ne 0 ]; then
        printf 'Failed to build for %s/%s\n' "$goos" "$goarch" >&2
        printf '  hint: install cargo-zigbuild + ziglang (KOMARI_BUILD_TOOL=zigbuild),\n' >&2
        printf '        or provide CC_%s and a linker for this target.\n' "${target//-/_}" >&2
        return 1
    fi

    local binary="target/${target}/release/komari-agent"
    if [ "$goos" = "windows" ]; then
        binary="${binary}.exe"
    fi

    if [ ! -f "$binary" ]; then
        printf 'Build succeeded but %s is missing\n' "$binary" >&2
        return 1
    fi

    cp -f "$binary" "${BUILD_DIR}/${asset}"
    chmod 0755 "${BUILD_DIR}/${asset}" 2>/dev/null || true
    printf 'Successfully built %s\n' "${BUILD_DIR}/${asset}"
    return 0
}

for entry in "${TARGETS[@]}"; do
    read -r goos goarch target <<<"$entry"
    if ! target_selected "${goos}-${goarch}"; then
        continue
    fi

    if build_one "$goos" "$goarch" "$target"; then
        BUILT=$((BUILT + 1))
    else
        FAILED_BUILDS+=("${goos}/${goarch}")
    fi
done

printf '\n'
if [ "${#FAILED_BUILDS[@]}" -ne 0 ]; then
    printf 'The following builds failed:\n' >&2
    local_fail=
    for local_fail in "${FAILED_BUILDS[@]}"; do
        printf '  - %s\n' "$local_fail" >&2
    done
    printf '\n%d build(s) succeeded, %d failed. Binaries are in ./%s.\n' \
        "$BUILT" "${#FAILED_BUILDS[@]}" "$BUILD_DIR" >&2
    exit 1
fi

if [ "$BUILT" -eq 0 ] && [ "${#FAILED_BUILDS[@]}" -eq 0 ]; then
    printf 'No build targets matched KOMARI_BUILD_TARGETS=%s\n' "$ONLY" >&2
    exit 2
fi

printf 'All %d builds completed successfully. Binaries are in ./%s.\n' "$BUILT" "$BUILD_DIR"
