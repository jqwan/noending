# NoEnding

> **Conversations end. Context doesn't.**
> 对话会结束，上下文不会。

NoEnding 是一个本地多 Agent 工作空间，基于 Tauri 2、React、TypeScript、Rust 和 SQLite 构建，支持 macOS 与 Windows。
它把分散在不同 Agent 中的会话、项目和任务放到同一个界面，并持续保存工作上下文，让工作能够跨会话、跨 Agent 延续。

## 能做什么

- **管理会话**：统一阅读、搜索不同 Agent 的本地历史，在终端或对应桌面应用中继续。
- **新建会话**：选择任务、工作目录和 Agent，输入消息后启动内嵌终端。
- **组织工作**：用任务关联会话和工作目录；项目支持 Git 仓库及 worktree、普通目录和默认聊天目录。
- **积累上下文**：按需更新或重新生成会话、任务和项目摘要，保留关键结论、进展与待解决问题。
- **工作区助手**：在内置项目中使用 Codex、Claude Code 或 Pi，查询工作资料并通过 CLI 管理工作数据。

会话历史和工作数据保存在本机；需要 AI 的操作通过用户自己的 Agent CLI 执行。

## 支持的 Agent

| Agent | 新建会话 | 继续会话 |
| --- | --- | --- |
| Codex | `codex` CLI | CLI 或 Codex 桌面端 |
| Claude Code | `claude` CLI | CLI |
| Pi | `pi` CLI | CLI |
| Antigravity | `agy` CLI | CLI 会话用 CLI；IDE 会话打开应用 |
| Qoder | — | 打开 Qoder 应用 |
| WorkBuddy | — | 桌面端定位到会话 |
| dsh | — | 打开 DeepSeek Harness 应用 |
| ZCode | — | 打开 ZCode 应用 |

以上 Agent 均支持历史摄入。「打开应用」表示只打开桌面应用，不自动定位会话。
新建需要安装对应 CLI，桌面端继续需要安装对应应用。

## 运行工程

需要 Node.js 20、pnpm 9、Rust stable，以及对应平台的 Tauri 2 构建工具。

```bash
git clone https://github.com/jqwan/noending.git
cd noending
pnpm install --frozen-lockfile
pnpm tauri dev
```

生成安装包：

```bash
pnpm tauri build
```

构建产物位于 `src-tauri/target/release/bundle/`。

## 使用

1. 在「代理」总览进入各代理详情，查看安装情况、重新检测 CLI、选择是否用于新建会话，并设置默认模型、提供商和推理强度。模型列表按需从 CLI 获取；无法获取时仍可使用原生默认或手动输入。Claude Code 展示建议别名。
   历史来源可分别启停自动同步、添加目录或手动同步。关闭历史自动同步后，NoEnding 启动的终端仍按 Agent 通知同步当前会话并精确绑定。
2. 在「新会话」选择任务（可选）、项目/工作目录和 Agent，输入消息后发送。可沿用代理页的运行参数、使用 Agent 原生默认，或单次自定义模型等参数；单次设置不写回默认值。Enter 发送，Shift+Enter 换行。
3. 在「会话」阅读、搜索或继续历史会话。退出的终端仍可从侧栏进入，已绑定会话的终端可重新连接，也可手动移除。
4. 在会话详情「更新摘要」，在「任务」组织相关会话并更新任务摘要，在项目详情更新项目摘要，持续维护工作上下文；在「助手」选择查询范围并新建原生会话，查询和处理工作信息。可在助手目录的 `USER.md` 中补充自己的使用要求。
5. 任务和会话可以归档、取消归档。归档任务不能新建会话，归档会话不能继续；只有已归档项可以永久删除。

永久删除只清理 NoEnding 本地数据，不删除 Agent 源文件；后续同步可能重新摄入仍有源文件的会话。

「设置」中可选择浅色、深色或跟随系统，内嵌终端同步切换主题。默认存储目录为 macOS 的 `~/.noending/`、Windows 的 `%USERPROFILE%\.noending\`，可在设置中更改，重启后生效。

也可通过 CLI 查询历史、管理偏好和任务、更新摘要。保持应用运行，使用 `ne --help` 查看命令；`ne` 和 `noending` 使用同一套命令。安装及用法见 [CLI 使用说明](docs/cli.md)。

开发约定与验证规则见 [AGENTS.md](AGENTS.md)。
