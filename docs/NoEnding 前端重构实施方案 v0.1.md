# NoEnding 前端重构实施方案 v0.1

## 1. 改造目标

本次改造不改变 NoEnding 的核心 Domain Model，重点重构前端信息架构与交互路径。

目标是把当前以：

```text
Projects
Assistant
All Sessions
Sources
Recent Workstreams
```

为主的界面，调整为：

```text
Home
│
├── Continue
│
└── Recent Sessions

Workstreams
└── Workstream Detail

Sessions
└── Session Detail

Assistant

Projects
└── Project Detail

Settings
```

核心原则：

> 打开应用后，用户首先看到最近正在推进的 Workstream，并可以一键 New / Resume。

---

# 2. 当前前端结构

当前主要结构：

```text
src/
├── App.tsx
├── LazyRouter.tsx
├── Router.tsx
├── api.ts
├── types.ts
├── styles.css
│
├── components/
│   ├── SidebarLogo
│   └── CommandPalette
│
└── features/
    ├── assistant/
    ├── launcher/
    ├── projects/
    ├── search/
    ├── sessions/
    ├── sources/
    └── workstreams/
```

目前：

```text
route.view === "home"
```

实际对应：

```text
ProjectsView
```

这是本次首先需要解除的耦合。

---

# 3. 推荐目标结构

调整为：

```text
src/
│
├── app/
│   ├── AppShell.tsx
│   ├── Router.tsx
│   └── routes.ts
│
├── layout/
│   ├── Sidebar.tsx
│   ├── SidebarSection.tsx
│   ├── PageHeader.tsx
│   └── PageContent.tsx
│
├── components/
│   ├── AgentIcon.tsx
│   ├── Button.tsx
│   ├── IconButton.tsx
│   ├── Badge.tsx
│   ├── Modal.tsx
│   ├── EmptyState.tsx
│   ├── SearchInput.tsx
│   └── CommandPalette.tsx
│
├── features/
│   │
│   ├── home/
│   │   ├── HomeView.tsx
│   │   ├── ContinueSection.tsx
│   │   └── RecentSessions.tsx
│   │
│   ├── workstreams/
│   │   ├── WorkstreamsView.tsx
│   │   ├── WorkstreamDetailView.tsx
│   │   ├── WorkstreamCard.tsx
│   │   ├── WorkstreamContext.tsx
│   │   ├── WorkstreamSessions.tsx
│   │   └── WorkstreamActivity.tsx
│   │
│   ├── sessions/
│   │   ├── SessionsView.tsx
│   │   ├── SessionDetailView.tsx
│   │   ├── SessionTable.tsx
│   │   └── SessionMessage.tsx
│   │
│   ├── assistant/
│   │   ├── AssistantView.tsx
│   │   ├── AssistantMessage.tsx
│   │   ├── AssistantAction.tsx
│   │   └── ScopeSelector.tsx
│   │
│   ├── projects/
│   │   ├── ProjectsView.tsx
│   │   ├── ProjectDetail.tsx
│   │   ├── ProjectCard.tsx
│   │   └── ProjectResources.tsx
│   │
│   ├── settings/
│   │   ├── SettingsView.tsx
│   │   ├── GeneralSettings.tsx
│   │   ├── AgentsSettings.tsx
│   │   ├── SourcesSettings.tsx
│   │   ├── ContextSyncSettings.tsx
│   │   ├── AppearanceSettings.tsx
│   │   └── AdvancedSettings.tsx
│   │
│   └── search/
│       └── SearchView.tsx
│
├── api.ts
├── types.ts
└── styles/
    ├── tokens.css
    ├── global.css
    ├── layout.css
    └── components.css
```

第一版不要求一次完成整个目录迁移。

可以渐进式重构。

---

# 4. 第一阶段：AppShell

首先从 `App.tsx` 中拆出整体应用 Shell。

当前：

```text
App.tsx

state
sidebar
router
command palette
sync listeners
```

逐渐调整为：

```text
App.tsx

<AppShell>
    <Router />
</AppShell>
```

推荐：

```text
App.tsx
↓
AppShell.tsx
├── Sidebar
├── Main
│   └── Router
└── CommandPalette
```

---

# 5. Route 重构

当前 Route 增加：

```ts
type Route =
  | { view: "home" }
  | { view: "workstreams" }
  | { view: "workstream"; workstreamId: string }
  | { view: "sessions" }
  | { view: "session"; sessionId: string }
  | { view: "assistant"; scope?: AssistantScope }
  | { view: "projects" }
  | { view: "project"; projectId: string }
  | { view: "settings"; section?: SettingsSection }
  | { view: "search"; query: string };
```

移除：

```text
sources
```

作为独立一级 Route。

迁移到：

```text
settings / session-sources
```

---

# 6. Home 不再指向 ProjectsView

当前：

```tsx
case "home":
  return <ProjectsView ... />
```

改为：

```tsx
case "home":
  return <HomeView ... />
```

Projects 使用：

```text
view: "projects"
```

独立访问。

---

# 7. Sidebar 重构

从当前 `App.tsx` 中的 `Sidebar()` 抽离：

```text
src/layout/Sidebar.tsx
```

Sidebar 自己管理：

```text
Projects
Recent Workstreams
Current navigation state
```

AppShell 只传：

```text
route
navigate
```

推荐接口：

```tsx
<Sidebar
  route={route}
  navigate={navigate}
/>
```

---

# 8. Sidebar 数据

Sidebar 启动加载：

```text
listWorkstreams()
listProjects()
```

Recent Workstreams：

```text
filter:
lifecycle = open
visibility != archived

sort:
lastActivity DESC

limit:
5 ~ 7
```

目前 Workstream 本身如果没有 `last_activity` 字段，可以第一版先使用：

```text
updated_at
```

作为近似值。

后续再增加真正聚合后的 `last_activity_at`。

---

# 9. Sidebar 导航

最终：

```text
Brand
→ Home

Search
→ CommandPalette

Workstreams
→ WorkstreamsView

Sessions
→ SessionsView

Assistant
→ AssistantView

Recent Workstreams
→ WorkstreamDetail

Projects
→ ProjectDetail

Settings
→ Settings
```

---

# 10. Sidebar 删除一级 Sources

删除：

```text
会话数据源
```

一级导航。

原：

```text
features/sources/SourcesView.tsx
```

暂时不要删除。

先迁移为：

```text
features/settings/SourcesSettings.tsx
```

第一阶段甚至可以直接复用：

```tsx
<SourcesView />
```

嵌入 Settings。

等 UI 稳定后再重构命名。

---

# 11. HomeView

新增：

```text
src/features/home/HomeView.tsx
```

结构：

```text
HomeView
│
├── Page Header
├── ContinueSection
└── RecentSessions
```

第一版不需要复杂聚合 API。

可以：

```text
listWorkstreams()
listSessions()
```

前端排序截取。

---

# 12. ContinueSection

展示：

```text
4 个最近 Workstreams
```

最多：

```text
6
```

两列：

```text
grid-template-columns:
repeat(2, minmax(0, 1fr))
```

窄窗口：

```text
1 column
```

右上：

```text
Workstreams →
```

进入完整页面。

---

# 13. WorkstreamCard

这是本轮改造最重要的共享组件。

新增：

```text
features/workstreams/WorkstreamCard.tsx
```

接口建议：

```ts
interface WorkstreamCardProps {
  workstream: Workstream;
  project?: Project;
  currentState?: string;
  latestSession?: Session;
  sessionCount: number;

  mode?: "compact" | "full";

  onOpen(): void;
  onNewSession(): void;
  onResumeSession?(): void;
}
```

---

# 14. WorkstreamCard 状态

### 有 Session

显示：

```text
Title
Project?
Current State
Last activity
Session Count

[Default Agent New]
[Latest Agent Resume]
```

### 无 Session

显示：

```text
Title
Project?
Current State / Description

No sessions yet

[Default Agent Start]
```

---

# 15. AgentIcon

新增统一组件：

```text
components/AgentIcon.tsx
```

接口：

```ts
<AgentIcon
  agent="codex"
  size={16}
/>
```

支持：

```text
codex
claude_code
pi
```

不要在每个页面分别写 Agent badge。

---

# 16. Default Agent

当前后端已有：

```text
get_agent_status
assistant_config
```

但需要确认是否已经有真正的：

```text
default_agent
```

用户级设置。

如果当前不存在，第一版前端可以先：

```text
General Settings
Default Agent
```

然后新增一个非常小的 backend setting。

这是新首页上线之前唯一比较值得同步改 Rust 的配置项。

因为：

```text
New
```

必须知道默认 Agent。

---

# 17. New Session 调用

WorkstreamCard：

```text
onNewSession
```

直接：

```text
getDefaultAgent()
↓
launch_new_session
```

参数中：

```text
selected_workstream_ids = [workstream.id]
```

不展示 Agent Picker。

---

# 18. Resume 调用

首先获取 Workstream 最近 Session。

规则：

```text
bindings
↓
Session.last_activity_at DESC
↓
first
```

点击：

```text
launch_resume_session
```

直接恢复。

如果找不到 Session：

```text
Resume
```

不显示。

---

# 19. WorkstreamsView

目前如果没有独立完整页面，需要新增：

```text
features/workstreams/WorkstreamsView.tsx
```

与 Detail 分开。

结构：

```text
PageHeader
├ Title
└ + New Workstream

Search

Filters
├ All
├ Active
└ Archived

Sort

CardGrid
```

---

# 20. Workstreams 数据状态

第一版组件内 state：

```ts
const [query, setQuery] = useState("");
const [filter, setFilter] = useState<"all" | "active" | "archived">("active");
const [sort, setSort] = useState<Sort>("recent");
```

无需立即引入：

```text
Redux
Zustand
React Query
```

当前应用规模完全可以继续使用：

```text
React state + API calls
```

不要为了 UI 重构顺便增加状态管理框架。

---

# 21. Workstream Detail 重构

当前：

```text
features/workstreams/WorkstreamView
```

建议逐步改名：

```text
WorkstreamDetailView
```

这是核心页面。

---

# 22. Workstream Detail 布局

结构：

```text
WorkstreamDetailView

Header
├ breadcrumb
├ title
├ project
├ lifecycle
├ Ask Assistant
├ New
└ Resume

Body
├ Main
│   ├ Current State
│   ├ Goal
│   ├ Open Questions
│   ├ Decisions
│   ├ Constraints
│   └ Extended Context
│
└ Right Rail
    ├ Sessions
    └ Activity
```

CSS：

```text
display: grid

grid-template-columns:
minmax(0, 1fr) 320px
```

窄窗口：

```text
1 column
```

---

# 23. Core Context 展示

当前已经有：

```text
get_workstream_context
```

可以继续使用。

前端把：

```text
goal
current_state
constraint
decision
open_question
```

按类型分组。

推荐顺序：

```text
current_state
goal
open_question
decision
constraint
```

不是按照数据库顺序渲染。

---

# 24. Core Context 编辑

当前已经有：

```text
edit_context_item
set_item_status
```

第一版直接复用。

交互：

```text
Section
→ Edit
→ inline textarea
→ Save
```

避免跳转 Modal。

如果一个 Core Section 有多个 item：

第一版可以继续按现有 Resolver 的结果展示。

---

# 25. Extended Context

不要一次显示所有。

策略：

```text
status = active
↓
importance / updated_at
↓
top N
```

如果当前 Domain 没有 importance：

```text
updated_at DESC
```

即可。

展示：

```text
Todo
Finding
Risk
Reference
...
```

最多：

```text
6 ~ 10
```

然后：

```text
View all
```

---

# 26. Context History

继续复用：

```text
get_item_history
```

新增：

```text
ContextHistoryPanel
```

第一版可以用 Modal。

不必马上引入 Drawer 系统。

---

# 27. Workstream Sessions

新增：

```text
WorkstreamSessions.tsx
```

数据可以来自：

```text
list_sessions
```

后前端过滤 binding。

但更推荐后续新增：

```text
list_workstream_sessions(workstream_id)
```

避免全量拉取。

第一版如果 Session 不多，可以先不动后端。

---

# 28. Specific Resume

Workstream Detail 中每个 Session：

```text
[ Resume ]
```

直接：

```text
launch_resume_session(session.id)
```

这与 Header Resume 区分：

```text
Header Resume
→ latest session

Session Row Resume
→ selected session
```

---

# 29. Workstream Activity

第一版可以先做“伪聚合”。

数据来源：

```text
context revisions
sessions
sync runs
```

如果实现成本高：

第一版只展示：

```text
recent ContextItem revisions
```

标题仍可叫：

```text
Recent Changes
```

不要为了 Activity UI 立即设计一个完整 event timeline backend。

后续再升级。

---

# 30. SessionsView

现有：

```text
features/sessions/SessionsView
```

保留，但重构布局。

从目前偏页面组件改成：

```text
PageHeader
Search
Filters
SessionTable
```

---

# 31. SessionTable

新增：

```text
SessionTable.tsx
```

字段：

```text
Agent
Session
Workstream
Project
Last Activity
Actions
```

避免 Card。

Sessions 数量较多，Table 更适合。

---

# 32. Session 搜索

第一版前端 filter：

```text
title
agent
cwd
workstream title
project
```

后续 Session 超过一定数量，再转后端查询。

---

# 33. Global New Session

Sessions Header：

```text
+ New Session
```

第一版点击可以直接打开一个小 Modal：

```text
New Session

Workstream
None

Agent
Claude Code
Default

[ Start ]
```

这里允许 Workstream 为空。

如果用户不操作任何字段，直接：

```text
Default Agent
+
No Workstream
```

启动。

---

# 34. Session Detail

现有：

```text
SessionDetailView
```

继续复用。

重点改视觉层级。

Header：

```text
Session title
Agent
Workstream
Project
Last active
Resume
```

正文：

```text
normalized messages
```

不要把 Session Detail 做成 Context 页。

---

# 35. SessionMessage

新增：

```text
SessionMessage.tsx
```

支持：

```text
user_message
assistant_message
tool_call
tool_result
system
artifact
unknown
```

统一三种 Agent 的消息视觉。

---

# 36. Session Binding

当前已有：

```text
bind_session_workstream
assign_session_project
```

第一版 Session Detail 提供：

```text
Edit Workstreams
```

Modal 即可。

不要把分类与 binding 操作塞进 Sessions Table。

---

# 37. AssistantView

现有 Assistant 可以保留核心逻辑。

重点改变定位和 UI。

新增顶部：

```text
ScopeSelector
```

---

# 38. AssistantScope

前端类型：

```ts
type AssistantScope =
  | { type: "workspace" }
  | { type: "project"; id: string }
  | { type: "workstream"; id: string }
  | { type: "session"; id: string };
```

如果后端第一版还不支持 scope：

可以先只在 Prompt 构建时注入：

```text
scope metadata
```

不要为了 UI 先设计复杂新协议。

---

# 39. Ask Assistant

在：

```text
Workstream Detail
Session Detail
Project Detail
```

放：

```text
Ask Assistant
```

点击：

```ts
navigate({
  view: "assistant",
  scope: {
    type: "workstream",
    id: workstreamId
  }
})
```

---

# 40. Assistant Action Card

当前已经有：

```text
assistant_execute_action
```

所以非常适合抽：

```text
AssistantAction.tsx
```

所有会改变 Domain 的操作统一展示为：

```text
Action Summary

Target
Effect

Cancel
Confirm
```

而不是直接在聊天文本里嵌 Button。

---

# 41. ProjectsView

当前 `ProjectsView` 从 Home 退回真正的：

```text
Projects
```

页面。

Route：

```text
view: "projects"
```

新增：

```text
+ New Project
```

---

# 42. Projects UI

第一版使用：

```text
single column list
```

或者：

```text
2-column light card
```

不需要复杂 Project Dashboard。

Project Card：

```text
Name
Description
Workstream Count
Session Count
Last Activity
```

---

# 43. ProjectDetail

现有组件继续复用。

重新组织为：

```text
Header

Description

Resources

Workstreams

Recent Sessions
```

Workstreams 应该是最主要 section。

---

# 44. Project Resources

当前已有：

```text
add_project_resource
list_project_resources
remove_project_resource
```

第一版可以直接做：

```text
Resource Row

icon
kind
uri
open
remove
```

不要做文件浏览器。

---

# 45. SettingsView

新增：

```text
features/settings/
```

并把一些原本散落在页面的配置集中进去。

结构：

```text
SettingsView
│
├ General
├ Agents
├ Session Sources
├ Context & Sync
├ Appearance
└ Data & Advanced
```

---

# 46. Settings Layout

使用：

```text
Settings sidebar
+
Settings content
```

注意这是：

```text
Main Content 内部二级导航
```

不是应用全局 Sidebar。

例如：

```text
Global Sidebar
│
└ Settings
    │
    ├ General
    ├ Agents
    └ Sources
```

---

# 47. General Settings

最优先实现：

```text
Default Agent
```

其次：

```text
Startup Page = Home
```

但实际上第一版可以直接固定 Home。

无需真的提供可配置 Startup Page。

---

# 48. Agents Settings

复用：

```text
get_agent_status
```

展示：

```text
Agent
Detected / unavailable
Executable
Version
```

如果后端没有手动指定 executable 的 API：

第一版先只读。

---

# 49. Sources Settings

把现有：

```text
SourcesView
```

迁过来。

路径：

```text
Settings
→ Session Sources
```

功能全部保持：

```text
Add
Enable
Disable
Re-ingest
Remove
```

只是改变页面位置和视觉。

---

# 50. Context & Sync Settings

第一版可以只做：

```text
Automatic sync
Background reconcile
```

如果这些目前实际上固定开启，也可以暂时仅展示说明，不提供 toggle。

不要为“设置页面完整”而创造无意义配置。

---

# 51. Appearance Settings

第一版只需要：

```text
Theme
System
Light
Dark
```

如果当前还没有 theme framework：

可以推迟到整个 UI 重构结束后。

---

# 52. Data & Advanced

可以最后实现。

第一版只提供：

```text
Database Path
Open Data Directory
```

以及：

```text
Rebuild Search Index
```

如果后端已有对应能力。

---

# 53. CommandPalette 改造

现有：

```text
CommandPalette
```

保留。

新增固定 commands：

```text
Go to Home
Go to Workstreams
Go to Sessions
Go to Assistant
Go to Projects
Go to Settings

New Workstream
New Session
```

同时搜索：

```text
Workstreams
Sessions
Projects
```

---

# 54. SearchView

保留现有全局搜索。

但不要让：

```text
SearchView
```

承担 Command Palette 的实时搜索 UI。

区分：

```text
Command Palette
→ 快速导航 / command

Search Results
→ 完整搜索结果
```

---

# 55. styles.css 重构

当前单文件：

```text
styles.css
```

已经较大。

这次重构适合顺便拆分。

第一步：

```text
styles/
tokens.css
global.css
layout.css
```

组件自己的复杂样式可以继续使用 class。

不建议此时引入：

```text
Tailwind
CSS-in-JS
styled-components
```

因为当前没有必要。

---

# 56. Design Tokens

先建立：

```css
:root {
  --bg-app: ...;
  --bg-sidebar: ...;
  --bg-panel: ...;
  --bg-hover: ...;
  --bg-active: ...;

  --text-primary: ...;
  --text-secondary: ...;
  --text-muted: ...;

  --border-subtle: ...;
  --border-default: ...;

  --accent: ...;
  --danger: ...;

  --radius-sm: 6px;
  --radius-md: 8px;
  --radius-lg: 12px;

  --sidebar-width: 248px;
}
```

以后组件禁止大量直接写颜色。

---

# 57. Dark Mode

从 token 开始设计：

```text
[data-theme="dark"]
```

覆盖 variables。

不要做两套 component CSS。

---

# 58. Button Component

目前各页面如果大量直接：

```tsx
<button className="...">
```

建议逐渐统一：

```tsx
<Button
  variant="primary | secondary | ghost"
  size="sm | md"
>
```

第一版只抽最常使用的 Button。

不要一开始建设大型 Design System。

---

# 59. Badge

统一：

```text
Active
Archived
Agent
Project
```

但不要所有 metadata 都变 Badge。

原则：

```text
Badge = state / category
Text = metadata
```

---

# 60. 数据刷新

当前 App 有：

```text
sync-completed
reconcile-completed
```

事件。

建议集中到 AppShell。

收到事件后：

```text
window.dispatchEvent(...)
```

可以继续暂时使用。

但应该逐步变成：

```text
RefreshContext
```

或一个非常小的：

```text
revision counter
```

---

# 61. 暂不引入 React Query

虽然这些页面已经开始涉及：

```text
cache
invalidate
background refresh
```

但第一版不建议顺便引入 React Query。

原因：

```text
改动面过大
Tauri API 非 REST
当前规模仍小
```

先保持简单。

如果后续页面明显出现大量：

```text
loading / cache / invalidation duplication
```

再引入。

---

# 62. 推荐新增前端 View Models

不要让 UI 大量直接拼 Domain Model。

例如：

```ts
interface WorkstreamCardModel {
  workstream: Workstream;
  project?: Project;
  currentState?: string;
  sessionCount: number;
  latestSession?: Session;
  lastActivityAt?: string;
}
```

以后如果 backend 提供 aggregate API：

前端组件不需要重写。

---

# 63. 推荐后端聚合 API

不是本轮必须，但后续很值得增加。

### Home

```text
get_home_overview
```

返回：

```text
recent_workstreams
recent_sessions
default_agent
```

### Workstream

```text
get_workstream_detail
```

返回：

```text
workstream
project
core_context
extended_context
sessions
activity
```

### Project

```text
get_project_detail
```

返回：

```text
project
resources
workstreams
recent_sessions
```

这三个 API 可以明显减少前端 orchestration。

---

# 64. 第一阶段不要动的后端能力

以下继续复用：

```text
create_workstream
update_workstream
archive_workstream
merge_workstreams

get_workstream_context

list_sessions
get_session_detail

launch_new_session
launch_resume_session

preview_context_bundle

assistant_send
assistant_execute_action

list_projects
list_project_resources

get_agent_status
```

前端重构不要同步演变成 Domain 重构。

---

# 65. 实施阶段划分

## Phase 1 — App Shell

完成：

```text
AppShell
Sidebar
Routes
Design Tokens
Page Layout
```

并使现有页面仍然能正常访问。

这是整个重构基础。

---

# 66. Phase 2 — Home

完成：

```text
HomeView
ContinueSection
WorkstreamCard
AgentIcon

New
Resume
```

这一步完成后，应用启动体验已经发生根本变化。

---

# 67. Phase 3 — Workstreams

完成：

```text
WorkstreamsView
Search
Filter
Sort

New Workstream
```

并使：

```text
Home → Workstreams
Sidebar → Workstreams
```

路径全部成立。

---

# 68. Phase 4 — Workstream Detail

这是本轮工作量最大的页面。

完成：

```text
Header
Core Context
Inline Edit
Extended Context
Sessions Rail
Specific Resume
Context History
Ask Assistant
```

Activity 可以作为：

```text
P1
```

---

# 69. Phase 5 — Sessions

完成：

```text
SessionTable
Filters
New Session
Session Detail redesign
Binding edit
```

---

# 70. Phase 6 — Settings

先完成：

```text
General
Default Agent

Agents

Session Sources
```

把 Sources 从一级导航彻底移除。

---

# 71. Phase 7 — Assistant

重构：

```text
Scope
Empty State
Conversation UI
Action Cards
```

不急着增加新的智能能力。

重点是产品定位与交互表达。

---

# 72. Phase 8 — Projects

完成：

```text
ProjectsView
Project Detail redesign
Resources
Workstreams
Recent Sessions
```

Project 视觉权重保持次于 Workstream。

---

# 73. Phase 9 — Polish

最后统一：

```text
Empty States
Loading Skeleton
Error State
Tooltips
Dark Mode
Responsive
Keyboard navigation
Animation
```

---

# 74. 推荐真实开发顺序

如果直接按 Commit 开发，我建议：

```text
1.
refactor(ui): introduce AppShell and sidebar

2.
feat(home): add continuity-focused home view

3.
feat(workstreams): add shared workstream card and board

4.
refactor(workstreams): redesign workstream detail

5.
refactor(sessions): introduce session table and detail layout

6.
feat(settings): add settings shell and default agent

7.
refactor(settings): move ingest sources into settings

8.
refactor(assistant): add scoped workspace assistant UI

9.
refactor(projects): redesign projects views

10.
style(ui): consolidate tokens, dark mode and states
```

每个 Commit 尽量保持可以运行。

---

# 75. 第一轮最小闭环

如果不想一次改完，最值得先上线的是：

```text
AppShell
+
Sidebar
+
Home
+
Workstream Card
+
Workstreams View
```

这五项已经可以让 NoEnding 的整体产品感觉完全不同。

用户会立即形成：

```text
打开应用
↓
看到最近 Workstream
↓
New / Resume
```

的新心智。

---

# 76. 第二轮闭环

随后：

```text
Workstream Detail
+
Specific Session Resume
+
Core Context editing
```

这会建立 NoEnding 真正的核心体验：

```text
Continue
+
Understand
+
Execute
```

---

# 77. 验收标准：Home

打开应用：

```text
Home
```

自动出现。

用户可以在：

```text
1 click
```

内：

```text
Resume 最近 Workstream
```

或：

```text
New Session
```

不出现 Agent Picker。

按钮上明确显示 Agent Icon。

---

# 78. 验收标准：Workstreams

用户可以：

```text
浏览全部 Active Workstreams
搜索
排序
查看 Archived
新建 Workstream
```

点击 Card：

```text
→ Workstream Detail
```

---

# 79. 验收标准：Workstream Detail

第一屏能够回答：

```text
这是什么？
现在做到哪？
目标是什么？
还有什么问题？
做过什么决定？
有哪些约束？
最近有哪些 Session？
```

并可以直接：

```text
New
Resume latest
Resume specific
Ask Assistant
```

---

# 80. 验收标准：Sessions

Sessions 页面能够：

```text
快速找到具体 Session
```

而不是承担 Workstream 浏览功能。

用户可以：

```text
Search
Filter
Open
Resume
```

---

# 81. 验收标准：Assistant

Assistant 页面明确表现为：

```text
Workspace Assistant
```

而不是新的外部 Agent。

必须能看到当前：

```text
Scope
```

对于 mutation：

```text
User confirmation required
```

---

# 82. 验收标准：Projects

用户能够：

```text
查看 Project
查看其中 Workstreams
管理 Resources
```

但 UI 不要求任何 Workstream 必须属于 Project。

---

# 83. 验收标准：Settings

Sidebar 不再存在：

```text
会话数据源
```

一级入口。

全部进入：

```text
Settings
→ Session Sources
```

同时 Default Agent 设置真正影响：

```text
New Session
```

行为。

---

# 84. 首轮不追求的目标

本次重构不要同时做：

```text
Domain redesign
Sync architecture rewrite
New Agent support
Cloud sync
Embedded terminal
Advanced analytics
Complex notification system
```

目标只有一个：

> **让已经具备的 NoEnding Domain 能力，通过正确的信息架构真正呈现出来。**

---

# 85. 最终判断标准

完成这轮前端改造以后，一个第一次接触 NoEnding 的用户应该能够自然理解：

```text
Workstream
= 我要持续推进的事情

Session
= 我使用某个 Agent 推进这件事的一次执行

Project
= 我用来整理这些事情的长期主题

Assistant
= 帮我理解和操作整个 Workspace 的智能层
```

而不需要先阅读 NoEnding 的产品设计文档。

这就是此次前端重构最重要的成功标准。