# RSSH 鸿蒙构建

鸿蒙 GUI 复用 `src-tauri/src/lib.rs` 和现有业务模块，通过单独的 Cargo
workspace 使用固定的 Tauri/Wry/Tao OHOS 分支及 ability 1.0 beta 插件框架。
桌面、Android、iOS、CLI 和 headless 继续使用根 `src-tauri/Cargo.toml` / `Cargo.lock` 的官方依赖。

## 环境

- DevEco Studio 与完整 HarmonyOS SDK（含 `native/`、hvigor、ohpm）。
- Rust stable、Node.js 18+、npm、Git、Bash。
- `ohrs` 固定为 1.5.0。无需替换系统或 npm 的 Tauri CLI。

```bash
cargo install ohrs --version 1.5.0 --locked
rustup target add aarch64-unknown-linux-ohos
npm ci
```

`OHOS_HOME` 指向完整 SDK 根目录，即包含 `native/` 的目录。脚本会探测
macOS DevEco 常见路径，也支持显式设置：

```bash
export OHOS_HOME="/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony"
```

非标准安装位置还可以设置 `DEVECO_SDK_HOME`、`HVIGOR_BIN`（`hvigorw.js`
文件路径）和 `OHPM_BIN`（`ohpm` 可执行文件路径）。脚本不会安装 SDK、
启动模拟器、连接设备或安装应用。

## 签名

用 DevEco Studio 打开 `src-tauri/gen/ohos`，在 Project Structure →
Signing Configs 中配置签名。签名材料保存在本机，不提交仓库。
构建脚本要求产出 signed HAP；缺少签名时会报错退出。

## 构建

```bash
./build-ohos.sh
```

脚本按顺序执行：

1. 用普通 `npm run build -- --outDir ...` 将前端生成到
   `src-tauri/target/ohos-frontend`。所有平台共用相同前端入口；独立产物目录
   保证鸿蒙构建不覆盖普通 GUI/server 使用的 `dist/`。
2. 根据 `src-tauri/ohos/sources.json` 获取完整 commit 固定的上游源码，
   逐个校验并应用 `patches/` 中的补丁，再生成
   `src-tauri/target/ohos-sources` 和 DevEco 的 ability HAR 模块。
3. 从根 Cargo manifest 派生 `src-tauri/target/ohos-workspace`，复用同一份
   `src/`、`build.rs`、capabilities、图标及资源，加载 OHOS 专属依赖与独立
   `Cargo.lock`。运行 `cargo metadata --locked` 和
   `ohrs build --locked --lib --features custom-protocol`。
4. 将本次成功构建的库、运行时依赖和前端资源复制到 DevEco 工程。
5. 执行 `ohpm install`、hvigor HAP 组装，并检查 signed HAP 存在。

任一步失败都会中止。脚本先清理旧 HAP 和暂存产物，编译失败不会继续
用旧 `.so` 打包；也不会修改 DevEco 的 `hvigorfile.ts` 或重新生成工程。

输出：

```text
src-tauri/gen/ohos/entry/build/default/outputs/default/entry-default-signed.hap
```

当前脚本仅构建 ARM64。产出 HAP 不代表设备行为已经验证；安装、模拟器
和真机测试应另行执行。

## 四条入口与设备验收

| 入口 | 处理方式 |
| --- | --- |
| 桌面 GUI | 使用官方 Tauri 文件、剪贴板、外链插件；文件操作统一走原生后端命令。 |
| 移动 GUI | Android / iOS 使用相同业务命令及官方插件；OHOS 通过 ability 的 files/url/permission 插件及应用能力插件支持文件选择、文件描述符传输、剪贴板和外链。 |
| CLI | 无新增 CLI 命令，继续使用官方依赖及共享业务模块。 |
| Headless / JetBrains | 剪贴板由 IPC shim 调浏览器；文本导出在浏览器用 Blob 下载，JetBrains 继续明确提示不支持。补齐 `open_path` 分发。 |

手机目录上传和目录下载不提供，与现有移动 UI 一致。OHOS 向用户选定
URI 的文件描述符流式写入；中途失败可能保留部分文件，不保证原子替换。

配置和密钥导入继续共用 HTML 文件输入框。ArkWeb 在未覆盖
`onShowFileSelector` 时提供系统文件选择界面，无需另写鸿蒙导入命令。
参见[官方文件选择事件说明](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/reference/apis-arkweb/arkts-basic-components-web-events.md#onshowfileselector9)。

当前底层 `ohos-web-binding 0.2.3` 的 JavaScript proxy 与 scheme callback
使用裸指针保存回调上下文，缺少对应的 Rust 回收路径。适配层会释放应用
闭包并拒绝旧 WebView 的回调，但重复创建 WebView 仍可能积累这些原生
回调元数据。此上游缺口尚未修复；构建和单元测试不能证明原生层无泄漏。

设备验收由用户执行，重点检查：

- 选择文件上传、取消上传，以及包含中文的文件名。
- 下载保存、同名文件确认，以及取消和失败后的文件状态。
- 复制粘贴及系统授权、打开外部链接。
- 重启后的偏好设置。原型版本的主题、语言等 UI 偏好不迁移。
- 横屏布局、后台切回后的终端会话与文件传输。

## 依赖隔离与更新

`src-tauri/Cargo.toml` 是共享依赖的唯一来源。`scripts/prepare-ohos.mjs`
保留它的内容，只在派生文件中固定 `tauri` 与 `tauri-build`，再加入 OHOS
专属依赖和 patch。**不要把 OHOS patch 放回根 manifest**：Cargo 的 patch
作用于整张依赖图，包括在主机上运行的 build-dependencies，并不能通过
`target.cfg` 隔离。

普通构建使用 `src-tauri/Cargo.lock`；OHOS 构建只使用
`src-tauri/ohos/Cargo.lock`。OHOS 源码中的其他平台实现不进入普通依赖图。

`sources.json` 固定上游 commit；`patches/ability.patch`、`tao.patch`、
`wry.patch`、`tauri.patch` 记录兼容改动。Rust 与 ArkTS 从同一 ability 源码
生成，避免手工维护两份框架。`target/ohos-sources` 和
`gen/ohos/vendor/ability` 都是生成目录；直接修改会在下一次准备时被覆盖。
框架变更应先导出补丁并重新执行准备，不能修改 `~/.cargo/git/checkouts`。

例如修改生成的 Tao checkout 后，导出业务源码和 manifest 的差异：

```bash
git -C src-tauri/target/ohos-sources/tao diff --binary HEAD -- Cargo.toml src > src-tauri/ohos/patches/tao.patch
```

不要把上游 checkout 中独立诊断构建生成的 `Cargo.lock` 放入补丁；应用的
完整依赖图由 `src-tauri/ohos/Cargo.lock` 固定。

源码缓存在 `src-tauri/target/ohos-git`。已缓存全部固定 commit 时可离线重放：

```bash
node scripts/prepare-ohos-sources.mjs --offline
```

缺少固定 commit 或补丁不再适用都会报错中止，不会自动切换分支或跳过补丁。
完整离线构建还需要已经缓存 Rust、npm、OHPM 的依赖；`--offline` 仅控制
源码准备步骤。

共享依赖更新后，如 OHOS 的 `--locked` 检查提示锁文件过期：

```bash
node scripts/prepare-ohos-sources.mjs
node scripts/prepare-ohos.mjs
# 在派生 workspace 中按需执行 cargo update -p <包名>，审查变更后：
cp src-tauri/target/ohos-workspace/Cargo.lock src-tauri/ohos/Cargo.lock
```

同时提交共享 manifest 和适用的锁文件更新。不要手工维护第二份完整
Cargo manifest，也不要提交 `target/` 下的派生文件。

## 构建流程自动化检查

```bash
node --test scripts/prepare-ohos-sources.test.mjs scripts/prepare-ohos.test.mjs scripts/build-ohos.test.mjs
# 已准备固定源码后，检查真实 ability 框架的生命周期契约：
node --test src-tauri/ohos/tests/*.test.mjs
```

这些检查在临时目录中使用小型 Git 仓库及替身构建工具，验证固定 commit、
离线缓存重放、补丁失败中止、正常 Cargo 文件及 `dist/` 保持原样，以及前端、
源码准备、依赖解析、Rust 或 OHPM 失败时不会继续组装旧 HAP。它们不运行 GUI、服务器、模拟器或设备操作。
