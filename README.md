# NoEnding

> **Conversations end. Context doesn't.**
> 对话会结束，上下文不会。

NoEnding 是一个以 **Workstream Context** 为核心的本地多 Agent 工作空间（Local Agent Workspace）。
它不管理聊天记录，而是把不同 Agent（Codex / Claude Code / Pi）的 Session 组织进持续演进的
Workstream 语义层：Session 会结束，Agent 会切换，Context 持续存在。

设计文档见 `docs/`：

- `Local Agent Workspace 产品与领域设计方案 v0.2.md` — 领域模型与产品定义
- `Local Agent Workspace 技术实现方案 v0.2.md` — 技术架构
- `Local Agent Workspace 技术实现方案 v0.3 增补 - Platform Abstraction.md` — Windows/macOS 平台抽象层
- `NoEnding 品牌设计规范 v1.0.md` — 品牌与视觉

## 领域模型

四类实体的职责固定：

- **Project** — 一组属于同一物理工作空间或 Git family 的 WorkspacePath。不是分类容器。
- **Workstream** — 一项持续工作，可以拥有多个 WorkspacePath，因此可以跨越多个 Project。
- **Session** — 一个逻辑会话：一次用户可感知、可 Resume 的主会话，加上它派生出的内部执行
  成员（子 Agent / side）。身份是 Root 成员真实的 Resume 身份（`root_agent_session_id`）。
  只有 Root 会话产生会话消息（SessionMessage）；所属 Project 由 Root 工作目录派生，
  语义归属由 Owner Workstream 表达，两者互相独立。
- **SessionMember** — 会话内部的执行单元（root / child / side），是执行信息与统计的载体，
  不是另一个 Session。

核心 invariant：

```text
A Logical Session has exactly one Root Member.
Only the Root Member produces SessionMessages.

A Session has at most one Owner Workstream.

Session ownership never mutates Workstream workspace paths.

Workstream path mutations never change Session ownership.

Session physical Project membership and semantic Workstream ownership are independent.

NoEnding never deletes Agent-owned session sources.
(Permanent delete = NoEnding-local purge only,
 available only for a trashed Session whose Root source is freshly confirmed absent.)
```

## 技术栈

```text
Tauri 2 + React + TypeScript + Rust + SQLite (FTS5)
```

## 已实现（对应技术方案 MVP Phase 1–4）

| 能力 | 说明 |
|---|---|
| Agent Adapter | Codex / Claude Code / Qoder / DSH / ZCode / Pi / AutoClaw / WorkBuddy 的 member discovery、增量解析、raw_ref 溯源；Root 之外的执行（child/side）只进拓扑与统计 |
| Platform Abstraction | PlatformPaths（`CODEX_HOME`/`CLAUDE_CONFIG_DIR`/`PI_HOME`/`DSH_HOME` 覆盖）、ExecutableResolver、PlatformLauncher（macOS Terminal / Windows Terminal / PowerShell） |
| Session Ingestion | Member 图解析（Root/ForkRoot 建会话，child/side 沿父链归属，悬空源进摄入诊断）+ Member 级增量游标（append-only）；原始 Agent 数据永不修改，已摄入会话不随源文件压缩/截断消失；消息/统计/游标单事务原子提交 |
| Workstream | CRUD / archive；一个 Session 最多只有一个 Owner Workstream（所属任务）；LaunchIntent 匹配只认 Root，fork 不继承 Owner |
| Context (L1/L2/L3) | CoreContextResolver 投影 Goal/Current State/Constraints/Decisions/Open Questions；ContextItem + Revision 历史；Supersede 演进链 |
| Sync Engine | SyncJob（会话消息 → extract → merge → Context frontier），无 kind/长度二次过滤；Context 路由只认 Session 的 Owner Workstream（无 Owner 时 frontier 冻结，归属后重放）；确定性 Merge Engine（Dedup / Supersede / Resolve / Conflict 保留不自动覆盖），Authority 分级（user_edit 不可被 Agent 静默覆盖） |
| Context Builder | New / Resume 两种模式的最小充分上下文 bundle + token budget |
| Launcher | New Session / Resume Session（Resume 前 fresh 检查 Root 源可用性，再同步成员并注入 bundle 启动 CLI） |
| Lifecycle | Trash / Restore；永久删除 = 仅清除 NoEnding 本地数据，且只对「已入回收站 + Root 源确认不存在」的会话开放 |
| Search | SQLite FTS5（token 内子串查询由 LIKE 兜底），只索引会话文档与 Root 会话消息 |
| Workspace Assistant | Interactive Mode v0（基于 Domain API 检索）；Background Mode 即 Sync Engine，LLM Runtime 通过 trait 预留接入 |

## 运行

```bash
pnpm install
pnpm tauri dev     # 开发
pnpm tauri build   # 打包
```

数据库位置（平台应用数据目录）：

- macOS: `~/Library/Application Support/app.noending.desktop/noending.db`
- Windows: `%APPDATA%\app.noending.desktop\noending.db`

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
source root、缺失的默认设置），属于当前环境的幂等 reconciliation，不是 migration。

SQLite 数据库是可重建的本地投影：Session 来自 Agent 源数据，删除数据库后重启会重新摄入。
但 **Workstream / Context 等 NoEnding 自有状态无法从 Agent 源恢复**，重建会丢失这部分数据。

破坏性 schema 修改的唯一流程：

```text
1. 修改 src-tauri/src/storage/schema.rs 中的完整当前 DDL
2. 递增 DATABASE_FORMAT_VERSION
3. 同步 schema 契约测试（src-tauri/tests/schema_test.rs）
4. 删除本地数据库并重启，重新摄入 Session
```

不要为旧 NoEnding schema 增加兼容分支：`schema.rs` 之外不创建 schema，不使用
`ALTER TABLE` migration，不保留只为读取旧 NoEnding 数据库而存在的 row mapping 或 fallback。

## 代码结构

```text
src/                      React UI
  features/assistant      Workspace Assistant 面板
  features/projects       Project 列表 / 详情 / Resources
  features/workstreams    Workstream 上下文页（L1 分区 + Extended Items + 历史）
  features/sessions       Session 列表 / 会话消息视图（仅 user/assistant）
  features/launcher       New / Resume Session 启动器（含 bundle 预览）
src-tauri/
  domain/                 平台无关领域模型（Session / SessionMember / SessionMessage / …）
  adapters/               codex / claude / qoder / dsh / zcode / pi / autoclaw / workbuddy Adapter（member 契约 + 注册表）
  platform/               PlatformPaths / ExecutableResolver / PlatformLauncher
  ingestion/              Member 发现 → 逻辑图解析 → 成员原子提交 → Context sync
  sync/                   SyncJob + 启发式 Extractor + 确定性 MergeEngine
  context/                CoreContextResolver + ContextBuilder
  launcher/               Session Launcher（New / Resume 流程）
  lifecycle/              Trash / Restore / 永久本地删除
  search/                 FTS5 检索
  storage/                SQLite schema + 仓储（sessions / members / messages / cursors / stats / diagnostics）
```

## 测试

```bash
cd src-tauri && cargo test
```

覆盖：PlatformPaths 跨平台路径解码、Sync 引擎端到端（提取→分类→合并→去重→游标）、
Context Bundle 核心分区、FTS 搜索。
