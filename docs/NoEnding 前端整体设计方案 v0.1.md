# NoEnding 前端整体设计方案 v0.1

## 1. 设计目标

NoEnding 第一版前端不追求“大而全的 AI Workspace”，而是优先建立一个清晰、稳定、低学习成本的桌面工作空间。

核心体验只有一句话：

> **打开 NoEnding，立即知道自己上次做到哪里，并用最短路径继续。**

前端设计围绕四个核心对象展开：

```text
Workstream
持续工作的语义单位

Session
一次具体 Agent 交互

Project
可选的长期整理层

Assistant
整个 Workspace 的智能操作入口
```

产品层级保持：

```text
Home
→ Continue

Workstreams
→ Organize

Workstream Detail
→ Understand

Session
→ Execute

Assistant
→ Ask / Act

Projects
→ Organize long-term topics
```

---

# 2. 整体设计原则

## 2.1 Workstream-centered

视觉和导航都必须体现：

```text
Workstream > Session > Project
```

这里的 `>` 不是领域父子关系，而是用户日常使用频率和视觉优先级。

不能通过 UI 暗示：

```text
Project
└── Workstream
    └── Session
```

这种严格层级。

---

## 2.2 Continue first

任何高频页面都应该优先提供：

```text
New
Resume
```

而不是：

```text
Manage
Configure
Edit
```

Home 尤其如此。

---

## 2.3 Context first

Workstream Detail 第一屏展示：

```text
Current State
Goal
Open Questions
Decisions
Constraints
```

而不是：

```text
Sessions
Files
Statistics
```

因为 Workstream 的价值在 Context，而不是 Session 数量。

---

## 2.4 Agent-aware, not Agent-centered

Agent 信息需要可见，但不成为主心智。

例如：

```text
[ Claude icon  New ]

[ Codex icon   Resume ]
```

用户知道即将打开哪个 Agent，但不需要先选择 Agent。

---

## 2.5 Desktop-first

首版主要服务：

```text
macOS
Windows
```

不按移动端 Web 产品设计。

重点优化：

- 鼠标
- 键盘
- Hover
- Command Palette
- 大窗口
- 多任务快速切换

---

# 3. 整体视觉方向

第一版采用：

> **Minimal Desktop Workspace**

关键词：

```text
轻
安静
高信息密度
弱边框
低饱和
少量强调色
大量留白
```

视觉上不要靠近：

```text
Jira
Trello
Notion Dashboard
传统后台管理系统
```

更接近：

```text
Linear
Raycast
Arc settings
现代 IDE sidebar
轻量知识工具
```

但不直接模仿任何产品。

---

# 4. 应用整体框架

桌面布局：

```text
┌─────────────────────────────────────────────────────────────┐
│ Sidebar │                    Main                           │
│         │                                                   │
│ 248px   │                    flexible                       │
│         │                                                   │
└─────────────────────────────────────────────────────────────┘
```

首版：

```text
Sidebar: 248px
Main: flex: 1
```

Main 最大阅读宽度：

```text
1200px ~ 1400px
```

对于 Workstream Detail 这种文本密集页面：

```text
Main content:
约 1100px
```

避免横向过宽。

---

# 5. Sidebar

推荐最终结构：

```text
◇ NoEnding
  Context doesn't end.

Search                        ⌘K

WORKSPACE

▦ Workstreams
◫ Sessions
✦ Assistant


RECENT

Context Integrity
Initial UI Redesign
Agent Adapter
Context Sync


PROJECTS                    ›

NoEnding
Japan Trip


────────────────────────────

⚙ Settings
```

---

# 6. Logo 行为

```text
NoEnding Logo / Brand
→ Home
```

不单独设置 Home 菜单项。

因此：

```text
App Launch
→ Home

Logo
→ Home
```

Home 是产品起点，而不是普通模块。

---

# 7. Sidebar 一级导航

顺序固定：

```text
Workstreams
Sessions
Assistant
```

原因：

```text
Workstreams
= 用户正在推进的事情

Sessions
= 具体执行记录

Assistant
= 智能能力
```

Assistant 不应该放第一位。

---

# 8. Recent Workstreams

显示最近：

```text
5 ~ 7
```

按：

```text
last_activity DESC
```

这里只显示：

```text
Workstream Title
```

不显示 Current State 或按钮。

点击：

```text
→ Workstream Detail
```

未来可以加入 hover menu：

```text
•••
New
Resume
Pin
```

第一版不需要。

---

# 9. Projects Sidebar

Project 与 Recent 独立显示。

不要：

```text
▼ NoEnding
    Context Integrity
    UI Redesign
```

而是：

```text
RECENT

Context Integrity
UI Redesign


PROJECTS

NoEnding
Japan Trip
```

保持 Workstream 与 Project 的领域独立性。

---

# 10. Sidebar Active State

Workstreams Board：

```text
▦ Workstreams
```

强高亮。

进入 Workstream：

```text
▦ Workstreams          ← weak active

RECENT
▌ Context Integrity    ← strong active
```

Session Detail：

```text
◫ Sessions
```

保持 Active。

Project Detail：

```text
PROJECTS

▌ NoEnding
```

高亮具体 Project。

---

# 11. Sidebar Scroll

结构：

```text
Fixed
├ Logo
└ Search

Scrollable
├ Workspace
├ Recent
└ Projects

Fixed
└ Settings
```

Settings 永远保持可见。

---

# 12. Home

Home 的唯一任务：

> **Continue where you left off.**

页面结构：

```text
Good to see you again.
Continue where you left off.


最近活动                              Workstreams →

[ Workstream Card ] [ Workstream Card ]

[ Workstream Card ] [ Workstream Card ]


最近会话
...
```

首页保持极简。

---

# 13. Home Workstream 数量

默认：

```text
4
```

宽屏：

```text
2 × 2
```

如果需要稍微多展示：

```text
最多 6
```

不要无限增长。

完整集合进入：

```text
Workstreams →
```

---

# 14. Workstream Card

统一组件：

```text
WorkstreamCard
```

卡片结构：

```text
┌────────────────────────────────────┐
│ Context Integrity        NoEnding  │
│                                    │
│ 验证 compact → append → rescan... │
│                                    │
│ 18 分钟前 · 3 sessions             │
│                                    │
│ [ Claude New ]   [ Codex Resume ] │
└────────────────────────────────────┘
```

---

# 15. Card 信息优先级

顺序：

```text
Title

Project

Current State

Last Activity
Session Count

Actions
```

---

# 16. Current State fallback

正文优先级：

```text
Current State
↓
Description
↓
Goal
```

最多：

```text
2 lines
```

不要把卡片做成 Context 阅读器。

---

# 17. Card Actions

有 Session：

```text
[ Default Agent Icon  New ]

[ Latest Session Agent Icon  Resume ]
```

无 Session：

```text
[ Default Agent Icon  Start ]
```

---

# 18. New Session

点击：

```text
New
```

立即：

```text
Default Agent
↓
Build Workstream Context Bundle
↓
Launch
```

无 Agent 确认页。

按钮提前显示默认 Agent 图标。

---

# 19. Resume

点击：

```text
Resume
```

立即：

```text
Latest Activity Session
↓
Sync if stale
↓
Build Delta Context
↓
Resume
```

不再弹 Session Picker。

需要指定 Session：

```text
→ Workstream Detail
```

---

# 20. Card 点击

卡片主体：

```text
→ Workstream Detail
```

按钮：

```text
New
→ New Session

Resume
→ Latest Session
```

整张卡片不能直接 Resume。

---

# 21. Home 最近会话

第一版可以保留一个非常轻量的 Recent Sessions 区域。

例如：

```text
最近会话

Codex
Context Integrity v0.1.1-fix2
18 分钟前

Claude
UI exploration
2 小时前
```

最多：

```text
3
```

如果后续发现使用率低，可以删除。

Home 的视觉中心仍然是 Workstream。

---

# 22. Workstreams 页面

这是完整浏览和整理页面。

结构：

```text
Workstreams                          + New Workstream

Search workstreams...

All   Active   Archived

All Projects               Last Activity ↓


Active

[ Card ] [ Card ]

[ Card ] [ Card ]
```

---

# 23. Workstreams 布局

桌面：

```text
2 columns
```

窄窗口：

```text
1 column
```

不采用传统：

```text
Todo
Doing
Done
```

Kanban。

Workstream lifecycle 不是 task workflow。

---

# 24. Workstreams Search

搜索：

```text
Title
Description
Current State
Project Name
```

只用于找到 Workstream。

全局 Context 搜索仍通过：

```text
⌘K
```

---

# 25. Workstreams Filter

第一版：

```text
All
Active
Archived
```

可选：

```text
Project
```

不要第一版就做复杂 Filters。

---

# 26. Workstreams Sort

第一版：

```text
Last Activity
Created
Name
```

默认：

```text
Last Activity DESC
```

---

# 27. Workstreams Group

第一版可以不实现。

预留：

```text
Group by
None
Project
```

默认：

```text
None
```

---

# 28. New Workstream

按钮：

```text
+ New Workstream
```

Modal：

```text
Create Workstream

Title
[                         ]

Description
[                         ]

Project
[ Optional              ▾ ]

Cancel               Create
```

Project 可选。

创建成功后：

```text
→ Workstream Detail
```

或者保持在当前页并将新 Card 插到最前。

我建议：

```text
Create
→ Workstream Detail
```

让用户自然继续补充 Context 或直接 Start Session。

---

# 29. Workstream Detail

这是整个产品最核心的内容页。

推荐：

```text
← Workstreams

● Context Integrity                         Ask Assistant

NoEnding   Active   Edited 2 hours ago

                         [ Claude New ]
                         [ Codex Resume ]
                         [ ••• ]
```

---

# 30. Workstream Detail 主体布局

建议两栏：

```text
┌──────────────────────────────┬──────────────────────┐
│ Main Context                 │ Sessions             │
│                              │                      │
│ Current State                │ Latest Sessions      │
│ Goal                         │                      │
│ Open Questions               │ Activity             │
│ Decisions                    │                      │
│ Constraints                  │                      │
│ Extended Context             │                      │
└──────────────────────────────┴──────────────────────┘
```

比例：

```text
Main: ~70%
Right rail: ~30%
```

---

# 31. Core Context

展示顺序建议：

```text
Current State

Goal

Open Questions

Decisions

Constraints
```

这里我建议把：

```text
Current State
```

放第一位。

因为用户进入详情页最关心：

> 现在做到哪里了？

---

# 32. Context Section

例如：

```text
当前状态                              Edit

Lazy source-driven identity migration 已完成，
当前正在验证 compact → append → rescan 情况。
```

Hover：

```text
Edit
```

点击 inline edit。

保存以后：

```text
user_edit
+
new Revision
```

UI 不暴露内部 authority 字段。

---

# 33. Context History

每个 section：

```text
•••
```

菜单：

```text
Edit
View History
Copy
```

View History 打开：

```text
Side panel / modal
```

显示：

```text
Current
Sep 14 · You

Previous
Sep 13 · Codex Sync

Previous
Sep 12 · You
```

无需跳转独立页面。

---

# 34. Extended Context

Core Context 下：

```text
More Context

TODO
Verify Windows source replacement

RISK
Legacy migration may ...

REFERENCE
src-tauri/src/ingestion/mod.rs

View all →
```

Extended Context 可以使用：

```text
compact rows
```

而不是大 Card。

---

# 35. Workstream Sessions Rail

右侧：

```text
Sessions                     View all →

Codex
Context Integrity v0.1.1-fix2
18 分钟前                     Resume

Claude
Review sync architecture
1 天前                        Resume

Pi
Context model discussion
3 天前                        Resume
```

这里 Resume 指定具体 Session。

---

# 36. Workstream Activity

右栏 Sessions 下：

```text
Activity

18m
Codex updated Current State

2h
Decision added

1d
Claude session bound

2d
Goal edited by you
```

只显示高层事件。

---

# 37. Ask Assistant

Workstream Detail 顶部：

```text
Ask Assistant
```

点击：

```text
→ Assistant

Scope:
Workstream = Context Integrity
```

而不是在右侧再嵌一个完整聊天框。

---

# 38. Sessions 页面

Sessions 是工具型页面。

布局优先采用：

```text
table / list
```

而不是 Card Grid。

```text
Sessions                                + New Session

Search sessions...

All Agents
All Workstreams
All Projects
Last Activity ↓


AGENT      SESSION              WORKSTREAM             LAST ACTIVE

Codex      Context Integrity    Context Integrity      18m
Claude     UI exploration       Initial UI Redesign     2h
Pi         Discussion           —                       1d
```

---

# 39. Sessions Filters

第一版：

```text
Agent
Workstream
Project
Assigned State
```

Assigned State：

```text
All
Assigned
Unassigned
```

---

# 40. Session Row

显示：

```text
Agent Icon
Session Title
Workstream
Last Activity
Resume
•••
```

点击行：

```text
→ Session Detail
```

Resume：

```text
→ 指定 Session Resume
```

---

# 41. New Session

Sessions 页：

```text
+ New Session
```

和 Workstream 中 New 不一样。

这里是“无 Workstream 前置”的全局 New Session。

点击后：

第一版也可以直接使用 Default Agent。

可选提供一个极轻量 Modal：

```text
New Session

Workstream
None                         ▾

Agent
Claude Code                  Default

Start
```

但默认：

```text
Workstream = None
Agent = Default Agent
```

用户可以直接 Start。

---

# 42. Session Detail

顶部：

```text
← Sessions

Context Integrity v0.1.1-fix2

Codex

Context Integrity · NoEnding

Started Sep 13
Last active 18m

                                      Resume
```

---

# 43. Session Detail 主内容

以标准化消息流展示：

```text
You

修复这个 migration 问题。


Codex

我先检查 schema migration...


Tool

cargo test


Codex

已经定位到问题...
```

所有 Agent 使用统一 UI。

---

# 44. Session Message Types

建议区分：

```text
user
assistant
tool
system
artifact
```

但视觉差异不要过大。

例如：

```text
User
→ 普通正文

Assistant
→ 普通正文

Tool
→ 小型 code / result block

System
→ 弱提示

Artifact
→ File card
```

---

# 45. Session Workstream Binding

Session Detail 右上或 metadata 区：

```text
Workstreams

Primary
Context Integrity

Related
Sync Engine

Edit
```

点击 Edit：

```text
Binding Modal
```

支持：

```text
Primary
Related
Remove
Add Workstream
```

---

# 46. Assistant

Assistant 是：

> Workspace Interface

不是“第四个 Agent”。

---

# 47. Assistant 页面

结构：

```text
Assistant

Scope: Workspace ▾


                  ✦

        What are you working on?

Ask about your Workstreams, Sessions and Context.


Suggested:

最近 NoEnding 项目主要解决了什么？

Context Integrity 还有哪些 Open Questions？

找一下讨论 Windows launcher 的 Session。

把这个决定加入 Context。


─────────────────────────────────────────────

Ask NoEnding...                         Send
```

---

# 48. Assistant Scope

支持：

```text
Workspace

Project

Workstream

Session
```

Scope selector：

```text
Scope: Workspace ▾
```

从其它页面进入自动设置。

例如：

```text
Workstream Detail
→ Ask Assistant
→ Scope = Workstream
```

---

# 49. Assistant Conversation

聊天区域和普通 Agent Chat 有所区别。

Assistant 回答要尽量引用内部对象：

```text
Context Integrity

Open Questions
...

Related Sessions
...
```

对象名称可点击。

点击：

```text
→ Workstream / Session Detail
```

---

# 50. Assistant Actions

Assistant 可提出：

```text
Action Card
```

例如：

```text
Merge Workstreams

Context Integrity
+
Sync Reliability

Target
Context Integrity

Cancel                  Merge
```

用户确认后才执行。

所有结构性改变必须明确确认。

---

# 51. Projects

Projects 页面保持弱管理属性。

```text
Projects                                + New Project

Search projects...


NoEnding
Local multi-Agent workspace...

3 workstreams · 12 sessions
Last active 18m


Japan Trip
4 workstreams · 7 sessions
Last active 2d
```

---

# 52. Project Card

显示：

```text
Name
Description
Active Workstreams
Session Count
Last Activity
```

不展示 Project Context。

---

# 53. Project Detail

结构：

```text
NoEnding

Local multi-Agent workspace centered around
persistent Workstream Context.


Resources                               Edit

GitHub
github.com/jqwan/noending

Workspace
~/code/noending


Workstreams

Context Integrity
Initial UI Redesign
Agent Adapter


Recent Sessions

Codex
Context Integrity...

Claude
UI exploration...
```

---

# 54. Project Resources

使用轻量 Resource Row：

```text
Repository
github.com/jqwan/noending

Workspace
~/code/noending

Document
Architecture.md

URL
...
```

支持：

```text
Add Resource
Remove
Open
```

---

# 55. Project Assignment Suggestion

Workstream 可出现：

```text
Suggested Project

NoEnding                  Accept
```

不使用 Warning。

不阻塞任何操作。

---

# 56. Settings

Settings 两栏布局：

```text
┌──────────────────┬─────────────────────────────┐
│ General          │ General                     │
│ Agents           │                             │
│ Session Sources  │ Default Agent               │
│ Context & Sync   │ Claude Code ▾               │
│ Appearance       │                             │
│ Data & Advanced  │ Startup Page                │
│                  │ Home                        │
└──────────────────┴─────────────────────────────┘
```

---

# 57. General

第一版：

```text
Default Agent
Claude Code

Startup Page
Home

Confirm before launching Session
Off
```

其中：

```text
Default Agent
```

是最重要设置。

---

# 58. Agents

```text
Claude Code

Detected
vX.X

/usr/local/bin/claude

Re-detect


Codex

Detected

...


Pi

Not detected
```

---

# 59. Session Sources

当前的独立：

```text
会话数据源
```

页面移入 Settings。

显示：

```text
Claude Code

~/.claude
Enabled


Codex

~/.codex
Disabled


+ Add Source
```

支持：

```text
Enable
Disable
Re-ingest
Remove Custom Source
```

---

# 60. Context & Sync

第一版只暴露真正有用户价值的设置：

```text
Automatic Sync
On

Automatic Workstream Classification
On

Background Reconcile
On
```

不要暴露：

```text
threshold
dedup similarity
authority weight
cursor
generation
```

这些属于实现细节。

---

# 61. Appearance

```text
Theme

System
Light
Dark


Density

Comfortable
Compact
```

首版 Sidebar collapse 可以作为 UI Action，而不一定需要设置。

---

# 62. Data & Advanced

```text
Database

/path/to/noending.db

Open Data Folder


Search

Rebuild Search Index


Data

Export Data


Developer

Debug Information
```

危险操作单独分组。

---

# 63. Command Palette

快捷键：

```text
⌘K
Ctrl+K
```

支持搜索：

```text
Workstreams
Sessions
Projects
Commands
```

例如：

```text
Open Workstream: Context Integrity

Resume Session: ...

Go to Workstreams

Go to Sessions

Go to Assistant

New Workstream

New Session
```

---

# 64. Global Keyboard Navigation

第一版只需要少量快捷键：

```text
⌘K / Ctrl+K
Command Palette

Esc
Close modal / panel

Enter
Execute focused primary action
```

暂时不要引入大量 IDE 风格 shortcut。

---

# 65. Modal 原则

Modal 只用于：

```text
Create
Confirm
Edit relationship
Danger action
```

不要把阅读内容放 Modal。

例如：

```text
New Workstream
→ Modal

Merge Workstream
→ Confirmation Modal

Session Binding
→ Modal
```

---

# 66. Drawer / Side Panel

适合：

```text
Context History
Item Details
Assistant references
```

而不是整个页面跳转。

第一版如果实现成本高，也可以统一使用 Modal。

---

# 67. Design Tokens

建议前端不要继续大量 scattered CSS。

定义统一 tokens。

例如：

```text
--bg-app
--bg-sidebar
--bg-panel
--bg-hover
--bg-active

--text-primary
--text-secondary
--text-muted

--border-subtle
--border-default

--accent
--success
--warning
--danger
```

---

# 68. Spacing

统一：

```text
4
8
12
16
20
24
32
40
48
```

常用：

```text
Card padding
16 ~ 20

Page padding
24 ~ 32

Section gap
24 ~ 32
```

---

# 69. Border Radius

建议：

```text
small control: 6px
button: 7 ~ 8px
card: 10 ~ 12px
modal: 12px
```

不要过度圆角。

NoEnding 是桌面生产力工具，不是移动消费 App。

---

# 70. Border 与 Shadow

优先：

```text
1px subtle border
```

而不是明显 shadow。

Card：

```text
background
+
subtle border
```

Hover 时：

```text
border slightly stronger
+
very light shadow
```

---

# 71. Typography

标题：

```text
Page Title
24 ~ 28px

Workstream Title
15 ~ 17px

Section Title
12 ~ 13px
uppercase / semibold

Body
13 ~ 14px

Metadata
11 ~ 12px
```

不要过多字体层级。

---

# 72. Agent Icon

统一尺寸：

```text
14px
16px
20px
```

Card Action：

```text
14 ~ 16px
```

Session List：

```text
14px
```

Session Detail：

```text
20px
```

Agent Icon 只用于快速识别，不用大面积品牌色。

---

# 73. Status Color

避免颜色滥用。

例如：

```text
Active
→ subtle green dot

Archived
→ gray

Conflict
→ amber

Error
→ red
```

正常状态不要用大量绿色 Badge。

---

# 74. Button Hierarchy

Primary：

```text
Resume
Create
Confirm
```

Secondary：

```text
New
Edit
```

Ghost：

```text
View all
•••
Back
```

但 Home 上：

```text
Resume
```

可以比：

```text
New
```

稍强一点。

因为 Resume 是最高频动作。

---

# 75. Loading

不要整页 Spinner。

使用：

```text
Skeleton
```

例如 Workstream Card：

```text
██████████

████████████████

██████
```

Sidebar Recent 也使用简单 skeleton rows。

---

# 76. Error State

避免：

```text
Something went wrong
```

应告诉用户具体失败对象。

例如：

```text
Couldn't resume this Codex session.

Codex executable was not found.

Open Agent Settings
```

---

# 77. Empty State

Workstreams：

```text
No workstreams yet.

Create a Workstream for something
you want to continue across sessions.

+ New Workstream
```

Sessions：

```text
No sessions have been imported yet.

Configure Session Sources
```

Project：

```text
No projects yet.

Projects are optional.
Create one when you want to group related work.
```

这一句很重要：

> Projects are optional.

---

# 78. Frontend Route 设计

建议 route 明确化。

当前手写：

```text
Route union
```

仍可保留。

推荐：

```text
home

workstreams
workstream/:id

sessions
session/:id

assistant

projects
project/:id

settings
settings/:section

search
```

---

# 79. React Feature 结构

建议逐步调整成：

```text
src/

app/
  AppShell.tsx
  Router.tsx
  routes.ts

components/
  Button
  IconButton
  Badge
  Modal
  EmptyState
  SearchInput
  AgentIcon

layout/
  Sidebar
  PageHeader
  ContentLayout

features/

  home/
    HomeView.tsx

  workstreams/
    WorkstreamsView.tsx
    WorkstreamDetailView.tsx
    WorkstreamCard.tsx
    WorkstreamContext.tsx
    WorkstreamSessions.tsx

  sessions/
    SessionsView.tsx
    SessionDetailView.tsx
    SessionTable.tsx
    SessionMessage.tsx

  assistant/
    AssistantView.tsx
    AssistantMessage.tsx
    AssistantAction.tsx
    ScopeSelector.tsx

  projects/
    ProjectsView.tsx
    ProjectDetailView.tsx
    ProjectCard.tsx
    ProjectResources.tsx

  settings/
    SettingsView.tsx
    GeneralSettings.tsx
    AgentSettings.tsx
    SourcesSettings.tsx
    ContextSyncSettings.tsx
    AppearanceSettings.tsx
    AdvancedSettings.tsx

  search/
    SearchView.tsx
    CommandPalette.tsx
```

---

# 80. AppShell

目前 `App.tsx` 承担 Sidebar + router + state。

建议逐渐拆为：

```text
App
↓
AppShell
├ Sidebar
└ RouterOutlet
```

Sidebar 自己负责：

```text
Projects
Recent Workstreams
Navigation state
```

App 不继续堆业务逻辑。

---

# 81. 数据加载原则

页面尽量自己加载页面所需数据。

例如：

```text
HomeView
→ getHomeOverview

WorkstreamsView
→ listWorkstreams

WorkstreamDetailView
→ getWorkstreamContext
→ listWorkstreamSessions

SessionsView
→ listSessions
```

避免 App 顶层把所有数据一次加载完后向下传递。

---

# 82. 推荐增加聚合 API

为了避免前端发过多请求，Home 可以增加：

```text
get_home_overview()
```

返回：

```text
recent_workstreams
recent_sessions
agent_status
```

Workstream Detail 可以考虑：

```text
get_workstream_detail(id)
```

返回：

```text
workstream
project
core_context
extended_context
recent_sessions
activity
```

这样前端页面模型会比直接拼多个 Domain API 更稳定。

---

# 83. UI State 与 Domain State 分离

前端自己管理：

```text
selected tab
modal open
search query
sort
filter
sidebar collapsed
```

Rust Domain 管理：

```text
Workstream lifecycle
Context authority
Binding
Classification
Conflict
Revision
```

不要在 React 复制 Domain invariant。

---

# 84. 乐观更新

第一版谨慎使用。

适合：

```text
Archive
Simple metadata edit
UI preference
```

不适合：

```text
Merge Workstream
Context mutation
Binding change
Resume
Sync
```

这些应等待 Rust 成功响应。

---

# 85. Toast

只用于：

```text
Background success
Non-blocking feedback
```

例如：

```text
Workstream created

Context updated

Session source enabled
```

重大错误使用页面内错误提示。

---

# 86. Background Sync

后台 Sync 完成后：

```text
reconcile-completed
sync-completed
```

前端不要整页刷新。

只触发：

```text
invalidate affected queries
```

例如：

```text
Sidebar Recent
Home cards
Current Workstream
Sessions
```

---

# 87. 第一版动效

保持很少。

建议：

```text
Hover
120 ~ 160ms

Modal
150ms

Card enter
none / very subtle

Sidebar collapse
180ms
```

不要做大量 spring animation。

---

# 88. 深色模式

NoEnding 是开发者/知识工作者桌面工具，Dark Mode 必须是一等模式。

但第一版首先保证：

```text
semantic color tokens
```

不要直接在组件里写：

```text
#fff
#000
```

这样 Light / Dark 才容易稳定维护。

---

# 89. Windows / macOS

布局尽量保持一致。

差异只体现在：

```text
Keyboard labels

⌘K
Ctrl+K
```

以及 Window chrome。

不要设计两套不同 UI。

---

# 90. 第一版明确不做

为了保持 v0.1 足够克制，以下暂时不要做：

```text
复杂 Kanban

拖拽排序

Dashboard Charts

多层 Project Tree

Workstream Nested Hierarchy

大量 Context Item 类型 UI

复杂 Saved Filters

多 Pane IDE Layout

内嵌 Terminal

内嵌 Agent Chat Runtime

自定义 Sidebar Layout

复杂 Activity Analytics
```

---

# 91. 第一版核心页面优先级

开发顺序建议：

```text
P0

AppShell / Sidebar
Home
Workstream Card
Workstreams
Workstream Detail


P1

Sessions
Session Detail


P2

Assistant


P3

Projects
Settings polish
```

Settings 中 Default Agent / Sources 如果现有功能依赖，可以提前实现。

---

# 92. 第一版最重要的用户路径

## 路径 A：继续工作

```text
Open NoEnding
↓
Home
↓
Context Integrity
↓
Resume
```

目标：

```text
1 click after entering Home
```

---

## 路径 B：用新 Agent Session 继续

```text
Home
↓
Workstream
↓
Claude New
```

也是：

```text
1 click
```

---

## 路径 C：指定 Session

```text
Home / Sidebar
↓
Workstream Detail
↓
Sessions
↓
Specific Resume
```

---

## 路径 D：找旧 Workstream

```text
⌘K
↓
Search
↓
Open Workstream
```

或者：

```text
Workstreams
↓
Search
```

---

## 路径 E：无 Workstream 直接开始

```text
Sessions
↓
New Session
```

或者 Command Palette：

```text
⌘K
↓
New Session
```

保持：

> Workstream-centered, not Workstream-required.

---

# 93. 最终设计语言

NoEnding 的前端不应该给人感觉：

> “这是一个管理 AI 对话的软件。”

而应该给人感觉：

> **“这是一个保存我长期工作状态，并让我随时继续的桌面工作空间。”**

所以设计重点始终是：

```text
Continuity
Context
Resume
Clarity
Low friction
```

而不是：

```text
Chat
Agent
Dashboard
Project Management
Automation Center
```

---

# 94. v0.1 最终界面体系

```text
App Shell
│
├── Home
│   └── Continue
│
├── Workstreams
│   └── Workstream Detail
│       ├── Context
│       ├── Sessions
│       └── Activity
│
├── Sessions
│   └── Session Detail
│
├── Assistant
│
├── Projects
│   └── Project Detail
│
└── Settings
    ├── General
    ├── Agents
    ├── Session Sources
    ├── Context & Sync
    ├── Appearance
    └── Data & Advanced
```

整个产品围绕同一个中心建立：

> **Workstream 保存持续演进的 Context，Session 只是继续推进它的一种方式。**

前端所有视觉、导航和交互，都应该不断强化这一点，而不是削弱它。