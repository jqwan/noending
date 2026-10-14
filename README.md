# NoEnding

> **Conversations end. Context doesn't.**
> 对话会结束，上下文不会。

NoEnding 是一个本地多 Agent 工作空间，基于 Tauri 2、React、TypeScript、Rust 和 SQLite 构建，支持 macOS 与 Windows。
它把分散在不同 Agent 中的会话、项目和任务放到同一个界面，并持续保存工作上下文，让工作能够跨会话、跨 Agent 延续。

## 能做什么

- **管理会话**：统一阅读、搜索不同 Agent 的本地历史，在终端或对应桌面应用中继续。
- **新建会话**：选择项目、工作目录和 Agent，可选跟踪任务，输入消息后启动内嵌终端。
- **组织工作**：项目按 Git 仓库及 worktree、单目录或内置目录组组织；任务关联项目，一个会话可跟踪多个任务。
- **积累上下文**：按需更新或重新生成会话、任务和项目摘要，同时提取用户及项目偏好，支持手动管理和锁定偏好。
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

1. 在「Agent」查看安装情况、重新检测 CLI，并管理历史来源的自动同步、目录和手动同步。模型列表可手动刷新，获取后会保留。
2. 在「新会话」选择项目、工作目录和 Agent，在标语下多选该项目的跟踪任务（可选），设置模型与思考等级，再输入消息发送。首次使用 Agent 原生参数，之后沿用该 Agent 上一次成功新建会话的模型设置；项目、目录、Agent 和消息草稿也会记住。Enter 发送，Shift+Enter 换行。
3. 在「会话」阅读、搜索或继续历史会话，通过顶部图标切换历史消息、详情和终端。同一会话可打开多个终端；退出的终端仍可从侧栏进入，已绑定会话的终端可重新连接，也可关闭。
4. 在会话、任务或项目详情中「更新摘要」或「重新生成」。在「设置」配置提取使用的 Agent、模型、思考等级和提示词，并管理用户偏好；项目偏好在项目详情管理。
5. 使用助手时，在「新会话」选择内置的 NoEnding Assistant 项目并发送需求，其会话与其他会话一样显示在侧栏。可在助手目录的 `USER.md` 中补充自己的使用要求。
6. 项目、任务和会话可独立归档或取消归档。已归档的项目和任务不能用于新建会话，已归档会话不能继续；项目归档不影响关联任务和会话的归档状态。永久删除需先归档，项目还需没有关联会话，聊天和助手项目不能永久删除。

永久删除只清理 NoEnding 本地数据，不删除 Agent 源文件；后续同步可能重新摄入仍有源文件的会话。

「设置」中可选择浅色、深色或跟随系统，内嵌终端随应用切换配色；Agent 自身的 TUI 主题行为由其控制，Codex CLI 可能需要重新连接才生效。默认存储目录为 macOS 的 `~/.noending/`、Windows 的 `%USERPROFILE%\.noending\`，可在设置中更改，重启后生效。

也可通过 CLI 查询历史、管理偏好和任务、更新摘要。保持应用运行，使用 `ne --help` 查看命令；`ne` 和 `noending` 使用同一套命令。安装及用法见 [CLI 使用说明](docs/cli.md)。

开发约定与验证规则见 [AGENTS.md](AGENTS.md)。
