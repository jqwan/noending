# NoEnding Projects Experience v0.2

## 1. 目标

把 Projects 改造成与：

```text
Workstreams
Sessions
```

同级的一等浏览页面。

最终信息架构：

```text
Sidebar

NoEnding
Search

工作区
├─ Workstreams
├─ Projects
├─ Sessions
└─ Assistant（实验）

最近
├─ Workstream A
├─ Workstream B
└─ ...

Settings
```

不再：

```text
PROJECTS
├─ Project A
├─ Project B
├─ Project C
├─ ...
```

Project 实体全部进入：

```text
Projects Board
```

浏览。

---

# 2. Sidebar 改造

当前 `Sidebar.tsx` 自己执行：

```ts
api.listProjects()
```

然后把所有 Project 渲染进侧栏。

删除：

```text
projects state
api.listProjects()
PROJECTS entity list
```

新增一个固定导航项：

```text
Projects
```

建议工作区导航顺序：

```text
Workstreams
Projects
Sessions
Assistant
```

原因：

```text
Workstream = 我正在做什么
Project    = 我在哪里做
Session    = 我做过哪些执行
```

Project Detail 时：

```text
Projects
```

保持弱高亮或 active。

这样 Sidebar 也少一次 Project 数据请求。

---

# 3. Projects 页面改成真正的 Board

当前 `ProjectsView` 虽然已经存在，但本质还是：

```text
纵向 ws-stack
+
1 + N API requests
```

即：

```text
listProjects()

然后每个 Project：
getProjectDetail(projectId)
```

Project 一多，这个结构不适合作为长期看板。

改成：

```text
Projects

物理工作空间，由工作目录自动派生。

[搜索 Projects...]            [刷新工作区状态]

筛选：全部 / 正常 / 有目录缺失
排序：最近活动 / 名称 / 目录数

┌──────────────┐ ┌──────────────┐
│ NoEnding     │ │ My App       │
│ Git 家族     │ │              │
│ ~/code/...   │ │ ~/dev/...    │
│              │ │              │
│ 2 路径       │ │ 1 路径       │
│ 3 Workstream │ │ 1 Workstream │
│ 18 Sessions  │ │ 4 Sessions   │
│ 最近 2h      │ │ 最近昨天     │
└──────────────┘ └──────────────┘
```

使用与 Workstreams 类似的：

```text
grid card layout
```

而不是 Sidebar list。

---

# 4. Project Card 信息

每张卡控制信息密度，只显示真正有辨识度的数据。

建议：

```text
Project Name

[Git 家族]
[1 个目录不在]

~/code/noending
另有 1 个目录

2 个目录
3 个 Workstream
18 个 Session

最近活动 2 小时前
```

不要显示：

```text
Project UUID
git_id
完整内部 identity
复杂 Git metadata
```

这些属于诊断信息，不是 Board 信息。

---

# 5. Search

Projects Board 增加搜索。

搜索面：

```text
Project name
WorkspacePath canonical path
```

例如输入：

```text
noending
```

既能找到：

```text
Project: NoEnding
```

也能找到：

```text
~/code/noending
```

第一版不用搜索 Workstream / Session 内容。

---

# 6. Filter

第一版只做少量真正有价值的筛选：

```text
全部
正常
有目录缺失
```

其中：

```text
正常
= 所有 WorkspacePath exists=true

有目录缺失
= 至少一个 WorkspacePath exists=false
```

不要加：

```text
active
completed
archived
```

因为 Project 没有 lifecycle。

也暂时不需要：

```text
Git / 非 Git
```

作为主要筛选。

Git family 可以作为卡片 badge。

---

# 7. Sort

建议：

```text
最近活动 ↓
名称
目录数量
```

默认：

```text
最近活动
```

其中 Project 的最近活动继续从：

```text
associated Session activity
+
Workstream activity
```

派生。

---

# 8. 解决当前 1 + N 请求

这是这轮应该顺手处理的重要问题。

当前：

```text
listProjects
+
N × getProjectDetail
```

改成专门的：

```rust
list_project_cards()
```

DTO：

```rust
ProjectCardData {
    id,
    name,
    name_customized,

    has_git_identity,

    path_count,
    missing_path_count,

    primary_workstream_count,
    related_workstream_count,

    session_count,

    representative_paths,

    last_activity_at,
}
```

前端：

```ts
api.listProjectCards()
```

一次拿完整 Board 数据。

不要为了卡片统计把整个：

```text
ProjectDetailData
```

全部传回来。

---

# 9. Projects 页面 Refresh 的语义

这里建议明确区分：

```text
重新读取 UI 数据
```

和：

```text
重新观察真实 workspace
```

Projects 页面真正有价值的是后者。

因此按钮不要简单叫：

```text
刷新
```

而叫：

```text
刷新工作区状态
```

它意味着：

```text
重新检查 WorkspacePath 是否存在
重新检测 Git 状态
重新检测 Git family / worktrees
执行已有 Project reassignment / merge
执行 WorkspacePath GC
执行 zero-path Project GC
```

它不是：

```text
Session Sync
Context Sync
Session 来源扫描
```

---

# 10. 全局 Refresh

增加 backend command：

```rust
refresh_workspace_projects()
```

内部复用现有：

```rust
workspace::project::reconcile_workspace_paths(...)
```

绝对不要重新实现 WorkspacePath 生命周期。

流程：

```text
点击「刷新工作区状态」
        ↓
backend background task
        ↓
scan registered WorkspacePaths
        ↓
filesystem observation
        ↓
Git observation
        ↓
existing Workspace reconcile
        ↓
existing GC rules
        ↓
emit workspace-reconcile-completed
        ↓
Projects Board reload
```

---

# 11. Refresh 必须后台执行

这里和刚才 Agent Model Discovery 的问题一样。

Workspace reconcile 里面会：

```text
Path::is_dir()
git rev-parse
git worktree list
```

而单次 Git probe 最大：

```text
10s
```

因此不能做：

```rust
#[tauri::command]
pub fn refresh_workspace_projects(...)
```

然后直接在 command thread 阻塞跑完。

应该：

```text
async command
+
spawn_blocking
```

或者沿用现有后台 worker/event 模式。

UI：

```text
[正在刷新工作区状态…]
```

但整个 App 仍然可操作。

---

# 12. Refresh Events

建议新增：

```text
workspace-reconcile-started
workspace-reconcile-progress
workspace-reconcile-completed
workspace-reconcile-failed
```

progress 可以只携带：

```text
scanned
total
```

不需要把内部 Git 信息推到前端。

完成结果可以：

```text
scanned
missing
moved
discovered
deleted_paths
deleted_projects
failed
```

---

# 13. Projects Board 的刷新体验

点击：

```text
刷新工作区状态
```

期间：

```text
Projects
                              [正在刷新…]

原有卡片仍然显示
```

不要：

```text
清空页面
→ Loading
→ 等 reconcile 完成
```

这是后台重新验证，不应该造成内容闪烁。

完成以后：

```text
toast:
工作区状态已刷新
```

如果发生：

```text
2 个目录变为不可用
1 个 Project 自动整理
```

可以轻量显示摘要。

---

# 14. 外部删除目录的体验

例如用户在 Finder 删除：

```text
~/code/foo
```

然后回 NoEnding：

```text
Projects
→ 刷新工作区状态
```

如果还有 Session / WorkstreamPath 引用：

```text
Project Foo
[1 个目录不在]
```

Card 保留。

进入 Detail：

```text
Workspace Paths

~/code/foo
[目录不存在]
```

完全符合当前 WorkspacePath lifecycle。

---

# 15. 无引用目录的 GC

例如：

```text
~/code/foo
```

外部已经删除，并且：

```text
Session refs = 0
WorkstreamPath refs = 0
not a live Git worktree
```

点击刷新：

```text
Workspace Reconcile
→ WorkspacePath GC
```

如果是 Project 最后一条路径：

```text
Project GC
```

然后：

```text
Project Card 自动从 Board 消失
```

这就是 Project 的真实生命周期。

---

# 16. Project Detail 增加局部 Refresh

Project Detail Header：

```text
NoEnding

[刷新目录状态] [重命名]
```

或者：

```text
[刷新] [重命名]
```

但 tooltip 明确：

```text
重新检查这个 Project 的工作目录与 Git 状态。
```

这个刷新只扫描：

```text
当前 Project 所拥有的 WorkspacePaths
```

不要全局扫所有 Project。

---

# 17. 后端增加 targeted reconcile

建议抽一个共享 primitive：

```rust
reconcile_workspace_path_ids(
    db,
    projection,
    path_ids,
)
```

然后：

```text
global refresh
→ registry all path ids

project refresh
→ project-owned path ids
```

两者最终都走相同：

```text
observe
ensure_workspace_path
gc_gone_workspace_paths
Project merge / retire
```

不要维护两套规则。

---

# 18. Project Refresh 后 Project 自己可能消失

这是正常情况。

例如 Project Detail：

```text
Project A
仅一个 WorkspacePath
```

用户已经从磁盘删除，而且没有引用。

点击：

```text
刷新目录状态
```

结果：

```text
WorkspacePath GC
→ zero-path Project GC
```

此时页面应：

```text
这个 Project 已经不存在

它最后一个工作目录已经离开 Workspace registry。
Workstream 与 Session 不会因此删除。

[返回 Projects]
```

当前 `ProjectDetail` 已经有一部分这种 gone UI，可以直接复用。

---

# 19. Project Detail 内容结构

建议整理成三个明确区域：

```text
Project Name
物理工作空间，由目录自动派生。

[刷新目录状态] [重命名]


概览
2 个工作目录
3 个 Workstream
18 个 Session


工作目录
────────────────
~/code/noending
Git · Main

~/worktrees/ui
Git · Worktree
[目录不存在]


Workstreams
────────────────
Workstream A      主关联
Workstream B      关联


Sessions
────────────────
最近 Session...
```

Project Detail 的重点始终是：

```text
Physical workspace projection
```

不是另一个 Workstream。

---

# 20. Projects 与 Workstreams 的视觉区别

虽然都用 Card Board，但不要做成完全一样。

Workstream Card 强调：

```text
意图
状态
Context
下一步
```

Project Card 强调：

```text
物理路径
可用性
Workstream / Session 数量
最近活动
```

所以用户一眼能理解：

```text
Workstream = 工作
Project = 地方
```

---

# 21. Sidebar Recent 保持不变

本轮我不建议删：

```text
最近 Workstreams
```

它和 Project 列表性质不同。

Recent 是：

```text
快速恢复近期工作
```

Projects list 是：

```text
把所有自动派生实体都铺在导航上
```

真正造成 Sidebar 膨胀的是后者。

因此本轮：

```text
Recent Workstreams   保留
Individual Projects 删除
Projects nav         新增
```

---

# 22. Sidebar active state

建议：

```text
route.view == projects
→ Projects active

route.view == project
→ Projects weak / active
```

与：

```text
Workstreams
Workstream Detail
```

保持相同模式。

---

# 23. Routes 不需要大改

现在已经有：

```ts
{ view: "projects" }
{ view: "project"; projectId }
```

所以 Router 模型不需要新增。

只是让：

```text
Sidebar → Projects
```

成为标准入口。

---

# 24. API

建议新增：

```text
list_project_cards

refresh_workspace_projects

refresh_project_workspace
```

保留：

```text
list_projects
get_project_detail
rename_project
```

其中：

```text
list_projects
```

仍可用于 Session filter 等轻量场景。

不要强制所有地方改用 cards。

---

# 25. 不改变 Project Domain

本轮非常重要：

```text
没有 New Project
没有 Delete Project
没有 Archive Project
没有手工 Add Session
没有手工 Add Workstream
```

唯一用户编辑仍然：

```text
Rename Project
```

Refresh：

```text
不是编辑 Project
```

而是：

> 重新观察外部物理世界，然后让现有 domain rules 重新投影。

---

# 26. 建议实现顺序

第一步改 Sidebar：

```text
删除 Project entity list
增加 Projects nav
```

第二步增加：

```text
ProjectCardData
list_project_cards
```

把当前 `ProjectsView` 从：

```text
1 + N detail reads
```

改成一次 Board read。

第三步重构 Projects Board：

```text
search
filter
sort
grid cards
```

第四步增加：

```text
global Workspace Refresh
```

第五步增加：

```text
Project Detail targeted refresh
```

最后补：

```text
loading / error / gone / refresh progress
```

---

# 27. 测试

前端至少锁：

```text
sidebar_has_projects_navigation

sidebar_does_not_render_individual_projects

projects_board_uses_single_card_query

projects_search_matches_name

projects_search_matches_workspace_path

missing_filter_only_shows_projects_with_missing_paths

project_refresh_keeps_existing_cards_while_running
```

Backend：

```text
global_refresh_reobserves_registered_paths

project_refresh_does_not_scan_unrelated_projects

refresh_marks_deleted_directory_missing

referenced_missing_path_survives_refresh

unreferenced_missing_path_is_gc_d

last_path_gc_retires_project

git_worktree_registration_prevents_premature_gc

restored_directory_becomes_present_again
```

还有：

```text
refresh_does_not_touch_workstream_paths
refresh_does_not_touch_session_history
refresh_does_not_ingest_sessions
```

---

# 28. 最终页面关系

完成以后：

```text
Sidebar
    │
    ├── Workstreams ──→ Workstreams Board
    │
    ├── Projects ─────→ Projects Board
    │                        ↓
    │                   Project Detail
    │
    └── Sessions ──────→ Sessions Board/Table
```

三者职责非常清楚：

```text
Workstreams
“我正在持续推进什么？”

Projects
“我的工作分布在哪些物理工作空间？”

Sessions
“我实际运行过哪些 Agent 会话？”
```

---

# 29. 我建议的最终 Projects Header

```text
Projects

NoEnding 根据 Session 和 Workstream 使用的工作目录自动整理这些工作空间。

[刷新工作区状态]
```

不要再用当前很长的说明文字反复解释：

```text
不能新建
不能删除
不能手工挂 Workstream
...
```

这些规则可以放到空态、tooltip 或帮助文案。

正常用户看到这个页面时应该首先看到自己的 Projects，而不是先读一段领域模型说明。

---

# 30. 建议提交拆分

```text
feat(ui): promote Projects to workspace navigation

feat(projects): add project board projection

feat(projects): add workspace state refresh

feat(projects): add targeted project refresh

test(projects): lock project board and reconcile UX
```

最终目标：

```text
Projects Experience v0.2

Sidebar cleaner                 ✅
Projects first-class board      ✅
No 1+N project detail reads     ✅
Search/filter/sort              ✅
Global physical refresh         ✅
Per-project refresh             ✅
External deletion observable    ✅
Existing GC semantics reused    ✅
No Project CRUD introduced      ✅
```
