# NoEnding

> **Conversations end. Context doesn't.**
> 对话会结束，上下文不会。

NoEnding 是一个以 **Workstream Context** 为核心的本地多 Agent 工作空间（Local Agent Workspace）。
它将不同 Agent 的会话组织进持续演进的 Workstream：会话会结束，Agent 会切换，
Context 持续存在。支持本地会话阅读、搜索、继续会话，以及显式更新工作上下文。

当前接入见下方[支持的 Agent](#支持的-agent)。

## 领域模型

四类实体的职责固定：

- **Project** — 一组属于同一物理工作空间或 Git family 的 WorkspacePath。不是分类容器。
- **WorkspacePath** — 被登记的物理工作目录；Session 的 cwd 命中它而归入 Project。
- **Workstream** — 一项持续工作，可以拥有多个 WorkspacePath，因此可以跨越多个 Project。
- **Session** — 一个逻辑会话对应一个根源（root source）的当前有效对话：身份是根源真实的
  Resume 身份（`root_agent_session_id`），只有根源产生会话消息（SessionMessage）；
  所属 Project 由根工作目录派生，语义归属由 Owner Workstream 表达，两者互相独立。
  Agent 在根源之外派生的执行（子 Agent / side）在发现时识别并跳过，不构成 Session。

核心 invariant：

```text
A Session is exactly one root source: one read cursor, one fact frontier.
Only that root source produces SessionMessages.

A Session has at most one Owner Workstream.

Session ownership never mutates Workstream workspace paths.

Workstream path mutations never change Session ownership.

Session physical Project membership and semantic Workstream ownership are independent.

NoEnding never deletes Agent-owned session sources.
(Permanent delete = NoEnding-local purge only,
 available only for a trashed Session whose root source is freshly confirmed absent.)
```

## 技术栈

```text
Tauri 2 + React + TypeScript + Rust + SQLite (FTS5)
```

## 当前能力

| 能力 | 说明 |
|---|---|
| Agent Adapter | 8 个 Agent（见[支持的 Agent](#支持的-agent)）的会话源发现、解析与原始记录溯源；启动能力按 CLI 与桌面端分别判断 |
| Platform Abstraction | PlatformPaths（`CODEX_HOME`/`CLAUDE_CONFIG_DIR`/`PI_CODING_AGENT_DIR`/`DSH_HOME` 覆盖）、ExecutableResolver、PlatformLauncher（macOS Terminal / Windows Terminal / PowerShell） |
| Session Ingestion | 会话发现（Root 建会话，ForkRoot 直接指认来源，子 Agent / side 识别后跳过）+ 会话级增量游标；原始 Agent 数据永不修改；原始消息行只追加保留作溯源，当前有效对话由消息投影（`session_message_projection` + 事实世代）维护，源文件压缩/截断/重排会切换世代并原子替换投影。消息/游标单事务原子提交，摄入永不调用 AI |
| Workstream | CRUD / archive；一个 Session 最多只有一个 Owner Workstream（所属任务）；LaunchIntent 匹配只认 Root，fork 不继承 Owner |
| Context (L1/L2) | CoreContextResolver 投影 Goal/Current State/Constraints/Decisions/Open Questions；ContextItem + Revision 历史；Supersede 演进链 |
| Context 更新（显式） | 用户点「更新摘要」（Session）或「更新状态」（Workstream）→ 至多一次 AI 调用 → 校验后一个事务内提交 Session Context 与 Workstream mutations；确定性 Merge Engine（Dedup / Supersede / Resolve / Conflict 保留不自动覆盖），Authority 分级（user_edit 不可被 Agent 静默覆盖）；CLI 失败不回退 heuristic |
| Launcher | New Session / Resume Session：只解析启动事实（Agent / cwd / runtime / Owner / LaunchIntent），不注入 Context、不等待摄入；启动成功后向 IngestionCoordinator 投递后台摄入 |
| Lifecycle | Trash / Restore；永久删除 = 仅清除 NoEnding 本地数据，且只对「已入回收站 + Root 源确认不存在」的会话开放 |
| Search | SQLite FTS5（token 内子串查询由 LIKE 兜底），只索引会话文档与会话消息 |
| Workspace Assistant | Interactive Mode v0（基于 Domain API 检索 + 经用户确认的动作块）；显式 Context 更新直接复用同一 Runtime/CLI 选择 |
| 设置 | Agent 运行配置、会话来源启停与扫描、外观、NoEnding Home 与默认工作目录 |

摄入在应用启动、超过 5 分钟未成功同步后回到前台、启动会话成功或手动扫描时触发。
普通页面打开只读取本地数据库；离开会话来源页面不会中断后台扫描。

## 支持的 Agent

新建会话需要可用的终端 CLI；没有 CLI 的 Agent 仍可摄入历史，并通过已安装的桌面应用继续。
「打开应用」表示不会自动定位到具体会话。

| Agent | 默认数据源 | 新建 | 继续 |
|---|---|---|---|
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | `codex` | CLI 或 Codex 桌面端 |
| Claude Code | `~/.claude/projects/<encoded-cwd>/<session>.jsonl` | `claude` | CLI |
| Pi | `~/.pi/agent/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl` | `pi` | CLI |
| Antigravity | `~/.gemini/antigravity/conversations/<id>.db`；CLI 库在 `~/.gemini/antigravity-cli/` | `agy` | CLI 库用 CLI；IDE 库打开应用 |
| Qoder | `~/.qoder-cn/projects/<encoded-cwd>/<session>.jsonl` | — | 打开 Qoder 应用 |
| WorkBuddy | `~/.workbuddy/projects/<slug>/<sessionId>.jsonl` | — | 桌面端定位到会话 |
| dsh | `~/.dsh/sessions/<encoded-cwd>/<id>/session[.vN].jsonl[.zstd]` | — | 打开 DeepSeek Harness 应用 |
| ZCode | `~/.zcode/cli/db/db.sqlite` | — | 打开 ZCode 应用 |

dsh 同时支持普通 JSONL 与 zstd 压缩源，同一会话选择最高文件代。Antigravity 与 ZCode
只读 SQLite 源库；不同 Agent 的子代理、sidechain 等内部执行不单独摄入。

数据根可用环境变量覆盖：Codex `CODEX_HOME`、Claude Code `CLAUDE_CONFIG_DIR`、Pi `PI_CODING_AGENT_DIR`
（或更具体的 `PI_CODING_AGENT_SESSION_DIR`，直接指定 sessions 目录）、dsh `DSH_HOME`。
也可以在「设置 → 会话来源」中添加自定义来源目录，并控制启停、重新扫描或从头重新入库。

## 运行

需要 Node.js 20、pnpm 9、Rust stable，以及对应平台的 Tauri 2 构建工具。

```bash
pnpm install --frozen-lockfile
pnpm tauri dev     # 开发
pnpm tauri build   # 打包
```

默认数据目录为 NoEnding Home：

- macOS: `~/.noending/`
- Windows: `%USERPROFILE%\.noending\`

数据库保存在 `<NoEnding Home>/data/noending.db`；`runtime/`、`logs/` 存放运行文件和日志，
`workspace/` 是默认工作目录。解析顺序为 `NOENDING_HOME` 环境变量 → 已保存的 Home → 默认目录。

可在「设置 → 数据与高级」修改 Home，重启后搬迁应用数据并生效；旧 `workspace/` 中的用户文件不会移动。

## 数据库兼容策略（无 Migration）

NoEnding 只支持当前数据库格式，不提供任何数据库 migration。数据库身份写在 SQLite
头部（`application_id` + 格式版本号），两者缺一不可：

```text
空文件                            → 按当前格式创建，并写入身份
当前格式 + 结构完整                → 直接使用；不执行任何 schema 修补，但会校验结构
当前格式 + 缺少表/索引             → 拒绝打开（不自动补建）
其他格式 / 其他 SQLite 文件        → 拒绝打开，提示删除数据库后重启
```

「不修补」不等于「不校验」：带着当前身份但结构残缺的数据库会被拒绝，而不是被悄悄补全。

启动时的 `reconcile_runtime_defaults` 只做环境相关的默认值补齐（新增 Agent 的默认
source root），属于当前环境的幂等 reconciliation，不是 migration。

会话数据是可重建的本地投影：Session 来自 Agent 源数据，重建数据库后会从已启用来源重新摄入。
但 **Workstream / Context 等 NoEnding 自有状态无法从 Agent 源恢复**，重建会丢失这部分数据。

破坏性 schema 修改的唯一流程：

```text
1. 修改 src-tauri/src/storage/schema.rs 中的完整当前 DDL
2. 递增 DATABASE_FORMAT_VERSION
3. 同步 schema 契约测试（src-tauri/tests/schema_test.rs）
4. 备份 NoEnding 自有数据后重建本地数据库，重新摄入 Session
```

不要为旧 NoEnding schema 增加兼容分支：`schema.rs` 之外不创建 schema，不使用
`ALTER TABLE` migration，不保留只为读取旧 NoEnding 数据库而存在的 row mapping 或 fallback。

## 代码结构

```text
src/                      React UI
  features/assistant      Workspace Assistant 面板
  features/projects       Project 列表 / 详情 / Resources
  features/workstreams    Workstream 上下文页（L1 分区 + Extended Items + 历史）
  features/sessions       Session 列表 / 会话消息视图（骨架分页 + 每轮中间回复折叠）
  features/launcher       New / Resume Session 启动器
  features/settings       Agent / 来源 / 外观 / 数据设置
  features/sources        来源管理与后台扫描状态
src-tauri/src/
  domain/                 平台无关领域模型（Session / SessionMessage / …）
  adapters/               codex / claude / pi / antigravity / qoder / workbuddy / dsh / zcode Adapter（会话源契约 + 注册表）
  platform/               PlatformPaths / ExecutableResolver / PlatformLauncher
  ingestion/              会话发现 → 解析 → 原子提交（纯事实，不调用 AI）
  sync/                   显式 Context 更新：严格 Extractor + 确定性 MergeEngine + AuthorityPolicy
  context/                显式 Context 服务（updateSession / updateWorkstream）与读取投影
  agent_runtime/          Agent 默认配置与 NoEnding 覆盖
  launcher/               Session Launcher（New / Resume 流程）
  lifecycle/              Trash / Restore / 永久本地删除
  search/                 FTS5 检索
  storage/                SQLite schema + 仓储（sessions / messages / 消息投影 / context / workstream / workspace）
  workspace/              NoEnding Home / Project / WorkspacePath
  commands/               Tauri API
```

## 开发验证

小改动按影响范围验证，完整规则见 [AGENTS.md](AGENTS.md)。文档修改只需 `git diff --check`。
前端行为修改运行类型检查与相关测试；Rust 修改运行格式、编译检查与相关测试。

```bash
# 仓库根目录：前端定向验证
pnpm exec tsc --noEmit
pnpm test src/features/sessions/SessionDetailView.test.tsx

# src-tauri 目录：后端定向验证
cd src-tauri
cargo fmt --check
cargo check
cargo test --test schema_test   # 按改动选择测试套件
cd ..
```

大重构、阶段验收或发布前，在最终代码状态上做一次全量验证：

```bash
cd src-tauri
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cd ..
pnpm test
pnpm build
```

测试覆盖会话摄入与消息投影、Context 更新与事务回滚、搜索、工作区关联及会话阅读交互。
CI 配置覆盖 macOS 与 Windows；远端执行需获授权。
