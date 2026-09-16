#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
ROOT="$PWD"
TARGET="aarch64-unknown-linux-ohos"
OHOS_PROJECT="$ROOT/src-tauri/gen/ohos"
WORKSPACE="$ROOT/src-tauri/target/ohos-workspace"
STAGING="$ROOT/src-tauri/target/ohos-staging"
FRONTEND="$ROOT/src-tauri/target/ohos-frontend"
HAP_DIR="$OHOS_PROJECT/entry/build/default/outputs/default"

for cmd in git rustup cargo npm node ohrs; do
    command -v "$cmd" >/dev/null || { echo "Missing: $cmd" >&2; exit 1; }
done
if [ "$(ohrs --version)" != "Version: 1.5.0" ]; then
    echo "Install the pinned build helper: cargo install ohrs --version 1.5.0 --locked" >&2
    exit 1
fi
if ! rustup target list --installed | grep -qx "$TARGET"; then
    echo "Missing Rust target: rustup target add $TARGET" >&2
    exit 1
fi

# Full SDK root, containing native/. No fork of the global Tauri CLI is needed.
if [ -z "${OHOS_HOME:-}" ]; then
    for candidate in \
        "/Applications/DevEco-Studio.app/Contents/sdk/"*/openharmony \
        "/Applications/DevEco Studio.app/Contents/sdk/"*/openharmony \
        "$HOME/ohos/sdk/openharmony" \
        "/opt/openharmony/sdk"; do
        if [ -d "$candidate/native" ]; then export OHOS_HOME="$candidate"; break; fi
    done
fi
if [ -z "${OHOS_HOME:-}" ] || [ ! -d "$OHOS_HOME/native" ]; then
    echo "Set OHOS_HOME to the full SDK root containing native/." >&2
    exit 1
fi
export OHOS_NDK_HOME="$OHOS_HOME"
NDK="$OHOS_HOME/native"
for tool in clang clang++ llvm-ar; do
    [ -x "$NDK/llvm/bin/$tool" ] || { echo "Missing NDK tool: $tool" >&2; exit 1; }
done

if [ -z "${DEVECO_SDK_HOME:-}" ]; then
    case "$OHOS_HOME" in
        */sdk/default/openharmony) export DEVECO_SDK_HOME="${OHOS_HOME%/sdk/default/openharmony}/sdk" ;;
        */openharmony) export DEVECO_SDK_HOME="${OHOS_HOME%/openharmony}" ;;
        *) export DEVECO_SDK_HOME="$OHOS_HOME" ;;
    esac
fi
DEVECO_CONTENTS="${DEVECO_SDK_HOME%/sdk*}"
HVIGOR_BIN="${HVIGOR_BIN:-$DEVECO_CONTENTS/tools/hvigor/bin/hvigorw.js}"
OHPM_BIN="${OHPM_BIN:-$DEVECO_CONTENTS/tools/ohpm/bin/ohpm}"
[ -f "$HVIGOR_BIN" ] || { echo "Set HVIGOR_BIN to DevEco's hvigorw.js." >&2; exit 1; }
[ -x "$OHPM_BIN" ] || { echo "Set OHPM_BIN to DevEco's ohpm executable." >&2; exit 1; }

# cc-rs does not infer the OHOS sysroot for bundled SQLite/tree-sitter.
export CC_aarch64_unknown_linux_ohos="$NDK/llvm/bin/clang"
export CXX_aarch64_unknown_linux_ohos="$NDK/llvm/bin/clang++"
export AR_aarch64_unknown_linux_ohos="$NDK/llvm/bin/llvm-ar"
# cc-rs otherwise splits FLAGS on whitespace and passes quote characters to
# clang literally. Enable shell parsing so SDK paths containing spaces work.
export CC_SHELL_ESCAPED_FLAGS=1
export CFLAGS_aarch64_unknown_linux_ohos="--target=aarch64-linux-ohos --sysroot=\"$NDK/sysroot\""
export CXXFLAGS_aarch64_unknown_linux_ohos="$CFLAGS_aarch64_unknown_linux_ohos"
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_TARGET_DIR="$ROOT/src-tauri/target/ohos"

# Discard stale package/staging outputs before doing any build. Every command
# below must succeed; a Rust/frontend failure must never package an older .so.
rm -rf "$STAGING" "$FRONTEND" "$HAP_DIR"
mkdir -p "$STAGING"
echo "Building OHOS frontend"
npm run build -- --outDir "$FRONTEND"
node scripts/prepare-ohos-sources.mjs
node scripts/prepare-ohos.mjs
echo "Building OHOS Rust library (isolated Cargo workspace)"
(
    cd "$WORKSPACE"
    # ohrs performs cargo metadata internally; check the lock before it can
    # resolve anything, then verify that it left the checked-in lock unchanged.
    cargo metadata --locked --format-version 1 --filter-platform "$TARGET" >/dev/null
    ohrs build --arch arm64 --release --dist "$STAGING" --target-dir "$CARGO_TARGET_DIR" -- \
        --locked --lib --features custom-protocol
    cmp Cargo.lock "$ROOT/src-tauri/ohos/Cargo.lock"
)
[ -s "$STAGING/arm64-v8a/librssh_lib.so" ] || { echo "Build produced no librssh_lib.so." >&2; exit 1; }
[ -s "$FRONTEND/index.html" ] || { echo "Build produced no frontend index.html." >&2; exit 1; }

# ohrs also stages required C++ runtime libraries. Replace the entire ABI set.
rm -rf "$OHOS_PROJECT/entry/libs/arm64-v8a" "$OHOS_PROJECT/entry/src/main/resources/rawfile"
mkdir -p "$OHOS_PROJECT/entry/libs" "$OHOS_PROJECT/entry/src/main/resources/rawfile"
cp -R "$STAGING/arm64-v8a" "$OHOS_PROJECT/entry/libs/"
cp -R "$FRONTEND/." "$OHOS_PROJECT/entry/src/main/resources/rawfile/"
echo "Assembling OHOS HAP"
(
    cd "$OHOS_PROJECT"
    "$OHPM_BIN" install
    node "$HVIGOR_BIN" assembleHap --mode module -p product=default --no-daemon
)

HAP="$HAP_DIR/entry-default-signed.hap"
[ -s "$HAP" ] || { echo "No signed HAP produced; configure signing in DevEco Studio." >&2; exit 1; }
echo "Built: $HAP"
ls -lh "$HAP"
