# RSSH 鸿蒙构建

鸿蒙 GUI 复用 `src-tauri/src/lib.rs` 和现有业务模块，通过单独的 Cargo
workspace 使用固定的 Tauri/Wry/Tao OHOS 分支及 ability 1.0 beta 插件框架。
macOS / Windows / Linux、Android、iOS、CLI 和 headless 继续使用根 `src-tauri/Cargo.toml` / `Cargo.lock` 的官方依赖。

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
默认构建要求两个 signed HAP 与签名 `.app` 都存在；缺少任一个就报错退出。
模拟器人工验收可显式使用 `--unsigned`，不能将未签名产物用于商店发布。

## 构建

```bash
./build-ohos.sh             # 正式签名包
./build-ohos.sh --unsigned  # 模拟器人工验收用未签名包
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
4. 将本次成功构建的库、运行时依赖和前端资源复制到 `common/runtime` HAR，
   由两个产品入口静态依赖；不分别维护两份实现。
5. 执行 `ohpm install`、`hvigor assembleApp --mode project -p product=default
   -p buildMode=release`，同时生成手机与电脑 HAP，再合成一个 App Pack。
   普通模式要求签名；`--unsigned` 显式关闭签名任务。

任一步失败都会中止。脚本先清理旧 HAP、App Pack 和暂存产物，编译失败不会继续
用旧 `.so` 打包；也不会修改 DevEco 的 `hvigorfile.ts` 或重新生成工程。

输出：

```text
src-tauri/gen/ohos/products/phone/build/default/outputs/default/entry-default-signed.hap
src-tauri/gen/ohos/products/desktop/build/default/outputs/default/desktop-default-signed.hap
src-tauri/gen/ohos/build/outputs/default/RSSH.app
```

`--unsigned` 将 HAP 文件名中的 `signed` 改为 `unsigned`，App Pack 名为
`RSSH-unsigned.app`。两种模式都编译 release，不启用 Tauri devtools。

当前脚本仅构建 ARM64。产出安装包不代表设备行为已经验证；安装、模拟器
和真机测试应另行执行。`CARGO_TARGET_DIR` 可指定 Cargo 缓存，默认使用独立的
`src-tauri/target/ohos`。

### 发布版本

`versionName` 跟随 `src-tauri/tauri.conf.json`，必须与 Cargo package version
一致。准备脚本仅更新派生锁文件中 RSSH 自身的版本，依赖版本仍由已提交的
OHOS 锁文件固定。组装时临时应用版本信息，结束或失败后恢复源 AppScope 文件。

商店发布前需递增 `versionCode`，通过环境变量传入：

```bash
OHOS_VERSION_CODE=3 ./build-ohos.sh
```

未传入时沿用 AppScope 中的值，不会自动递增。当前 GitHub 发布流程没有
鸿蒙签名打包任务，正式 HAP 和 App Pack 需在已配置签名的 DevEco 环境中构建。

## 独立产品入口与共享核心

采用华为推荐的产品入口 + 公共 HAR 结构：

```text
src-tauri/gen/ohos/
├── products/phone       entry HAP，deviceTypes: phone / tablet
├── products/desktop     desktop HAP，deviceTypes: 2in1
└── common/runtime       公共 HAR：NativeAbility、插件、Rust 库、前端资源
```

两个入口的设备范围互不重叠，同属 `com.rssh.app`、同版本、同签名，一次
`assembleApp` 将两个 HAP 放入 `RSSH.app`。商店依据 `deviceTypes` 向手机／平板
分发 `entry`，向电脑分发 `desktop`。公共 HAR 在编译时合入各自 HAP，设备无需
另外安装 HAR，也不需要下载另一个产品的 HAP。

手机保留原模块名 `entry` 和触控方向声明；电脑使用独立 `desktop` 入口，
单独声明主窗口置顶权限。两个入口依赖同一公共运行时，保留 Rust / ArkTS 插件
注册契约；串口系统模块仍只在满足 API 和系统能力条件时按需加载。

Rust 编译条件统一为 `linux` / `macos` / `windows` / `android` / `ios` /
`ohos`；`linux` 排除 OHOS。两个鸿蒙 HAP 共用一份 `librssh_lib.so`，
所以只有一个 `ohos` 编译条件。运行时在 `ohos::device::DeviceClass` 中
将 `phone` / `tablet` 归为 `Mobile`，将 `2in1` 归为 `Desktop`；未知类型
归为 `Other`，不会自动获得电脑专属能力。设备类别只用于原生 API 的适用范围，
具体功能仍须通过系统能力查询或运行时探测。

页面布局只随窗口或容器宽度改变，输入按实际触摸、鼠标和键盘事件处理；
`get_runtime_capabilities` 返回当前宿主实际实现的能力，前端加载成功后才
挂载页面。拆成两个安装包不会改变系统的应用沙箱权限。

官方参考：[电脑应用包管理策略](https://developer.huawei.com/consumer/cn/doc/doccenter-multi-device/bpta-pc-guide)、
[工程管理与公共模块](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides-V13/ide-using-V13)。

### 从旧通用 HAP 升级

本轮默认 `versionCode` 从 1 升为 2；应用包名、主 Ability 名、应用级数据目录
保持不变。原型电脑版的模块名由 `entry` 改成 `desktop`，必须使用更高版本号，
避免同版本 entry 唯一性冲突。不要先卸载或清数据来掩盖升级问题；覆盖安装、
原连接配置和密钥保留需要在设备上验收。若安装失败，保留错误信息与现有数据。
参见[官方安装更新一致性规则](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/quick-start/install-and-update-consistency-verification.md)。

### 宿主能力

- **窗口**：Tao 为每个窗口分配独立 ID，Wry 把 ArkWeb 绑定到该窗口的
  UIContext。系统关闭经过 Tauri 的 CloseRequested / Destroyed 流程；
  Ability 重建保留逻辑窗口及会话。置顶仅开放给系统允许的主窗口。
- **文件**：手机与电脑提供多选，目录传输按原生能力开放。原生文件引用保持
  不透明，目录授权、后代路径解析、父目录创建和文件描述符由原生适配器处理。
- **本地终端**：所有桌面目标共用 `portable-pty 0.9`。鸿蒙电脑在应用沙箱内
  探测 openpty、启动 shell 和正常退出，成功后才开放入口，不能用 SDK
  含有函数声明代替运行权限验证。
- **串口**：优先使用 API 26 的 `SystemCapability.BusManager.Serial`，
  否则使用 API 19 的 `SystemCapability.USB.USBManager.Serial`。均通过公开
  系统接口授权和异步收发，不直接打开私有设备节点；未插设备时仍保留串口入口。
  API 19 支持 SDK 列举的波特率、数据位、校验和停止位；API 26 还支持自定义
  波特率、流控、XANY、DTR、RTS、BREAK。前端通过 `serial_get_capabilities`
  展示可用设置，同步来的不支持配置保留原值并提示修改，连接时后端也会拒绝，
  不静默改变配置。所有桌面继续共用 Hex、换行转换、慢速发送、回显和登录脚本。
  BREAK 脉冲时长由鸿蒙系统决定；电气收发、芯片兼容性和控制线需 USB 硬件验收。
- **外部工具**：Docker / kubectl 继续真实探测可执行程序。HAP 不提供桌面
  CLI 安装、读取用户默认 SSH 密钥目录或 SSH Agent 入口；通过文件选择器
  导入的密钥、密码认证和远程 SSH/SFTP 不受影响。
- **插件**：资源访问范围绑定到实际应用数据目录下的 plugins；不假设桌面
  HOME 或 APPDATA 路径。Headless 不提供原生资源协议，因此关闭插件界面。
- **AI 本地分析**：提示词与工具执行使用同一宿主能力检查。鸿蒙电脑不会因
  `mobile` 编译标记被误认为手机；只有额外窗口和本地终端实际可用时才开放
  `analyze_locally`。SSH 的诊断文件下载独立可用，保存到应用诊断目录。
- **密钥存储**：新安装通过官方 Asset Store Kit（API 11）保存数据库加密
  主密钥；启动时实际探测服务。业务密码仍由共用 `HybridStore` 加密后存入
  数据库。已有 `file` 后端不自动迁移、不更换主密钥；已选系统密钥库后若服务
  不可用则报错，不静默回退。Asset 只启用同设备系统备份，不启用跨设备或
  账号云同步；跨设备迁移使用 RSSH 配置导出/同步。

串口接口依据：[官方 API 19 串口管理](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/reference/apis-basic-services-kit/js-apis-serialManager.md)
及 SDK 的 `@ohos.usbManager.serial.d.ts`、`@ohos.busManager.serial.d.ts`。

## 四条入口与设备验收

| 入口 | 处理方式 |
| --- | --- |
| 桌面 GUI | 常规桌面使用官方 Tauri 插件；鸿蒙电脑通过 ability 原生适配。两者共用业务命令、会话 registry 和桌面前端。 |
| 移动 GUI | Android / iOS 使用相同业务命令及官方插件；OHOS 通过 ability 的 files/url/permission 插件及应用能力插件支持文件选择、文件描述符传输、剪贴板和外链。 |
| CLI | 无新增 CLI 命令，继续使用官方依赖及共享业务模块。 |
| Headless / JetBrains | 剪贴板由 IPC shim 调浏览器；文本导出在浏览器用 Blob 下载，JetBrains 继续明确提示不支持。文件多选与目录入口同时要求后端支持和宿主提供选择器；JetBrains 桥准备完成后刷新能力。`resolve_local_paths`、`open_path`、`serial_get_capabilities` 均有 server 分发，串口指服务端主机的端口。 |

页面排版随窗口或容器宽度变化，窄窗口与宽窗口不按系统或 UA 区分。
原生功能只按宿主能力开放，触摸、鼠标和键盘各自按输入事件处理。
鸿蒙手机的系统 API 不提供目录选择，因此不开放目录上传/下载。OHOS 向用户选定
URI 的文件描述符流式写入；中途失败可能保留部分文件，不保证原子替换。

配置和密钥导入继续共用 HTML 文件输入框。ArkWeb 在未覆盖
`onShowFileSelector` 时提供系统文件选择界面，无需另写鸿蒙导入命令。
参见[官方文件选择事件说明](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/reference/apis-arkweb/arkts-basic-components-web-events.md#onshowfileselector9)。

当前底层 `ohos-web-binding 0.2.3` 的 JavaScript proxy 与 scheme callback
使用裸指针保存回调上下文，缺少对应的 Rust 回收路径。适配层会释放应用
闭包并拒绝旧 WebView 的回调，但重复创建 WebView 仍可能积累这些原生
回调元数据。此上游缺口尚未修复；构建和单元测试不能证明原生层无泄漏。

鸿蒙 IPC 使用 Tauri 现有的 `postMessage` 通道。自定义资源协议仅接受
GET/HEAD，其它方法返回 501，避免进入上游异步请求体读取中生命周期不安全
的缓冲区路径；普通 HTTP/HTTPS 请求不受此限制。

设备验收重点（本轮交由用户执行）：

- **首先验收升级**：在原手机与电脑应用上覆盖安装对应 HAP，确认名称／图标正常，
  原连接、密码／密钥、主题／语言仍可用。电脑安装后应为 `desktop` 模块，手机为 `entry`。
- 冷启动、退出重开均能连接 SSH，终端有输出、输入和中文粘贴正常。
- 调整窗口宽度跨过 640px，设置导航、侧栏偏好和终端按键条随宽度切换；
  放宽后软键盘入口仍可点击，已有终端连接不重建。
- 窄窗口锁定 Ctrl/Alt 后拉宽，普通字符恢复普通输入；关闭后台终端标签后，
  当前终端的 Esc、Tab、方向键仍发送到正确会话。
- 触摸长按选字后使用外接鼠标/键盘，右键、首个输入字符和中文输入均正常；
  终端滚动条仍可用鼠标拖动，插件侧栏与横条有可点击的关闭按钮。
- 电脑端独立窗口、最大化/还原、主窗置顶、关闭子窗不影响主窗会话。
- 电脑端键鼠输入、快捷键、SSH 分屏，以及本地终端能力探测结果。
- 电脑端多选上传、包含中文与特殊字符的嵌套目录上传/下载。
- 串口：未插设备时可进入编辑器并刷新；授权拒绝后可重试，授权期间关闭
  标签/窗口后再同意也不会留下占用；同一端口不能在两个标签同时打开。
  连续关闭、立即重连应等待旧端口释放，不应误报“端口占用”。
  系统授权弹框不能由应用强制关闭；取消连接后若弹框仍在，请在系统弹框中
  同意或拒绝，以完成迟到授权的清理。原生关闭失败必须报错，不能冒充关闭成功。
- API 19：115200 / 8N1 和 2 停止位收发，大段粘贴、Hex、换行、慢速发送、
  回显及脚本；同步含流控、XANY 或不支持波特率的配置后，原值保留并明确提示。
- API 26：额外验收硬件/软件流控、XANY、DTR、RTS、BREAK；需支持相应信号的
  真实 USB 串口硬件。两条 API 路径均验收拔出、重插、刷新与重连。
- AI：具备本地终端的电脑不再收到“手机不支持”提示；本地终端不可用时
  提示实际缺失能力，常规 SSH 诊断和文件下载仍可使用。
- 密钥：覆盖升级后原密码和私钥仍能连接；保存新凭据后退出重开仍可使用。
  如有额外的全新设备/模拟器，可验证首次选择系统密钥库后的保存与重启，
  不要为了覆盖此场景删除现有数据。

- 选择文件上传、取消上传，以及包含中文的文件名。
- 下载保存、同名文件确认，以及取消和失败后的文件状态。
- 复制粘贴及系统授权、打开外部链接。
- 重启后的偏好设置。原型版本的主题、语言等 UI 偏好不迁移。
- 横屏布局、后台切回后的终端会话与文件传输。

### 人工安装验收

先启动需要验收的模拟器，用 SDK 的 `hdc list targets` 查询设备 ID，再分别
安装对应 HAP（以下命令从仓库根目录运行，将设备 ID 占位符替换为实际值）：

```bash
hdc -t <手机设备ID> install -r src-tauri/gen/ohos/products/phone/build/default/outputs/default/entry-default-unsigned.hap
hdc -t <电脑设备ID> install -r src-tauri/gen/ohos/products/desktop/build/default/outputs/default/desktop-default-unsigned.hap
```

随后从模拟器桌面启动 RSSH。不要把两个 HAP 同时装到同一设备。正式发布则
将签名的 `RSSH.app` 交给应用市场按设备分发；未签名 App Pack 仅供本地检查。

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
git -C src-tauri/target/ohos-sources/tao add -N -- Cargo.toml src
git -C src-tauri/target/ohos-sources/tao diff --binary HEAD -- Cargo.toml src > src-tauri/ohos/patches/tao.patch
```

`add -N` 使新增文件进入差异；仅执行 `git diff` 会漏掉新模块。导出后须离线
重放补丁并运行框架检查，确认重放后的源码可编译。

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
离线缓存重放、补丁失败中止、正常 Cargo 文件及 `dist/` 保持原样，签名模式不能退回未签名产物，
两个 HAP 或 App Pack 缺一不可，以及前端、
源码准备、依赖解析、Rust 或 OHPM 失败时不会继续组装旧 HAP。它们不运行 GUI、服务器、模拟器或设备操作。
