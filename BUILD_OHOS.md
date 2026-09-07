# RSSH 鸿蒙（HarmonyOS）打包与测试指南

`feat/ohos-port` 分支的 PoC 移植。目标只有一个：**在鸿蒙真机上开一个 SSH
会话，能敲命令、看渲染**。本指南面向在自己机器上打包的人。

## 一、前置环境

| 组件 | 要求 | 说明 |
| --- | --- | --- |
| DevEco Studio | 5.0+（含 HarmonyOS NEXT SDK） | Windows / macOS / Linux 均可 |
| Rust | rustup + stable | https://rustup.rs |
| Node.js | 18+ | DevEco 自带的也行 |
| 华为账号 | 已实名 | 签名用，免费 |

## 二、一次性设置

```bash
# 1. 拉代码切分支
git clone https://github.com/rssh-org/rssh.git
cd rssh && git checkout feat/ohos-port

# 2. 装 fork 的 tauri CLI（crates.io 官方版没有 ohos 子命令，必须装这个）
cargo install --git https://github.com/yangyongzhen/tauri --branch feat/open-harmony tauri-cli

# 3. Rust 目标（build-ohos.sh 也会自动装）
rustup target add aarch64-unknown-linux-ohos
```

网络提示：构建要拉 GitHub 的 git 依赖和 npm 包，国内网络建议先配好代理。
拉不动 `cargo install --git` 的话，可以给 git 配置加速镜像后再试。

`OHOS_HOME` 环境变量指向**完整 SDK 根目录**（包含 `native/` 的那个，不是
纯 NDK）。不设的话 `build-ohos.sh` 会自动探测常见安装路径。

## 三、签名（出能安装的包的前提，需要做一次）

1. DevEco Studio → Open → 打开 `src-tauri/gen/ohos` 工程；
2. File → Project Structure → Signing Configs → 勾选
   **Automatically generate signature**（未登录会弹华为账号登录）；
3. 之后命令行 hvigor 打包自动复用这套签名材料。

注意：`cargo tauri ohos init` 重新生成工程会清空签名配置；bundle 名不变
的话，从 `~/.ohos/config/` 恢复 material 段即可，不用重新登录。

## 四、打包

```bash
./build-ohos.sh
```

脚本做这些事：探测 SDK → 配 NDK 交叉编译环境变量 →
`cargo tauri ohos build -t aarch64 --features custom-protocol`。

### Windows 已知坑（来自已验证的真机移植实录，别慌）

- **HAP 装配最后一步报错**（`Failed to assemble HAP: 系统找不到指定的文件`）：
  Windows 上拉不起 `.bat`，属预期行为。`.so` 已经编译好了，手工装配：
  ```bash
  cd src-tauri/gen/ohos && ohpm install && cd entry && ohpm install && cd ..
  node "<DevEco>/tools/hvigor/bin/hvigorw.js" assembleHap --mode module -p product=default --no-daemon
  ```
- **hvigor 报 ENOENT / 找不到 cmd.exe、java**：PATH 必须是纯 Windows 风格
  （含 `C:\Windows\System32` + DevEco 的 node、`jbr\bin`），MSYS/Git Bash 的
  PATH 转换会坏事。
- 生成的工程 `entry/hvigorfile.ts` 若带 cargo 钩子在 Windows 上会炸，替换成
  `plugins: []` 的无钩子版（.so 已手动编译时）。

macOS / Linux 上这些坑不存在，脚本应一路走通。

## 五、装到手机

手机：设置 → 关于手机 → 连点版本号开开发者模式 → 开发人员选项 → 打开
USB 调试。然后：

```bash
hdc install -r src-tauri/gen/ohos/entry/build/default/outputs/default/entry-default-signed.hap
hdc shell "aa start -a EntryAbility -b com.rssh.app"
```

或者直接在 DevEco Studio 里 Run。

## 六、重点测试项（这就是这次 PoC 要的答案）

1. **启动**：能装上、能起，看到连接列表 UI（不闪退、不白屏）；
2. **核心链路**：新建一个 SSH 连接，敲命令，看输出——重点看字体渲染、
   颜色、有没有乱码/残影/花屏（xterm.js 在 ArkWeb 上的表现是本次最大未知数）；
3. **软键盘**：能唤起、能输入、组合键条是否正常；
4. **触控**：长按选择、复制粘贴；
5. **后台**：切到后台再回来，会话是否存活。

## 七、已知限制（预期内，不要当 bug 报）

- **SFTP 的上传/下载保存不可用**：官方文件插件没有鸿蒙实现，属于本 PoC
  刻意未做的部分（后续版本接 ArkTS filePicker 桥）；
- **部分 UI 状态重启丢失**：ArkWeb 的 localStorage 是内存降级实现；
- AI 相关功能未在鸿蒙上验证过。

## 八、遇到编译错误

- **`aws-lc-sys` 编不过**：这是已知风险点（russh 传递依赖的 C 库），
  把报错发回来，需要调整 russh 的加密后端 feature；
- 其他编译错误：把 `cargo tauri ohos build` 的完整输出发回来。

数据目录在 `/data/storage/el2/base/files/`（与
`/data/app/el2/100/base/com.rssh.app/files` 是同一存储的两个视图）。
