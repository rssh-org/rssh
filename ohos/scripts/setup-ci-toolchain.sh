#!/usr/bin/env bash
set -euo pipefail

[[ "$(uname -s)/$(uname -m)" == "Linux/x86_64" ]] || { echo 'OHOS tools require Linux x64.' >&2; exit 1; }
: "${RUNNER_TEMP:?Run this script on a GitHub-hosted runner.}"
: "${GITHUB_ENV:?Missing GitHub environment file.}"
: "${GITHUB_PATH:?Missing GitHub path file.}"
: "${OHOS_COMMAND_LINE_TOOLS_URL:?Configure the official SDK download Secret.}"
[[ "$OHOS_COMMAND_LINE_TOOLS_URL" == https://contentcenter-vali-drcn.dbankcdn.cn/* ]] \
    || { echo 'Expected the official Huawei HTTPS download URL.' >&2; exit 1; }

manifest="$(cd "$(dirname "$0")/.." && pwd)/ci-toolchain.json"
read -r version sha256 <<< "$(python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); print(c["version"], c["sha256"])' "$manifest")"
install_dir="$(mktemp -d "$RUNNER_TEMP/rssh-ohos.XXXXXX")"
trap 'rm -rf "$install_dir"' EXIT
archive="$install_dir/toolchain.zip"
echo "Downloading HarmonyOS Command Line Tools $version."
# Huawei's CDN rejects HEAD requests; do not print its signed URL.
curl --fail --silent --show-error --location --retry 3 \
    --connect-timeout 30 --max-time 1800 --proto '=https' --proto-redir '=https' \
    --user-agent 'Mozilla/5.0' --referer 'https://developer.huawei.com/' \
    --output "$archive" "$OHOS_COMMAND_LINE_TOOLS_URL"
python3 - "$archive" "$sha256" <<'PY'
import hashlib, sys
digest = hashlib.sha256()
with open(sys.argv[1], 'rb') as archive:
    for chunk in iter(lambda: archive.read(1024 * 1024), b''):
        digest.update(chunk)
if digest.hexdigest() != sys.argv[2]:
    raise SystemExit('OHOS archive SHA-256 does not match the pinned official checksum.')
PY
unzip -q "$archive" -d "$install_dir"
rm "$archive"

# The checksum pins the entire bundle, including its SDK, Node, Hvigor and ohpm.
toolchain="$install_dir/command-line-tools"
OHOS_HOME="$toolchain/sdk/default/openharmony"
OHOS_NDK_HOME="$OHOS_HOME"
DEVECO_SDK_HOME="$toolchain/sdk"
DEVECO_NODE_HOME="$toolchain/tool/node"
NODE_HOME="$DEVECO_NODE_HOME"
HVIGOR_BIN="$toolchain/hvigor/bin/hvigorw.js"
OHPM_BIN="$toolchain/bin/ohpm"
for name in OHOS_HOME OHOS_NDK_HOME DEVECO_SDK_HOME DEVECO_NODE_HOME NODE_HOME HVIGOR_BIN OHPM_BIN; do
    printf '%s=%s\n' "$name" "${!name}" >> "$GITHUB_ENV"
done
printf '%s\n' "$DEVECO_NODE_HOME/bin" "$toolchain/bin" "$OHOS_HOME/toolchains" >> "$GITHUB_PATH"
trap - EXIT
echo "HarmonyOS $version is ready."
