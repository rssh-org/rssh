#!/usr/bin/env bash
set -euo pipefail

fail() { echo "OHOS toolchain: $*" >&2; exit 1; }

[[ "$(uname -s)/$(uname -m)" == "Linux/x86_64" ]] || fail "requires a Linux x64 runner."
: "${RUNNER_TEMP:?Run this script on a GitHub-hosted runner.}"
: "${GITHUB_ENV:?Missing GitHub environment file.}"
: "${GITHUB_PATH:?Missing GitHub path file.}"
for command in python3 curl sha256sum unzip java; do
    command -v "$command" >/dev/null || fail "missing $command."
done

manifest="$(cd "$(dirname "$0")/.." && pwd)/ci-toolchain.json"
metadata="$(python3 - "$manifest" <<'PY'
import json, sys
with open(sys.argv[1]) as source:
    config = json.load(source)
for key in ('version', 'sha256', 'nodeVersion', 'sdkVersion', 'apiVersion', 'hvigorVersion', 'ohpmVersion'):
    value = config[key]
    if not isinstance(value, str) or '\n' in value or '\r' in value:
        raise SystemExit(f'Invalid toolchain manifest field: {key}')
    print(value)
PY
)"
config=()
while IFS= read -r value; do config+=("$value"); done <<< "$metadata"
version="${config[0]}"
sha256="${config[1]}"
toolchain_dir="$RUNNER_TEMP/rssh-ohos-toolchain"
toolchain="$toolchain_dir/command-line-tools"

if [[ ! -f "$toolchain_dir/.archive-sha256" ]] || [[ "$(cat "$toolchain_dir/.archive-sha256")" != "$sha256" ]]; then
    : "${OHOS_COMMAND_LINE_TOOLS_URL:?Configure the official download URL in the harmonyos-release environment.}"
    python3 - <<'PY'
import os
from urllib.parse import urlsplit
url = os.environ['OHOS_COMMAND_LINE_TOOLS_URL']
parsed = urlsplit(url)
if '\n' in url or '\r' in url or parsed.scheme != 'https' or parsed.hostname != 'contentcenter-vali-drcn.dbankcdn.cn':
    raise SystemExit('OHOS toolchain: expected an HTTPS download URL from the official Huawei CDN.')
PY
    staging="$(mktemp -d "$RUNNER_TEMP/rssh-ohos-toolchain.XXXXXX")"
    trap 'rm -rf "$staging"' EXIT
    archive="$staging/toolchain.zip"
    echo "Downloading HarmonyOS Command Line Tools $version."
    # Huawei's CDN rejects HEAD requests. Use GET and never print its signed URL.
    curl --fail --silent --show-error --location --retry 3 \
        --connect-timeout 30 --max-time 1800 --proto '=https' --proto-redir '=https' \
        --user-agent 'Mozilla/5.0' --referer 'https://developer.huawei.com/' \
        --output "$archive" "$OHOS_COMMAND_LINE_TOOLS_URL"
    printf '%s  %s\n' "$sha256" "$archive" | sha256sum -c >/dev/null \
        || fail "archive SHA-256 does not match the pinned official checksum."
    unzip -q "$archive" -d "$staging"
    rm "$archive"
    [[ -d "$staging/command-line-tools" ]] || fail "archive is missing command-line-tools/."
    printf '%s\n' "$sha256" > "$staging/.archive-sha256"
    rm -rf "$toolchain_dir"
    mv "$staging" "$toolchain_dir"
    trap - EXIT
fi

export OHOS_HOME="$toolchain/sdk/default/openharmony"
export OHOS_NDK_HOME="$OHOS_HOME"
export DEVECO_SDK_HOME="$toolchain/sdk"
export DEVECO_NODE_HOME="$toolchain/tool/node"
export NODE_HOME="$DEVECO_NODE_HOME"
export HVIGOR_BIN="$toolchain/hvigor/bin/hvigorw.js"
export OHPM_BIN="$toolchain/bin/ohpm"
export PATH="$DEVECO_NODE_HOME/bin:$toolchain/bin:$OHOS_HOME/toolchains:$PATH"

for file in "$HVIGOR_BIN" "$OHOS_HOME/toolchains/lib/hap-sign-tool.jar" "$OHOS_HOME/toolchains/lib/app_packing_tool.jar"; do
    [[ -s "$file" ]] || fail "missing $file."
done
for file in "$DEVECO_NODE_HOME/bin/node" "$OHPM_BIN" "$OHOS_HOME/native/llvm/bin/clang"; do
    [[ -x "$file" ]] || fail "missing executable $file."
done
[[ "$(node --version)" == "v${config[2]}" ]] || fail "bundled Node version does not match the manifest."
python3 - "$toolchain" "$manifest" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
config = json.loads(pathlib.Path(sys.argv[2]).read_text())
sdk = json.loads((root / 'sdk/default/sdk-pkg.json').read_text())['data']
if sdk['version'] != config['sdkVersion'] or str(sdk['apiVersion']) != config['apiVersion']:
    raise SystemExit('OHOS toolchain: SDK version does not match the manifest.')
for package, expected in [('hvigor/hvigor', 'hvigorVersion'), ('ohpm', 'ohpmVersion')]:
    actual = json.loads((root / package / 'package.json').read_text())['version']
    if actual != config[expected]:
        raise SystemExit(f'OHOS toolchain: {package} version does not match the manifest.')
PY

for name in OHOS_HOME OHOS_NDK_HOME DEVECO_SDK_HOME DEVECO_NODE_HOME NODE_HOME HVIGOR_BIN OHPM_BIN; do
    printf '%s=%s\n' "$name" "${!name}" >> "$GITHUB_ENV"
done
printf '%s\n' "$DEVECO_NODE_HOME/bin" "$toolchain/bin" "$OHOS_HOME/toolchains" >> "$GITHUB_PATH"
echo "HarmonyOS $version (SDK ${config[3]}, API ${config[4]}) is ready."
