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

## 技术栈

```text
Tauri 2 + React + TypeScript + Rust + SQLite (FTS5)
```

## 已实现（对应技术方案 MVP Phase 1–4）

| 能力 | 说明 |
|---|---|
| Agent Adapter | Codex / Claude Code / Pi 的 session discovery、JSONL 增量解析、raw_ref 溯源 |
| Platform Abstraction | PlatformPaths（`CODEX_HOME`/`CLAUDE_CONFIG_DIR`/`PI_HOME` 覆盖）、ExecutableResolver、PlatformLauncher（macOS Terminal / Windows Terminal / PowerShell） |
| Session Ingestion | 增量游标（append-only），原始 Agent 文件永不修改，已摄入历史不随源文件删除 |
| Workstream | CRUD / merge / archive；Session ↔ Workstream 多对多 Binding |
| Context (L1/L2/L3) | CoreContextResolver 投影 Goal/Current State/Constraints/Decisions/Open Questions；ContextItem + Revision 历史；Supersede 演进链 |
| Sync Engine | SyncJob（delta → pre-filter → extract → classify → merge → cursor），确定性 Merge Engine（Dedup / Supersede / Resolve / Conflict 保留不自动覆盖），Authority 分级（user_edit 不可被 Agent 静默覆盖） |
| Context Builder | New / Resume 两种模式的最小充分上下文 bundle + token budget |
| Launcher | New Session / Resume Session（先同步 stale session，再注入 bundle 启动 CLI） |
| Search | SQLite FTS5（FTS 不可用时 LIKE 兜底），优先 Current Context |
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

## 代码结构

```text
src/                      React UI
  features/assistant      Workspace Assistant 面板
  features/projects       Project 列表 / 详情 / Resources
  features/workstreams    Workstream 上下文页（L1 分区 + Extended Items + 历史）
  features/sessions       Session 列表 / 标准化消息视图
  features/launcher       New / Resume Session 启动器（含 bundle 预览）
src-tauri/
  domain/                 平台无关领域模型
  adapters/               codex / claude / pi Adapter（trait + 注册表）
  platform/               PlatformPaths / ExecutableResolver / PlatformLauncher
  ingestion/              发现 → 入库 → 游标
  sync/                   SyncJob + 启发式 Extractor + 确定性 MergeEngine
  context/                CoreContextResolver + ContextBuilder
  launcher/               Session Launcher（New / Resume 流程）
  search/                 FTS5 检索
  storage/                SQLite schema + 仓储
```

## 测试

```bash
cd src-tauri && cargo test
```

覆盖：PlatformPaths 跨平台路径解码、Sync 引擎端到端（提取→分类→合并→去重→游标）、
Context Bundle 核心分区、FTS 搜索。
