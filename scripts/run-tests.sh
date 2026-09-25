#!/usr/bin/env bash
# Windows 测试运行器。为什么不能直接 cargo test：
# tauri-build 的 manifest 经 tauri_winres 只注入 bin 目标；cargo test 的测试
# 二进制没有 Common-Controls v6 manifest，而 lib 里 context-menu 等组件导入了
# comctl32 v6-only 的 API（TaskDialogIndirect 等），默认加载 v5 直接
# STATUS_ENTRYPOINT_NOT_FOUND 起不来。这里把 v6 manifest 通过链接参数嵌进
# 测试目标——配合 --lib 只影响测试二进制，不碰任何 bin。
set -e
cd "$(dirname "$0")/../src-tauri"
MANIFEST="$(cygpath -w "$PWD/windows-test.manifest")"
export RUSTFLAGS="-C link-arg=/MANIFEST:EMBED -C link-arg=/MANIFESTINPUT:$MANIFEST"
exec cargo test --lib "$@"
