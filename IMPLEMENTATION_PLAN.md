# Phase D: SFTP ohos file bridge — feat/ohos-port

Goal: replace the ohos_file_bridge_pending stubs with a real picker bridge.

Architecture: ArkTS (EntryAbility, ours to edit) registers callback functions
into Rust via a napi export at app start; ohos SFTP commands call through
that bridge to pop system pickers. Files round-trip through the app cache
dir (picker URIs are only openable from ArkTS; Rust gets plain paths).
The fd pass-through optimization (no double copy) is deliberately deferred.

## Phase 1: Rust bridge module
napi export `register_ohos_file_bridge(obj)` holding global callbacks;
ohos-only module, no effect on other targets.
**Tests**: desktop suite untouched/green; ohos build compiles the symbols.
**Status**: 未开始

## Phase 2: ArkTS side
FileBridge: documentViewPicker (save/select) + fs stream copy to/from cache.
Registered from EntryAbility.onCreate.
**Tests**: compiles into the HAP; runtime check in phase 4.
**Status**: 未开始

## Phase 3: Command wiring
sftp_pick_save_path / sftp_pick_open_path / sftp_download_to /
sftp_upload_from get real ohos implementations (stubs removed);
save-file.ts gains an ohos branch (it imports plugin-dialog directly).
**Tests**: desktop cargo test + vitest green.
**Status**: 未开始

## Phase 4: Emulator verification
Upload a file into a session dir and download one back through real
pickers; confirm copies land in user-visible storage.
**Status**: 未开始
