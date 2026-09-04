# RSSH

[English](README.md) | [中文](README_zh.md)

**一款真正不一样的 SSH 工具——解决传统客户端一直没人管的痛点。**

> 传统客户端给你一个 shell 就到此为止: 翻不完的滚屏、背不住的静态 IP、把日志复制粘贴给聊天机器人。
> 
> RSSH 继续往前走：命令变成可导航的区块，容器实时发现，AI像运维一样读终端输出
> 
> 桌面 · 手机 · JetBrains · 命令行，一套数据通用。

[![Release](https://img.shields.io/github/v/release/rssh-org/rssh)](https://github.com/rssh-org/rssh/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/rssh-org/rssh/total)](https://github.com/rssh-org/rssh/releases)
![Platforms](https://img.shields.io/badge/macOS%20·%20Windows%20·%20Linux%20·%20Android·%20iOS-555)
[![License](https://img.shields.io/github/license/rssh-org/rssh)](LICENSE)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/rssh-org/rssh)

<p align="center">
  <img src="https://rssh.ofcoder.com/img_local.png" alt="RSSH —— 问一句，AI 提议命令，你点同意才执行" height="180">
  <img src="https://rssh.ofcoder.com/img_blocks_context_menu.png" alt="彩色命令块与右键菜单" height="180">
  <img src="https://rssh.ofcoder.com/img_ai_panel.png" alt="RSSH —— AI 面板读终端上下文，提议命令待批准" height="180"> 
</p>

<p align="center"><b><a href="https://github.com/rssh-org/rssh/releases">⬇️ 下载最新版</a></b> &nbsp;·&nbsp; <a href="https://github.com/rssh-org/docs/blob/main/article_zh.md">为什么是 RSSH？</a></p>

---

## 为什么选 RSSH

<table>
<tr>
<td width="50%" valign="top">

### 🤖 AI 排障
不是又一个聊天框，它不需要你在服务器上装任何软件，它模拟人类的操作，直接读取终端的输入输出。

<img src="https://rssh.ofcoder.com/welcome-ai.gif" alt="AI 排障：读终端，提议命令" width="400">

</td>
<td width="50%" valign="top">

### 🎨 彩色命令块
每条命令和它的输出自动成块、左侧按色分隔，一眼找到上一条命令的输出在哪儿。**纯本地渲染**，零远端依赖。

<img src="https://rssh.ofcoder.com/welcome-blocks.gif" alt="彩色命令块" width="400">

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 🐳 动态发现
容器和 Pod 时时刻刻都在变，过去通过 IP 连接服务器的方案已经过时了，RSSH 会动态发现本地生产测试的容器。

<img src="https://rssh.ofcoder.com/welcome-discovery.gif" alt="动态发现：容器实时出现在 Home" width="400">

</td>
<td width="50%" valign="top">

### 🔐 多平台数据同步
密钥保存在你本地的 keyserver 程序中，连接配置加密后保存在你的 GitHub 私有仓库里面，不需要保存在第三方服务器。

<img src="https://rssh.ofcoder.com/welcome-sync.gif" alt="安全与同步：密钥进钥匙串，配置加密到 GitHub" width="400">

</td>
</tr>
</table>

---

## 功能

- **SSH** —— 密码、私钥、SSH Agent/Pageant、键盘交互、跳板机（ProxyJump）
- **Telnet** —— 保存连接配置、回显协商、行规程设置、expect/send 登录脚本
- **串口控制台（桌面端）** —— 保存 UART 配置、文本/Hex 模式、流控、登录脚本、DTR/RTS/Break 控制
- **动态发现（Docker/K8S）** —— 通过本机 CLI context 发现 Docker 容器和运行中的 Kubernetes Pod，直接打开临时 exec 终端；[为什么需要动态发现](https://github.com/rssh-org/docs/blob/main/article_dynamic_discovery_zh.md)
- **终端** —— xterm 仿真、10 000 行回滚、可折叠彩色命令块、正则高亮、搜索
- **多会话工作台（桌面端）** —— 实时预览已连接终端，并向选中的会话广播内容
- **SFTP** —— 远程文件浏览和上传/下载
- **端口转发** —— 本地、远程和动态（SOCKS5），命名配置，实时流量统计
- **本地终端（桌面端）** —— 自动识别 zsh/bash/PowerShell
- **会话录制** —— asciicast v2 格式，变速回放
- **连接与凭据** —— SQLite 存储，可从 `~/.ssh/config` 导入
- **安全与同步** —— Secret 使用 ChaCha20-Poly1305 本地加密，系统可用时由钥匙串保管主密钥；可选择远程同步范围，并加密备份到你自己的 GitHub 仓库或 WebDAV 服务器
- **片段** —— 可复用命令快捷键（Cmd+E）
- **移动端** —— 虚拟键盘栏（Ctrl/Alt/方向键/Tab/Esc）、安全区、栈式导航
- **IDE 插件** —— 在 JetBrains IDE 的工具窗口里运行 RSSH（共享数据目录）

## 安装

从 [Releases](https://github.com/rssh-org/rssh/releases) 下载：

| 平台                  | 文件                                       | 备注                     |
|---------------------|------------------------------------------|------------------------|
| macOS Apple Silicon | `rssh-{ver}-macos-aarch64.dmg`           |                        |
| macOS Intel         | `rssh-{ver}-macos-x86_64.dmg`            |                        |
| Linux (deb)         | `rssh-{ver}-linux-x86_64.deb`            | Debian/Ubuntu          |
| Linux (rpm)         | `rssh-{ver}-linux-x86_64.rpm`            | Fedora/RHEL            |
| Linux (AppImage)    | `rssh-{ver}-linux-x86_64.AppImage`       | 任意发行版                  |
| Windows             | `rssh-{ver}-windows-x86_64.msi`          | 静默安装：`msiexec /i`      |
| Windows             | `rssh-{ver}-windows-x86_64-setup.exe`    | 图形安装器                  |
| Windows             | `rssh-{ver}-windows-x86_64-portable.zip` | 免安装 GUI + CLI          |
| Android (arm64)     | `rssh-{ver}-android-arm64.apk`           | 64 位手机，体积约 1/3      |
| Android             | `rssh-{ver}-android-universal.apk`       | 全 ABI（含模拟器）         |
| iOS                 |                                          | AppStore，由 [@paradoxie](https://github.com/paradoxie) 维护 |

### IntelliJ / JetBrains 插件

在 JetBrains IDE 的工具窗口里运行完整 RSSH —— 与桌面版共享同一套主机、密钥、设置
（共享 `~/.rssh`）。每个 zip 内置 headless `rssh-server`，自包含、按平台区分：

| 平台                  | 文件                                              |
|---------------------|--------------------------------------------------|
| macOS Apple Silicon | `rssh-{ver}-macos-aarch64-jetbrains-plugin.zip`  |
| macOS Intel         | `rssh-{ver}-macos-x86_64-jetbrains-plugin.zip`   |
| Linux               | `rssh-{ver}-linux-x86_64-jetbrains-plugin.zip`   |
| Windows             | `rssh-{ver}-windows-x86_64-jetbrains-plugin.zip` |

安装：**Settings → Plugins → ⚙ → Install Plugin from Disk…**，选对应平台的 zip 后重启。
打开底部 **RSSH** 工具窗口即可使用；标题栏的 ✕ 停止内置 server。

## 开发

参见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 协议

MIT

## 友情链接

- [LINUX DO - 新的理想型社区](https://linux.do/)
