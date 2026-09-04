- 补齐cli功能（group...）cli-first，rssh status CLI 子命令 — 列当前活跃 SSH session / forward / SFTP
- Host key / known_hosts 可视化
- 拆分线程，现在所有会话的所有操作都在一个线程上执行。改成线程池（注意： SFTP 重连场景、后续 Handle 操作），暂时没有瓶颈
- 命令片段搜索最近命令
- 无活动锁定密码
- 增加只读
- 隐藏标题栏
- home搜索框，改成快速连接 ？
- 导入阿里云、aws账号资源
- AI 上下文管理，压缩、历史记忆(rag?)
- AI 导入～/CLAUDE.md
- 接入cloudflare https://linux.do/t/topic/2487408/74
- https://linux.do/t/topic/2487408/70
- cat profile.html ls折叠/展开，会出现很多clear不掉的行，调整size后clear会清理 ???
- 移动端增加自定义键盘
- profile/forward 自定义icon
- iOS快捷键盘，随着软键盘升起


![Stars](https://img.shields.io/github/stars/rssh-org/rssh)
![Forks](https://img.shields.io/github/forks/rssh-org/rssh)
![Watchers](https://img.shields.io/github/watchers/rssh-org/rssh)
![Contributors](https://img.shields.io/github/contributors/rssh-org/rssh)
![Open Issues](https://img.shields.io/github/issues/rssh-org/rssh)
![Closed Issues](https://img.shields.io/github/issues-closed/rssh-org/rssh)
![Open PRs](https://img.shields.io/github/issues-pr/rssh-org/rssh)
![Last Commit](https://img.shields.io/github/last-commit/rssh-org/rssh)
![Commit Activity](https://img.shields.io/github/commit-activity/m/rssh-org/rssh)
![Commits Since Release](https://img.shields.io/github/commits-since/rssh-org/rssh/latest)
![Total Downloads](https://img.shields.io/github/downloads/rssh-org/rssh/total)
![Latest Downloads](https://img.shields.io/github/downloads/rssh-org/rssh/latest/total)
![Release](https://img.shields.io/github/v/release/rssh-org/rssh)
![Release Date](https://img.shields.io/github/release-date/rssh-org/rssh)