# NoEnding 技术实现方案 v0.3

## 1. 技术目标

第一版技术实现需要优先保证：

1. 不修改 Codex、Claude Code、Pi 的原始 Session 数据；
2. 能可靠发现和增量读取 Session；
3. 能将不同 Agent 数据标准化；
4. 能维护 Workstream Context；
5. 能进行增量智能 Sync；
6. 能可靠创建 New Session；
7. 能针对已有具体 Session 执行 Resume；
8. New Session 可以不绑定任何 Workstream；
9. Workstream 可以不归属任何 Project；
10. 能保留 Context 来源与历史；
11. Workspace Assistant 与后台 Sync 共享同一智能核心；
12. Windows 与 macOS 从 Phase 1 开始作为一等平台约束。

技术实现必须遵守：

> **核心 Domain 不得假设 Project 一定是代码项目，也不得假设 Session 一定拥有 Repository、Workspace 或 cwd。**

> **Session 是即时交互入口；Workstream 是连续性载体；Project 是可选的长期整理层。**

> **Workstream-centered，不代表 Workstream-required。**

---

# 2. 推荐技术栈

桌面端推荐：

```text
Tauri 2
+
React
+
TypeScript
+
Rust
+
SQLite
```

### React / TypeScript

负责：

- UI；
- Workstream 页面；
- Session 列表与详情；
- Assistant Chat；
- Context Editor；
- Search；
- New Workstream；
- New Session；
- 针对具体 Session 的 Resume 操作；
- Settings。

### Rust

负责：

- 文件系统；
- Process 管理；
- Agent Adapter；
- Platform Abstraction；
- Executable Resolver；
- Session discovery；
- File watcher；
- SQLite；
- New Session Launcher；
- Resume Session；
- URI Scheme；
- 后台任务调度。

### SQLite

负责：

- Domain 数据；
- Session 索引；
- Project；
- Workstream；
- SessionWorkstreamBinding；
- Context Items；
- Source；
- Sync Cursor；
- Context History；
- Project Resolver evidence；
- FTS5 全文搜索。

---

# 3. 总体架构

```text
┌─────────────────────────────┐
│        Desktop UI           │
│ React / TypeScript          │
└──────────────┬──────────────┘
               │
               ▼
┌─────────────────────────────┐
│     Application Domain API  │
└──────────────┬──────────────┘
               │
      ┌────────┼───────────┐
      ▼        ▼           ▼
  Projects  Workstreams  Sessions
      │        │           │
      └────────┼───────────┘
               ▼
┌─────────────────────────────┐
│     Workspace Assistant     │
│ Retrieval                   │
│ Context Extraction          │
│ Workstream Classification   │
│ Project Resolution          │
│ Merge                       │
│ Conflict Detection          │
│ Context Builder             │
└──────────────┬──────────────┘
               │
               ▼
┌─────────────────────────────┐
│        Agent Adapters       │
│ Codex / Claude / Pi         │
└──────────────┬──────────────┘
               │
               ▼
┌─────────────────────────────┐
│     Platform Abstraction    │
│ macOS / Windows             │
└─────────────────────────────┘
```

Agent 差异与平台差异必须保持两个独立维度：

```text
Codex / Claude / Pi
        ×
macOS / Windows
```

禁止演化成 `CodexMacAdapter`、`CodexWindowsAdapter` 这类组合爆炸。

---

# 4. Agent Adapter

统一能力：

```text
detect()
discover_sessions()
read_events()
watch()
get_session_metadata()
get_environment()

build_new_command()
build_resume_command()
inject_context(bundle)
```

`get_environment()` 返回可选信息：

```text
cwd?
repository?
branch?
runtime?
metadata
```

Session 可以完全没有 cwd、Repository 或 Workspace。

Adapter 负责：

- Session 目录规则；
- 文件格式；
- Session ID；
- Agent-specific metadata；
- New / Resume 参数；
- Context 注入；
- Agent 版本兼容。

Adapter 不负责：

- macOS Terminal 怎么打开；
- Windows Terminal / PowerShell 怎么启动；
- PATH 怎么解析；
- 进程怎么托管；
- URI Scheme 怎么注册。

这些属于 Platform Layer。

## 4.1 Platform Abstraction

Windows 与 macOS 从 Phase 1 同时纳入约束。

建议提供：

```text
PlatformServices
├── PlatformPaths
├── ExecutableResolver
├── ProcessLauncher
├── TerminalLauncher
├── FileWatcher
├── UriSchemeManager
├── AppDataProvider
└── ShellEnvironmentResolver
```

典型平台差异：

| 能力 | macOS | Windows |
|---|---|---|
| 用户目录 | `$HOME` | `%USERPROFILE%` |
| App Data | `~/Library/Application Support` | `%APPDATA%` / `%LOCALAPPDATA%` |
| 路径 | Unix path | 盘符 / UNC |
| Terminal | Terminal.app | Windows Terminal / PowerShell / cmd |
| 进程 | PID / signal | process handle / job semantics |
| URI Scheme | App URL handling | Protocol registration |
| 安装 | `.app` / `.dmg` | `.msi` / `.exe` |
| 签名 | Code Signing + Notarization | Windows Code Signing |

### ExecutableResolver

不能假设 `Command::new("claude")` 一定可用。

建议：

```text
AgentInstallation
-----------------
id
agent
platform
executable_path
version
source
last_verified_at
```

统一接口：

```text
resolve_executable(agent)
verify_executable(path)
get_version(path)
```

### ProcessLauncher

Adapter 只生成：

```text
LaunchSpec
----------
executable
args[]
cwd?
env?
terminal_mode
```

平台层执行：

```text
launch_process(spec)
launch_terminal(spec)
open_desktop_app(spec)
open_uri(uri)
```

### FileWatcher

采用：

```text
native watcher
+
periodic reconciliation
```

Watcher 用于实时优化，Reconciliation 负责最终一致性。

### URI Scheme

预留：

```text
noending://session/<id>
noending://workstream/<id>
noending://project/<id>
```

---

# 5. 首版 Agent 能力

## Codex Adapter

需要支持：

- Session discovery；
- thread/session ID；
- cwd / Environment；
- rollout/event 增量读取；
- CLI New；
- CLI Resume；
- 可用时调用 Desktop；
- 版本检测；
- Windows / macOS executable discovery。

## Claude Code Adapter

逻辑上支持：

```text
claude
claude --resume <session>
claude --continue
```

但 Domain 不直接依赖命令字符串。

需要：

- transcript 解析；
- Sync Cursor；
- New / Resume command build；
- executable discovery；
- Windows / macOS 平台启动。

## Pi Adapter

需要：

- Session discovery；
- 增量 JSONL 解析；
- Resume；
- 可选 Environment / cwd 映射；
- Session tree metadata；
- Windows / macOS 平台启动。

所有 Agent 命令与数据格式变化都视为 Adapter 内部问题。

---

# 6. 标准化 Session Event

不同 Agent 的原始格式不能直接进入 Domain 层。

统一转换：

```text
NormalizedSessionEvent
```

建议字段：

```text
id
session_id
sequence
timestamp

type:
  user_message
  assistant_message
  tool_call
  tool_result
  compact
  system
  artifact
  unknown

text
raw_ref
metadata
```

Adapter 必须保留：

```text
raw_ref
```

用于追溯原始 Source。

---

# 7. Session Ingestion

每个 Session 维护：

```text
SessionCursor
```

例如：

```text
last_offset
last_event_id
last_sequence
last_seen_size
mtime
```

Adapter 每次只返回：

```text
events after cursor
```

同时检测：

- append；
- truncate；
- rewrite；
- rotate；
- file replacement。

原始 Agent 数据发生删除或 compact 时：

> 已进入本应用的历史记录不得因此自动删除。

## 7.1 Session 可以独立存在

Session 不要求先绑定 Workstream。

```text
classification_state:
  unassigned
  partially_assigned
  assigned
```

- `unassigned`：没有可靠 Workstream Binding；
- `partially_assigned`：已有部分归类，但仍有重要活动未归类；
- `assigned`：重要活动已合理映射到一个或多个 Workstream。

这些状态由系统维护，不要求用户手工整理。

---

# 8. Sync Scheduler

Sync 由 NoEnding 主动触发。

### Before New Session

如果用户选择了已有 Workstream Context，则优先同步相关 stale Session。

如果：

```text
selected_workstreams = []
```

则允许直接创建 Session。

### Before Resume

Resume 只针对一个已有的具体 Session：

1. 同步该 Session 的新增活动；
2. 解析已有 Workstream Binding；
3. 没有 Binding 也允许 Resume；
4. 后台可继续分类。

### Process Exit

Agent 进程退出后创建 checkpoint Sync Job。

### Manual

用户手动 Sync。

### Application Reconcile

应用启动后检查关闭期间产生的新数据。

后续增加：

```text
long-session periodic sync
```

---

# 9. Sync Job

输入：

```text
session
from_cursor
to_cursor
new_events
candidate_workstreams
candidate_projects?
```

流程：

```text
Read Delta
   ↓
Pre-filter
   ↓
Workspace Assistant
   ↓
Extract Meaningful Changes
   ↓
Classify to Workstreams
   ↓
Discover New Workstream Candidate
   ↓
Build Context Mutations
   ↓
Merge
   ↓
Update Context
   ↓
ProjectResolver
   ↓
Advance Cursor
```

只有完整成功后才推进 Cursor。

一次 Session delta 可以更新一个或多个 Workstream，也可以暂时保持 unassigned。

---

# 10. Workspace Assistant Core

Workspace Assistant 是：

```text
能力 + Tools + Policy
```

包括：

```text
Retriever
ContextExtractor
WorkstreamClassifier
ProjectResolver
ContextMerger
ConflictDetector
ContextBuilder
ApplicationTools
```

## Background Sync

每个 SyncJob 使用独立模型调用。

## Interactive

用户聊天拥有自己的 AssistantSession。

两者共享：

```text
Domain State
Tools
Policy
```

而不是共享 LLM Conversation History。

---

# 11. Assistant Runtime

模型层应抽象：

```text
AssistantRuntime
```

能力要求：

- Structured Output；
- Tool Calling；
- 较强分类能力；
- 较长上下文；
- 可控成本。

具体模型 Provider 不写死在 Domain 层。

允许未来配置：

```text
provider
model
temperature
context budget
```

同步类任务优先使用：

```text
低温度
严格 schema
小上下文增量
```

避免让 Assistant 自由发挥。

---

# 12. Context 数据模型

核心实体：

```text
Project
Workstream

Session
SessionWorkstreamBinding
SessionCursor

ContextItem
ContextItemRevision
SourceReference

ContextConflict
SyncRun

AssistantSession
AssistantMessage

ProjectResource
ProjectAffinityEvidence
```

## 12.1 Project 是可选整理层

Project 不依赖 Repository、Workspace、本地路径或 cwd。

Workstream 允许：

```text
project_id = null
```

Workstream 可以先存在，Project 后识别。

## 12.2 ProjectResolver

Project 不能定义成 `Session cwd`。

Coding 场景强信号：

```text
repository identity
git remote
workspace root
path ancestry
worktree lineage
existing session bindings
shared resources
```

通用场景信号：

```text
Workstream semantics
Context Items
documents
URLs
artifacts
places
explicit user assignment
existing Project membership
```

建议记录：

```text
ProjectAffinityEvidence
-----------------------
id
project_id
subject_type
subject_id

evidence_type:
  repo_identity
  git_remote
  path_ancestry
  workspace_lineage
  semantic_similarity
  shared_resource
  explicit_user_assignment
  prior_binding

score
metadata
created_at
```

高置信度自动归类，低置信度才询问用户。

## 12.3 ProjectResource

```text
ProjectResource
---------------
id
project_id
type
uri?
metadata

type:
  repository
  workspace
  file
  document
  url
  artifact
  external
```

Repository / Workspace 是可选资源，不是 Project 本体。

---

# 13. ContextItem

底层统一使用 ContextItem。

建议：

```text
ContextItem
---------
id
workstream_id
type
current_revision_id
status
created_at
updated_at
```

内容进入：

```text
ContextItemRevision
```

例如：

```text
revision_id
item_id
title
content
metadata
source_type
source_ref
created_at
```

修改 Item 不覆盖历史 Revision。

---

# 14. Item 演进

例如：

```text
Decision 12
Session A → Session B handoff
```

随后：

```text
Decision 34
Workstream Context → Session
```

随后：

```text
Decision 51
Session ↔ Workstream Context Sync
```

内部形成：

```text
12 ← superseded_by ← 34 ← superseded_by ← 51
```

Current View 只读取 51。

History 可以完整还原。

---

# 15. Core Context Resolver

L1 不建议作为五段彼此独立的大字符串直接保存。

推荐：

```text
ContextItem History
      ↓
CoreContextResolver
      ↓
WorkstreamCurrentView
```

生成：

```text
goal
current_state
constraints
decisions
open_questions
```

Goal 可以采用特殊单值规则。

Constraints / Decisions / Questions 来自 active Items。

Current State 可以由结构化 State Item 加上短摘要生成。

---

# 16. Context Merge Engine

Workspace Assistant 产生的是：

```text
ContextMutationProposal[]
```

例如：

```text
ADD item
UPDATE item
SUPERSEDE item
RESOLVE item
MOVE item
CREATE workstream
```

Merge Engine 执行确定性验证。

规则包括：

### Dedup

相同或高度相似 Item 不重复创建。

### Supersede

新信息明确替代旧内容时建立演进关系。

### Resolve

问题、Todo、Issue 已完成时改变状态。

### Move

重新归类到正确 Workstream。

### Conflict

无法确定谁应该覆盖谁时：

```text
ContextConflict
```

保留双方，不自动选择。

---

# 17. Authority

不同来源具有不同语义权重。

建议：

```text
user_explicit
user_edit
system_observed
agent_statement
agent_inferred
```

用户直接编辑 Context 后：

```text
authority = user_edit
```

Workspace Assistant 不得因为后续 Agent 推断而静默覆盖。

它可以创建：

```text
finding
conflict
```

---

# 18. Workstream Current Context

Workstream 不需要把所有历史直接作为 HEAD。

Current Context 是：

> 对当前 active Items 的 projection。

可以缓存：

```text
WorkstreamContextSnapshot
```

但 Snapshot 是派生数据。

源事实仍来自 ContextItem + Revision。

这样 Snapshot 失效时可以重新生成。

---

# 19. SessionWorkstreamBinding

Session 与 Workstream 是多对多关系。

```text
session_id
workstream_id
role

confidence?
source?

last_context_snapshot
last_seen_revision
last_sync_cursor

created_at
last_used_at
```

允许：

```text
Session 0 bindings
Session 1 binding
Session N bindings
```

Workstream 也可以暂时没有任何 Session。

### New Session

如果选择了 Workstream Context，则创建对应 Binding。

如果没有选择任何 Workstream，则不创建 Binding。

### Resume

已有 Binding 时计算：

```text
Current Workstream Context
-
Session Last Seen Context
=
Context Delta
```

无 Binding 也允许 Resume。

---

# 20. Context Builder

输入：

```text
target_agent
launch_type: new | resume
target_session?
selected_workstreams[]
user_intent?
token_budget
```

`launch_type` 是内部概念，不代表 UI 中存在 New / Resume 同级模式。

输出：

```text
SessionContextBundle
```

## New

允许：

```text
selected_workstreams = []
```

如果选择了 Workstream，则优先加入：

```text
Goal
Current State
Constraints
Decisions
Open Questions
High relevance items
Artifacts
```

## Resume

目标一定是具体已有 Session。

优先：

```text
重要 Core Context
+
Changes since last seen
+
new conflicts
+
new relevant artifacts
```

---

# 21. 多 Workstream 聚合

一个 Session 可以加载：

```text
A
B
C
```

Context Builder 不简单拼接三个全文。

需要：

```text
Shared task
Primary Workstream
Related Workstreams
Cross-workstream conflicts
Relevant items
```

目标是形成一份 coherent context，而不是三个摘要连接起来。

---

# 22. Context 注入 Agent

Adapter 根据 Agent 能力决定注入方式。

可能包括：

```text
Initial Prompt
Follow-up Message
Context File
Attachment
File Reference
```

统一抽象：

```text
inject_context(bundle)
```

Domain 层不关心具体 CLI 参数。

---

# 23. New Session Launcher

首页一级入口：

```text
[ New Session ]
```

流程：

```text
Select Agent
   ↓
Optional Select Workstreams
   ↓
Sync selected stale contexts
   ↓
Build Context Bundle
   ↓
AgentAdapter.build_new_command()
   ↓
PlatformLauncher
   ↓
Inject Context
   ↓
Create Session record
   ↓
Create optional Bindings
   ↓
Track process/session id
```

`selected_workstreams = []` 完全合法。

用户不需要为了开始一次 Agent 会话先创建 Workstream。

---

# 24. Resume Session Launcher

Resume 不作为一级入口。

入口只存在于具体 Session：

- Session 列表；
- Session 详情；
- Workstream 的 Related Sessions；
- Search result；
- Workspace Assistant 对具体 Session 的操作。

流程：

```text
Specific Existing Session
   ↓
Sync unsynced messages
   ↓
Resolve current Workstream Bindings
   ↓
Optional background reclassification
   ↓
Build Context Delta
   ↓
AgentAdapter.build_resume_command()
   ↓
PlatformLauncher
   ↓
Inject latest Context
```

无 Workstream Binding 也允许直接 Resume。

---

# 25. Workspace Assistant Tools

Assistant 只通过 Domain API 操作应用。

建议工具：

```text
search_projects
get_project
create_project
assign_workstream_project
resolve_project
get_project_evidence

search_workstreams
get_workstream
create_workstream
merge_workstreams
archive_workstream

search_context_items
get_context_item
get_item_history
update_context_item
move_context_item

search_sessions
get_session
get_session_messages

launch_new_session
resume_session

get_conflicts
resolve_conflict
```

`resume_session` 必须携带具体已有 Session ID。

禁止 Assistant 直接执行 SQL 修改核心状态。

---

# 26. Assistant 写操作

第一版原则：

### 自动允许

- Context Sync；
- 自动分类；
- 创建低风险 inferred Item；
- 更新 State；
- Resolve 明确完成事项；
- 创建自动识别 Workstream。

### 必须产生历史

任何 Context 修改都留下：

```text
source
time
previous revision
new revision
sync run
```

### 用户可随时纠正

人工修改具有更高 Authority。

### Conflict 不自动覆盖

尤其涉及明确用户 Goal / Constraint / Decision。

---

# 27. Search

第一版使用 SQLite FTS5。

索引：

```text
Project
Workstream
ContextItem
Session Message
Assistant Message
```

搜索结果优先显示：

```text
Current Context
```

同时允许：

```text
Search history
Search raw sessions
```

第一版无需向量数据库。

以后可增加 Semantic Search。

---

# 28. Raw Session Retention

建议采用分层保留。

### Source Index

长期保存：

```text
source ref
hash
important excerpt
metadata
```

### Normalized Meaningful Messages

长期或较长时间保存。

### Raw Tool Logs

设置 Retention，例如：

```text
30 天
```

### Agent 原始文件

应用不拥有，不主动删除。

用户可配置：

```text
Keep normalized raw history forever
```

---

# 29. Context Revision 与 Audit

每次 SyncRun 需要记录：

```text
input sessions
cursor range
model
output mutations
merge result
conflicts
duration
error
```

用户不一定看见技术细节，但这对：

- Debug；
- Context 恢复；
- 模型升级；
- Sync 算法调试

非常重要。

---

# 30. UI 推荐结构

主导航建议：

```text
Sidebar
├── Assistant
├── Workstreams
├── Sessions
├── Projects
└── Search
```

一级创建入口只提供：

```text
[ New Workstream ]   [ New Session ]
```

不提供一级：

```text
[ Resume Session ]
```

Resume 属于具体已有 Session。

## Workstream 页面

```text
Goal
Current State
Constraints
Decisions
Open Questions

Extended Items
Recent Activity
Related Sessions
Project?

[ New Session ]
```

从 Workstream 页面启动 New Session 时，默认选择当前 Workstream Context。

## New Session

```text
Agent
○ Codex
○ Claude Code
○ Pi

Contexts
☐ Workstream A
☐ Workstream B
☐ Workstream C

[ Start Session ]
```

不存在：

```text
Mode
○ New
○ Resume
```

Contexts 可以全部不选。

## Session 列表

```text
Claude
NoEnding Branding
2h ago
Workstreams: Branding
[ Resume ]

Codex
Untitled
Yesterday
Workstreams: —
[ Resume ]
```

可支持 Agent / Workstream / Project / assigned 状态过滤。

## Project 页面

```text
Project
├── Active Workstreams
├── Recent Sessions
└── Resources
```

Project 不是进入 NoEnding 的前置步骤。

---

# 31. 项目代码结构建议

```text
src/
  ui/
    features/
      assistant/
      projects/
      workstreams/
      sessions/
      search/
      new-session/
      new-workstream/

src-tauri/
  domain/
    project/
    project_resolver/
    resource/
    workstream/
    context/
    session/

  adapters/
    codex/
    claude/
    pi/

  platform/
    paths/
    executable/
    process/
    terminal/
    watcher/
    uri/
    macos/
    windows/

  ingestion/
  sync/
  assistant/
  context_builder/
  launcher/
  search/
  storage/
```

平台差异不得渗透到 Workstream / Context / Project 等核心 Domain。

---

# 32. MVP 开发顺序

## Phase 1：Foundation + Cross-platform Baseline

- Tauri 2；
- SQLite；
- macOS build；
- Windows build；
- PlatformPaths；
- ExecutableResolver；
- ProcessLauncher；
- FileWatcher abstraction；
- App Data；
- Project；
- ProjectResource；
- Workstream；
- Session；
- SessionWorkstreamBinding；
- Codex / Claude / Pi Adapter skeleton；
- Session discovery；
- Session viewer。

目标：

> 同一代码库从第一阶段起即可在 Windows / macOS 运行。

## Phase 2：Session UX

- New Session；
- Session list；
- Session detail；
- per-session Resume；
- unassigned / partially_assigned Session；
- Agent executable setup；
- 可选 Workstream Context。

## Phase 3：Workstream

- New Workstream；
- Workstream CRUD；
- Session ↔ Workstream Binding；
- ContextItem；
- Item History；
- 手工编辑；
- Workstream 可无 Project。

## Phase 4：Context Sync

- Session Cursor；
- SyncRun；
- Context extraction；
- Workstream classification；
- Workstream candidate discovery；
- Merge；
- Conflict；
- ProjectResolver；
- ProjectAffinityEvidence。

## Phase 5：Context Builder / Launcher Integration

- New Session Context Bundle；
- Resume Context Delta；
- 多 Workstream Context；
- Context injection；
- Windows / macOS Terminal / Process integration。

## Phase 6：Workspace Assistant

- Assistant Chat；
- Retrieval；
- Domain Tools；
- 自然语言创建 New Session；
- 对具体 Session 执行 Resume；
- Workstream / Project / Context 管理。

## Phase 7：Polish

- Background sync；
- Search；
- Context history UI；
- Adapter version compatibility；
- Platform edge cases；
- Packaging；
- macOS signing / notarization；
- Windows signing / installer；
- Performance。

---

# 33. 首版必须验证的核心假设

真正需要 MVP 验证的不是 UI，而是：

### 假设 1

从 Session 增量消息中能够稳定提取少量高价值 Context。

### 假设 2

多 Session 对同一 Workstream 的 Context 可以自动 Merge，而不会快速污染 Context。

### 假设 3

新 Agent 只获得 Current Context，就可以顺畅接手工作。

### 假设 4

Resume Session 获得 Context Delta 比完整重复上下文效果更好。

### 假设 5

用户只需偶尔纠错，而不需要频繁管理 Context。

如果这些假设成立，产品核心价值即成立。

---

# 34. Open Questions

以下问题留到 MVP 实验阶段确定：

- Workspace Assistant 默认模型；
- Sync interval；
- meaningful event 筛选算法；
- Workstream 自动创建阈值；
- Item 自动 merge 阈值；
- ProjectResolver 自动归类阈值；
- Context Bundle token budget；
- Context Conflict UI；
- Raw history retention 默认值；
- macOS 默认 Terminal 集成策略；
- Windows Terminal / PowerShell / cmd 默认策略；
- 多版本 Agent CLI 的优先级策略；
- 是否需要本地 embedding；
- 是否暴露 MCP Server。

这些问题不影响 v0.3 总体架构成立。

---

# 35. 技术实现原则

> Agent Adapter 负责适配 Agent 差异。

> Platform Layer 负责适配 Windows / macOS 差异。

> Domain Model 不依赖任何 Agent 的内部格式或平台路径。

> Session 是数据来源、即时交互入口和执行环境。

> Workstream 是长期 Context 的连续性载体，但不是创建 Session 的前置条件。

> Project 是可选整理层，不依赖 Repository、Workspace 或 cwd。

> 路径 / Repo / Remote 只是 Coding 场景中的 Project 归类证据，不是 Project 本身。

> Session 与 Workstream 独立存在，通过 SessionWorkstreamBinding 建立可选、多对多关系。

> New Workstream 与 New Session 是两个一级创建入口。

> Resume 只能针对具体已有 Session 执行，不作为一级创建入口。

> Workspace Assistant 是唯一智能转换层。

> Sync 使用增量数据。

> Merge 必须可追溯。

> Current Context 简洁，History 完整。

> 自动化优先，人工纠错兜底。

> 所有智能行为必须建立在稳定 Domain API 之上。
