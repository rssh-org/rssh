#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
ROOT="$PWD"
TARGET="aarch64-unknown-linux-ohos"
OHOS_PROJECT="$ROOT/src-tauri/gen/ohos"
WORKSPACE="$ROOT/src-tauri/target/ohos-workspace"
STAGING="$ROOT/src-tauri/target/ohos-staging"
FRONTEND="$ROOT/src-tauri/target/ohos-frontend"
RUNTIME="$OHOS_PROJECT/common/runtime"
PHONE_HAP_DIR="$OHOS_PROJECT/products/phone/build/default/outputs/default"
DESKTOP_HAP_DIR="$OHOS_PROJECT/products/desktop/build/default/outputs/default"
APP_DIR="$OHOS_PROJECT/build/outputs/default"
SIGN_PACKAGES=true
PACKAGE_SUFFIX=signed
APP_NAME=RSSH.app

case "${1:-}" in
    "") ;;
    --unsigned)
        SIGN_PACKAGES=false
        PACKAGE_SUFFIX=unsigned
        APP_NAME=RSSH-unsigned.app
        shift
        ;;
    --help|-h)
        echo "Usage: $0 [--unsigned]"
        echo "Build separate phone/tablet and desktop HAPs plus one AppGallery .app."
        echo "Signing is required by default; --unsigned produces emulator acceptance artifacts."
        exit 0
        ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
esac
[ "$#" -eq 0 ] || { echo "Usage: $0 [--unsigned]" >&2; exit 1; }

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
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/src-tauri/target/ohos}"

# Discard stale package/staging outputs before doing any build. Every command
# below must succeed; a Rust/frontend failure must never package an older .so.
rm -rf "$STAGING" "$FRONTEND" "$PHONE_HAP_DIR" "$DESKTOP_HAP_DIR" "$APP_DIR" \
    "$OHOS_PROJECT/entry/build/default/outputs/default"
mkdir -p "$STAGING"
echo "Building OHOS frontend"
npm run build -- --outDir "$FRONTEND"
node scripts/prepare-ohos-sources.mjs
node scripts/prepare-ohos.mjs
echo "Building OHOS Rust library (isolated Cargo workspace)"
(
    cd "$WORKSPACE"
    # ohrs performs cargo metadata internally; check the lock before it can
    # resolve anything, then verify that it left the normalized lock unchanged.
    # Only this application's release version differs from the checked-in lock.
    cargo metadata --locked --format-version 1 --filter-platform "$TARGET" >/dev/null
    ohrs build --arch arm64 --release --dist "$STAGING" --target-dir "$CARGO_TARGET_DIR" -- \
        --locked --lib --features custom-protocol
    cmp Cargo.lock Cargo.lock.expected
)
[ -s "$STAGING/arm64-v8a/librssh_lib.so" ] || { echo "Build produced no librssh_lib.so." >&2; exit 1; }
[ -s "$FRONTEND/index.html" ] || { echo "Build produced no frontend index.html." >&2; exit 1; }

# ohrs also stages required C++ runtime libraries. Replace the entire ABI set.
rm -rf "$RUNTIME/libs/arm64-v8a" "$RUNTIME/src/main/resources/rawfile"
mkdir -p "$RUNTIME/libs" "$RUNTIME/src/main/resources/rawfile"
cp -R "$STAGING/arm64-v8a" "$RUNTIME/libs/"
cp -R "$FRONTEND/." "$RUNTIME/src/main/resources/rawfile/"
# HAP metadata follows Tauri's version without dirtying the source manifest.
# An explicit OHOS_VERSION_CODE overrides the configured monotonic build code.
APP_CONFIG="$OHOS_PROJECT/AppScope/app.json5"
APP_CONFIG_BACKUP="$(mktemp "$WORKSPACE/app.json5.XXXXXX")"
cp "$APP_CONFIG" "$APP_CONFIG_BACKUP"
restore_app_config() {
    cp "$APP_CONFIG_BACKUP" "$APP_CONFIG"
    rm -f "$APP_CONFIG_BACKUP"
}
trap restore_app_config EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
cp "$WORKSPACE/app.json5" "$APP_CONFIG"
echo "Assembling OHOS phone/tablet and desktop HAPs and App Pack"
(
    cd "$OHOS_PROJECT"
    "$OHPM_BIN" install
    node "$HVIGOR_BIN" assembleApp --mode project -p product=default -p buildMode=release \
        -p "enableSignTask=$SIGN_PACKAGES" --no-daemon
)

PHONE_HAP="$PHONE_HAP_DIR/entry-default-$PACKAGE_SUFFIX.hap"
DESKTOP_HAP="$DESKTOP_HAP_DIR/desktop-default-$PACKAGE_SUFFIX.hap"
APP="$APP_DIR/$APP_NAME"
for package in "$PHONE_HAP" "$DESKTOP_HAP" "$APP"; do
    if [ ! -s "$package" ]; then
        echo "Missing $PACKAGE_SUFFIX package: $package" >&2
        if [ "$SIGN_PACKAGES" = true ]; then
            echo "Configure signing in DevEco Studio; unsigned packages are accepted only with --unsigned." >&2
        fi
        exit 1
    fi
done
echo "Built phone/tablet HAP: $PHONE_HAP"
echo "Built desktop HAP: $DESKTOP_HAP"
echo "Built App Pack: $APP"
ls -lh "$PHONE_HAP" "$DESKTOP_HAP" "$APP"
