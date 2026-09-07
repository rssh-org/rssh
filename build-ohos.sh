#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

# ── Cross-compile the RSSH Rust core for HarmonyOS (aarch64-unknown-linux-ohos)
# and assemble the HAP via the feat/open-harmony Tauri fork.
#
# One-time setup this script does NOT do for you:
#   1. DevEco Studio installed (brings SDK + hvigor + ohpm + hdc + node).
#   2. The fork's tauri CLI (the crates.io one has no `ohos` subcommand):
#        cargo install --git https://github.com/yangyongzhen/tauri \
#          --branch feat/open-harmony tauri-cli
#   3. A signing profile: open gen/ohos in DevEco Studio →
#      Project Structure → Signing Configs → Automatically generate
#      (requires a Huawei account login). Debug sideload works without it.
# ──

for cmd in rustup npm; do
    command -v "$cmd" >/dev/null || { echo "Missing: $cmd"; exit 1; }
done

if ! cargo tauri ohos --help >/dev/null 2>&1; then
    echo "Error: 'cargo tauri ohos' not available."
    echo "Install the fork CLI (see header of this script), then retry."
    exit 1
fi
command -v ohrs >/dev/null || { echo "Missing: ohrs (cargo install ohrs)"; exit 1; }

# OHOS_HOME must point at the FULL SDK (the dir containing native/ + ets/),
# not the bare NDK — DevEco's hvigor needs the complete component set.
if [ -z "${OHOS_HOME:-}" ]; then
    for candidate in \
        "/Applications/DevEco-Studio.app/Contents/sdk/"*/openharmony \
        "/Applications/DevEco Studio.app/Contents/sdk/"*/openharmony \
        "$HOME/ohos/sdk/openharmony" \
        "/opt/openharmony/sdk"; do
        if [ -d "$candidate" ]; then
            export OHOS_HOME="$candidate"
            break
        fi
    done
fi
if [ -z "${OHOS_HOME:-}" ] || [ ! -d "${OHOS_HOME:-}/native" ]; then
    echo "Error: OHOS_HOME not set and no SDK found in default locations."
    echo "Set OHOS_HOME to the full OpenHarmony SDK root (contains native/)."
    exit 1
fi
NDK="$OHOS_HOME/native"
echo "SDK: $OHOS_HOME"

CLANG="$NDK/llvm/bin/clang"
AR="$NDK/llvm/bin/llvm-ar"
for tool in "$CLANG" "$AR"; do
    [ -x "$tool" ] || { echo "Error: NDK tool not found: $tool"; exit 1; }
done

export OHOS_NDK_HOME="$OHOS_HOME"

# Linker/config for the Rust ohos target (env vars beat .cargo/config.toml,
# so no machine-specific paths get committed).
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="$CLANG"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_AR="$AR"
# C toolchain for cc-crate builds (rusqlite bundled sqlite, tree-sitter):
# explicit --target + --sysroot — cc cannot infer the OHOS sysroot.
CFLAGS_OHOS="--target=aarch64-linux-ohos --sysroot=$NDK/sysroot"
export CC_aarch64_unknown_linux_ohos="$CLANG"
export CXX_aarch64_unknown_linux_ohos="$NDK/llvm/bin/clang++"
export AR_aarch64_unknown_linux_ohos="$AR"
export CFLAGS_aarch64_unknown_linux_ohos="$CFLAGS_OHOS"
export CXXFLAGS_aarch64_unknown_linux_ohos="$CFLAGS_OHOS"

# The [patch.crates-io] fork revs are git dependencies.
export CARGO_NET_GIT_FETCH_WITH_CLI=true

rustup target add aarch64-unknown-linux-ohos

echo "=== Build Tauri OHOS (aarch64) ==="
# custom-protocol is passed explicitly: without it the artifact embeds the
# devUrl and ships a white screen (see the fork porting notes).
# The trailing `|| true`: on Windows the final HAP-assembly step of this
# command fails on a .bat spawn even though the .so is built — we re-assemble
# below ourselves anyway, so the initial failure is not fatal.
cargo tauri ohos build -t aarch64 --features custom-protocol || \
    echo "(initial HAP assembly failed — .so should still be staged; continuing)"

# ── Post-build fixes (the fork's ohos pipeline has two gaps) ──
OHOS_PROJECT="src-tauri/gen/ohos"

# 1. The frontend never lands in the DevEco project on its own — sync it.
RAWFILE="$OHOS_PROJECT/entry/src/main/resources/rawfile"
rm -rf "$RAWFILE"
mkdir -p "$RAWFILE"
cp -r dist/. "$RAWFILE"/

# 2. The generated entry/hvigorfile.ts shells out to a cargo-mobile2 daemon
# (`dev-eco-studio-script`) that only exists inside a `cargo tauri ohos build`
# invocation. Strip it so plain hvigor runs work; the .so it used to build is
# already staged in entry/libs/arm64-v8a/ by the step above.
HVIGORFILE="$OHOS_PROJECT/entry/hvigorfile.ts"
if grep -q "tauriPlugin" "$HVIGORFILE"; then
    cat > "$HVIGORFILE" <<'EOF'
import { hapTasks } from '@ohos/hvigor-ohos-plugin';

export default {
  system: hapTasks,
  /* cargo hook removed: librssh_lib.so is cross-compiled by build-ohos.sh
     (cargo tauri ohos build) and already staged in entry/libs/arm64-v8a/.
     The hook's dev-eco-studio-script needs the cargo-mobile2 daemon and is
     the documented Windows blocker anyway. */
  plugins: []
}
EOF
fi

# 3. Re-assemble so the HAP actually contains the frontend from step 1.
if [ -z "${DEVECO_SDK_HOME:-}" ]; then
    case "$OHOS_HOME" in
        */openharmony) export DEVECO_SDK_HOME="${OHOS_HOME%/openharmony}" ;;
        *) export DEVECO_SDK_HOME="$OHOS_HOME" ;;
    esac
fi
HVIGOR_BIN=""
for candidate in \
    "/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin/hvigorw.js" \
    "${DEVECO_SDK_HOME%/sdk*}/tools/hvigor/bin/hvigorw.js"; do
    if [ -f "$candidate" ]; then HVIGOR_BIN="$candidate"; break; fi
done
if [ -n "$HVIGOR_BIN" ]; then
    echo "=== Re-assemble HAP with frontend ==="
    (cd "$OHOS_PROJECT" && node "$HVIGOR_BIN" assembleHap --mode module -p product=default --no-daemon)
else
    echo "WARNING: hvigorw.js not found — HAP lacks the frontend until you re-run"
    echo "hvigor assembleHap from DevEco Studio (or install command-line tools)."
fi

echo "=== Done ==="
SO="src-tauri/target/aarch64-unknown-linux-ohos/release/librssh_lib.so"
[ -f "$SO" ] && ls -lh "$SO"

HAP_DIR="src-tauri/gen/ohos/entry/build/default/outputs/default"
if ls "$HAP_DIR"/*.hap >/dev/null 2>&1; then
    echo ""
    echo "HAP:"
    ls -lh "$HAP_DIR"/*.hap
    echo ""
    echo "Deploy to a developer-mode device:"
    echo "  hdc install -r $HAP_DIR/entry-default-signed.hap"
    echo "  hdc shell \"aa start -a EntryAbility -b com.rssh.app\""
else
    echo ""
    echo "No HAP assembled. If the CLI stopped after the .so (known on some"
    echo "hosts), finish manually:"
    echo "  cd src-tauri/gen/ohos && ohpm install && cd entry && ohpm install && cd .."
    echo "  node \"<DevEco>/tools/hvigor/bin/hvigorw.js\" assembleHap --mode module -p product=default --no-daemon"
fi
