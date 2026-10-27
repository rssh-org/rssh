# Contributing

## Data

On macOS, Windows and Linux, the CLI and desktop app share the same database:

```
~/.rssh/rssh.db        # SQLite (profiles, credentials, forwards, settings)
~/.rssh/snippets.json  # command snippets
~/.ssh/known_hosts     # OpenSSH-standard host key store (shared with `ssh`)
```

Android, iOS and HarmonyOS use their application sandbox for data and known
hosts. Use RSSH's export/import or encrypted sync to move configurations between
devices.

## Prerequisites

All platforms need:

- **Node.js** >= 20
- **Rust** stable (install via [rustup](https://rustup.rs))
- **npm** (comes with Node)

### macOS desktop

No extra dependencies. Xcode Command Line Tools must be installed:

```bash
xcode-select --install
```

### Linux (Debian/Ubuntu)

```bash
sudo apt-get update
sudo apt-get install -y \
    libgtk-3-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev libudev-dev
```

### Windows

No extra dependencies. Visual Studio Build Tools with C++ workload must be installed.

### Android

- **JDK** 17 (recommend Eclipse Temurin)
- **Android SDK** with NDK installed
- Rust targets:
  ```bash
  rustup target add aarch64-linux-android armv7-linux-androideabi
  ```

### HarmonyOS phone and PC

Install DevEco Studio with the full HarmonyOS SDK (`native/`, Hvigor and OHPM),
then install the build helper and Rust target:

```bash
cargo install ohrs --version 1.5.0 --locked
rustup target add aarch64-unknown-linux-ohos
```

The build script detects common SDK locations. Otherwise, set `OHOS_HOME` to
the SDK directory containing `native/`, and set `HVIGOR_BIN` / `OHPM_BIN` to
`hvigorw.js` / `ohpm` if needed.

## Dev

```bash
# install frontend deps
npm install

# start dev server (hot-reload frontend + Rust backend)
npm run tauri -- dev
```

## Build

### Desktop (macOS, Windows or Linux)

```bash
npm run tauri -- build
```

The repository's Tauri wrapper uses `desktop/scripts/` to build `rssh-cli`, stages it in the ignored
`app/bin/` directory and includes it in the desktop application's resources.
The GUI can then install the bundled CLI. Ordinary development and Cargo
check/test commands do not require this directory or a prebuilt CLI.
`npm run tauri -- build --no-bundle` also skips CLI staging.
Output lands in `target/release/bundle/`.

The platform scripts also install npm dependencies and perform their platform's
packaging steps:

```bash
./scripts/build-mac.sh
./scripts/build-linux.sh
```

### Cross-compile for a specific target

```bash
rustup target add x86_64-apple-darwin
npm run tauri -- build --target x86_64-apple-darwin
```

### Android

The maintained Android Studio project is in `android/`. The repository wrapper
prepares `app/gen/android/` before invoking Tauri; a fresh checkout does not
need `tauri android init`.

```bash
npm run tauri -- android dev
npm run tauri -- android build --apk
npm run tauri -- android dev --open
# Build both universal and ARM64 APKs:
./scripts/build-android.sh
```

Edit Kotlin, manifests and Gradle configuration in `android/`, not in the generated
project. Open the generated project through the wrapper when using Android Studio.
Use `npm run tauri -- ...` rather than invoking `npx tauri` directly so the source
preparation step is not skipped.

Keep `key.properties` and the release `.jks` keystore in `android/` (both ignored),
or use an absolute path to an external keystore. Generated Gradle builds read these
signing inputs from the maintained project; preparation does not copy them into
`gen/`. SDK paths may be supplied through the ignored `android/local.properties`;
existing local SDK configuration in the generated project is retained.

### iOS

Local iOS builds require macOS, full Xcode with the iOS SDK, CocoaPods and XcodeGen.

For a local device build, add an Apple ID under **Xcode > Settings > Accounts**,
connect and trust the iPhone, and enable Developer Mode. For a new account or
device, initialize the generated project once:

```bash
APPLE_DEVELOPMENT_TEAM=XXXXXXXXXX \
./scripts/build-ios.sh --open
```

In Xcode, select the app target, enable **Automatically manage signing**, choose
the Personal Team and connected iPhone, then Run once. This one-time step is
required because Tauri 2.11 can update provisioning assets but cannot register a
new device from its CLI build.

After the device is registered, build the IPA with the Personal Team ID:

```bash
APPLE_DEVELOPMENT_TEAM=XXXXXXXXXX ./scripts/build-ios.sh
```

The script uses Xcode automatic signing and the `debugging` export method by
default. A free Personal Team profile normally expires after seven days, so the
app must then be rebuilt and reinstalled. The Apple ID and password are read by
Xcode from the local account and must not be passed to the script. Development
builds use `com.rssh.app.dev.<TEAM_ID>` to avoid colliding with the release App
ID. Override it only when necessary:

```bash
APPLE_DEVELOPMENT_TEAM=XXXXXXXXXX \
IOS_BUNDLE_IDENTIFIER=com.example.rssh.dev \
./scripts/build-ios.sh
```

Maintain the iOS Info.plist and privacy manifest in `ios/`.
The script prepares the generated Xcode project, icons and privacy manifest.
IPA output lands in `app/gen/apple/build/arm64/`.

iOS may suspend network sessions when RSSH enters the background; this project
does not claim a background execution mode it cannot guarantee.

### HarmonyOS packaging

Maintain HarmonyOS sources in `ohos/`. The build script prepares the ignored
`app/gen/ohos/` native project automatically and builds the maintained Rust
workspace at `ohos/Cargo.toml` with its committed `ohos/Cargo.lock`.

```bash
./scripts/build-ohos.sh             # Signed packages; configure signing below first
./scripts/build-ohos.sh --unsigned  # Unsigned packages for emulator testing
```

The ARM64 build produces separate phone/tablet and PC HAPs plus one App Pack:

```text
app/gen/ohos/products/phone/build/default/outputs/default/entry-default-signed.hap
app/gen/ohos/products/desktop/build/default/outputs/default/desktop-default-signed.hap
app/gen/ohos/build/outputs/default/RSSH.app
```

With `--unsigned`, the HAP suffix becomes `unsigned` and the App Pack is named
`RSSH-unsigned.app`.

For AppGallery releases, set `OHOS_VERSION_CODE` higher than all previously
submitted versions. The version name follows `app/tauri.conf.json` and must
match the Cargo package version.

#### Signing

Create the local configuration:

```bash
cp ohos/app/signing.example.json ohos/app/signing.local.json
```

Use DevEco Studio's **Project Structure > Signing Configs** to generate signing
materials, then put its `signingConfigs` array in the local file, keeping the
configuration name `default`. If DevEco writes signing settings into
`build-profile.json5`, move them to the local file and restore the public
`app.signingConfigs` array to `[]`.

Keep certificates, the profile and keystore under `ohos/app/signing/`, using
`signing/...` paths in the configuration; external files need absolute paths.
Copy DevEco's encrypted `storePassword` / `keyPassword` values unchanged and
preserve the `material` directory beside the keystore, which Hvigor needs for
decryption.

`signing.local.json` and `signing/` are Git-ignored and copied into the generated
project during preparation. `--unsigned` does not require signing configuration.

#### GitHub Actions upload

Stable `vMAJOR.MINOR.PATCH` tags run **HarmonyOS Upload** after the release tests.
It builds on GitHub's Ubuntu runner, signs both HAPs, archives `RSSH.app`, and
uploads it and registers the package in AppGallery Connect. It **never submits
review or publishes**.
You can also run the workflow manually with a stable version and selected ref.

Configure the GitHub environment **`harmonyos-release`** under
**Settings > Environments**:

| Type | Name | Value |
| --- | --- | --- |
| Secret | `OHOS_SIGNING_BUNDLE_BASE64` | Bundle generated below from release signing inputs for `com.rssh.app` |
| Secret | `AGC_SERVICE_ACCOUNT_JSON` | Complete downloaded Service Account `private.json` |
| Secret | `OHOS_COMMAND_LINE_TOOLS_URL` | Official Linux tool bundle URL matching `ohos/ci-toolchain.json`; already configured for this repository |
| Variable | `AGC_APP_ID` | Numeric APP ID from AppGallery Connect, not the bundle identifier |
| Variable | `AGC_CHINESE_MAINLAND_FLAG` | For developers registered outside mainland China: `1` for mainland distribution, `0` otherwise; omit for mainland accounts |

For the [AGC Service Account](https://developer.huawei.com/consumer/en/doc/app/agc-help-connect-api-obtain-server-auth-0000002271134661),
open **Users and permissions > API key > Connect API > Service Account**.
Create a **developer-level** account with the **App administrator** role and
download its JSON. Project-level accounts are not supported by the upload API.

Prepare release certificates/profile/keystore in `ohos/app/signing/` and use
relative `signing/...` paths in `signing.local.json`. Keep the matching DevEco
`material/` beside the keystore. Pack and configure the Secret without printing it:

```bash
node ohos/scripts/ci-signing.mjs pack /tmp/rssh-signing.base64
gh secret set OHOS_SIGNING_BUNDLE_BASE64 --env harmonyos-release < /tmp/rssh-signing.base64
rm /tmp/rssh-signing.base64
gh secret set AGC_SERVICE_ACCOUNT_JSON --env harmonyos-release < /private/path/private.json
gh variable set AGC_APP_ID --env harmonyos-release --body YOUR_NUMERIC_APP_ID
```

CI pins the SDK archive checksum and removes signing inputs after packaging.
`versionCode` defaults to `major * 1000000 + minor * 1000 + patch`; minor and patch
must be below 1000. If existing AGC versions use higher codes, use the manual
workflow's `version_code` override for each affected release. Success confirms
upload and package registration; check asynchronous package processing in AGC
before submitting review yourself.

## Running outside Tauri (headless server + IDEA plugin)

The frontend can run **outside** the Tauri desktop shell — in a plain browser, or
inside IntelliJ's embedded Chromium (JCEF). This is **additive**; the desktop app
is unchanged.

**It is not an "IPC → WebSocket" rewrite.** The frontend always talks to the
backend through one seam: Tauri's injected `window.__TAURI_INTERNALS__`
(`invoke` / `listen`). There are two implementations of that one contract:

- **Desktop (unchanged):** Tauri injects the global; IPC is in-process via the WebView.
- **Outside Tauri:** `frontend/lib/ipc-shim.ts` detects the global is absent and installs a
  compatible one that routes `invoke`/`listen` over a localhost **WebSocket** to
  `rssh-server`. Inside the real app the shim is a no-op.

So the 90+ `invoke` / 25+ `listen` call sites are untouched and the desktop
transport stays byte-identical. On the Rust side the *same engine* is reused via a
`Host` (`core/src/emitter.rs`): the app adapters supply an event sink and
optional window capabilities. Tauri forwards events through its application handle;
the headless server forwards them through its WebSocket. The core has no Tauri dependency.

### Pieces

```
app/src/server/mod.rs        # headless adapter: HTTP (embedded UI) + ws (IPC) on one port
core/src/emitter.rs      # Host with injected event sink and window capabilities
app/src/server_main.rs   # the `rssh-server` binary entry (--features server; kept out of src/bin/ so the Tauri bundler ignores it)
frontend/lib/ipc-shim.ts            # Tauri-IPC shim over ws (no-op inside Tauri)
idea-plugin/                   # IntelliJ plugin (JCEF tool window)
```

`rssh-server` is self-contained: `npm run build`'s output is embedded into the binary
(`include_dir!`), and a single loopback port serves both the UI (HTTP) and the IPC
(WebSocket, guarded by a per-launch token).

### Run in a browser

```bash
npm run build
node idea-plugin/scripts/dev-browser.mjs   # builds + runs rssh-server, prints a localhost URL to open
```

### IDEA plugin

```bash
npm run build
cargo build --release -p rssh-app --features server --bin rssh-server
export RSSH_SERVER_BIN="$PWD/target/release/rssh-server"
# open idea-plugin/ in IDEA → run the `runIde` Gradle task → open the "RSSH" tool window
```

Package: `cd idea-plugin && ./gradlew buildPlugin` → a zip under `build/distributions/`.
Ship it on a GitHub release; install via Settings → Plugins → ⚙ → Install Plugin from
Disk. Details in `idea-plugin/README.md`.

## App icons

Maintain source artwork and platform icon sets in `icons/`. To regenerate the
standard desktop, Android and iOS sets from the canonical source image:

```bash
npm run tauri -- icon icons/icon-source.png --output icons
```

Desktop builds reference this directory directly. Android preparation copies the
existing launcher resource set from `icons/android/`; iOS packaging copies
`icons/ios/` into the generated Xcode asset catalog. Do not maintain duplicate icon
files in `android/app/src/main/res/` or edit icons under `app/gen/`.
The optional Android adaptive-icon XML is not staged, preserving the app's current
launcher appearance. HarmonyOS keeps its platform-specific app icon under
`ohos/app/AppScope/resources/base/media/`.

## Project Structure

```text
Cargo.toml / Cargo.lock       # ordinary platform workspace; shared dependency versions
frontend/                     # shared Svelte 5 frontend
core/                         # Tauri-free Rust business logic and connection engine
  src/commands/               # shared operations used by host adapters
  src/{db,ssh,terminal,sync}/  # storage, transports and configuration sync
app/                          # application adapters; library name remains rssh_lib
  src/tauri/                  # GUI startup and Tauri wire command wrappers
  src/server/                 # headless HTTP/WebSocket adapter
  src/server_main.rs          # rssh-server binary (server feature)
  capabilities/               # maintained Tauri frontend permissions
  gen/                        # ignored native projects and schemas
  bin/                        # ignored CLI staging for desktop packaging
cli/                          # standalone rssh-cli binary, depends on rssh-core
desktop/scripts/              # desktop CLI resource preparation
android/                      # maintained Gradle/Kotlin sources, staged into app/gen/android
ios/                          # maintained iOS plist and privacy manifest
icons/                        # shared source images and platform icon sets
ohos/
  Cargo.toml / Cargo.lock      # independent workspace, directly depends on app/core
  tauri.conf.json              # maintained OHOS application configuration
  app/                        # maintained phone / desktop HAPs and common runtime
  src/                        # native entry point and injected platform implementations
  framework/                  # upstream revisions, patches and tests
  scripts/                    # native project, upstream source and package metadata preparation
  tests/                      # Vitest tests against maintained ArkTS sources
idea-plugin/                  # IntelliJ plugin and browser development launcher
scripts/                      # shared build entry points, staging and platform cfg
target/                       # ignored Cargo outputs, dependency caches and packaging files
```

The root workspace contains `rssh-core` (shared engine), `rssh-app` (Tauri and
headless adapters), and `rssh-cli` (CLI). Each crate declares its own dependencies;
common versions come from the root's `workspace.dependencies`. GUI commands enable
the app's `tauri` feature through `app/tauri.conf.json`. CLI and headless builds do
not pull in Tauri or a WebView.

`rssh-ohos` has a separate workspace so its Tauri/Wry/Tao patches do not affect
other platforms. It depends directly on the original app/core crates and supplies
native services through their interfaces. Rust source files, manifests and lockfiles
are maintained in place; preparation only generates packaging metadata and resources.
To update a locked OHOS dependency, prepare its source checkouts first, then use
`cargo update` in that workspace and commit the resulting lockfile:

```bash
node ohos/scripts/prepare-ohos-sources.mjs
cargo update --manifest-path ohos/Cargo.toml -p <package>
```

The OHOS helper tests run on the development host after preparing those sources:
`cargo test --manifest-path ohos/Cargo.toml --lib --locked`.

```bash
cargo check -p rssh-cli
npm run build
cargo check -p rssh-app --features server --bin rssh-server
cargo check -p rssh-app --features tauri
cargo test --workspace --all-features
```

Android's native source project is staged for Tauri; iOS maintains plist and privacy
resources for a generated Xcode project. The IDEA plugin is a separate Gradle project
that packages the compiled headless server. These directories do not need Rust crates.

Only the maintained source directories are edited and committed. `app/gen/`,
`app/bin/`, `ohos/gen/` and build caches are disposable outputs. The build entry points
prepare them from source before invoking the platform tools. Desktop icons are
referenced directly from `icons/`; mobile packaging copies the required assets
into the generated native projects. `roadmap.md` remains at the repository root.

## Release

Push a git tag to trigger CI:

```bash
git tag v0.2.0
git push origin v0.2.0
```

GitHub Actions builds desktop and Android artifacts and creates a draft release.
Desktop/Android artifact naming:

```
rssh-{version}-{os}-{arch}.{ext}
```

Version is derived from the git tag. No need to manually update `tauri.conf.json` or `Cargo.toml` -- CI syncs them automatically.

## Code Style

- Rust: `cargo fmt` + `cargo clippy`
- Frontend: default Svelte/TypeScript conventions
- Commits: explain *why*, not *what*
