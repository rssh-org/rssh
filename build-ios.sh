#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "Error: iOS builds require macOS."
    exit 1
fi

for cmd in rustup npm npx xcodebuild xcrun pod xcodegen perl; do
    command -v "$cmd" >/dev/null || { echo "Missing: $cmd"; exit 1; }
done

if [ -z "${APPLE_DEVELOPMENT_TEAM:-}" ]; then
    echo "Error: APPLE_DEVELOPMENT_TEAM is not set."
    echo "Use the Team ID shown under Xcode > Settings > Accounts."
    exit 1
fi
if [[ ! "$APPLE_DEVELOPMENT_TEAM" =~ ^[A-Za-z0-9]{10}$ ]]; then
    echo "Error: APPLE_DEVELOPMENT_TEAM must be a 10-character Team ID."
    exit 1
fi

export_method="${IOS_EXPORT_METHOD:-debugging}"
case "$export_method" in
    debugging)
        bundle_identifier="${IOS_BUNDLE_IDENTIFIER:-com.rssh.app.dev.${APPLE_DEVELOPMENT_TEAM}}"
        if [[ ! "$bundle_identifier" =~ ^[A-Za-z0-9.-]+$ ]]; then
            echo "Error: IOS_BUNDLE_IDENTIFIER contains invalid characters."
            exit 1
        fi
        unset IOS_CERTIFICATE IOS_CERTIFICATE_PASSWORD IOS_MOBILE_PROVISION
        echo "Using Xcode automatic signing for development."
        echo "The Apple ID must already be signed in under Xcode > Settings > Accounts."
        echo "A new device must first be selected and run once from Xcode."
        echo "Bundle identifier: $bundle_identifier"
        ;;
    app-store-connect|release-testing)
        required_signing_vars=(
            IOS_CERTIFICATE
            IOS_CERTIFICATE_PASSWORD
            IOS_MOBILE_PROVISION
        )
        for var in "${required_signing_vars[@]}"; do
            if [ -z "${!var:-}" ]; then
                echo "Error: $var is required for $export_method signing."
                exit 1
            fi
        done
        ;;
    *)
        echo "Error: unsupported IOS_EXPORT_METHOD '$export_method'."
        exit 1
        ;;
esac

if ! xcrun --sdk iphoneos --show-sdk-path >/dev/null 2>&1; then
    echo "Error: a full Xcode installation with the iOS SDK is required."
    exit 1
fi

rustup target add aarch64-apple-ios

echo "=== 1. Install frontend dependencies ==="
npm ci

if [ ! -f src-tauri/gen/apple/project.yml ]; then
    echo "=== 2. Initialize the Tauri iOS project ==="
    npx tauri ios init --ci --skip-targets-install
else
    echo "=== 2. Tauri iOS project already initialized ==="
fi

project_yml="src-tauri/gen/apple/project.yml"
if grep -q 'script: npm run -- tauri ios xcode-script' "$project_yml"; then
    perl -0pi -e 's{script: npm run -- tauri ios xcode-script}{script: cd "\$\{SRCROOT\}/../../.." \&\& npm run -- tauri ios xcode-script}' "$project_yml"
fi

cp src-tauri/PrivacyInfo.xcprivacy src-tauri/gen/apple/rssh_iOS/PrivacyInfo.xcprivacy
xcodegen generate --spec "$project_yml" --project src-tauri/gen/apple

echo "=== 3. Sync iOS app icons ==="
app_icon_dir="src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset"
cp src-tauri/icons/ios/*.png "$app_icon_dir"/

echo "=== 4. Build iOS IPA ($export_method) ==="
if [ "$export_method" = "debugging" ]; then
    npx tauri ios build --ci --export-method debugging \
        --config "{\"identifier\":\"$bundle_identifier\"}" "$@"
else
    npx tauri ios build --ci --export-method "$export_method" "$@"
fi

IPA_DIR="src-tauri/gen/apple/build/arm64"
if [ -d "$IPA_DIR" ]; then
    echo "=== IPAs ==="
    find "$IPA_DIR" -maxdepth 1 -name "*.ipa" -exec ls -lh {} \;
fi
