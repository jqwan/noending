# NoEnding Workspace Domain v0.2 — 多 Agent 并行执行方案

## 0. 阶段目标

本阶段实现已经冻结的 Workspace Domain v0.2，不扩展 Context / Assistant / 智能分类。

最终用户模型：

```text
NoEnding Home
└── default workspace

Project
    1
    │
    N
WorkspacePath
    │
    ├──────── Session
    │
    └──────── WorkstreamPath
                   │
                   N
                   │
                   1
              Workstream
```

核心定义：

```text
Project
= NoEnding 自动维护的物理 workspace family

WorkspacePath
= 一个稳定识别的物理工作路径

Workstream
= 用户持续推进的一件工作

Session
= Agent 的一次真实执行
```

Project 完全由应用管理。

用户：

```text
不能创建 Project
不能删除 Project
不能手工选择 Session → Project
不能手工选择 Workstream → Project

只能重命名 Project
```

Project 在最后一个 WorkspacePath 消失时自动删除。

---

# 1. 本阶段不可修改的领域契约

所有 Agent 开工前必须阅读并接受以下 invariant。

## 1.1 WorkspacePath

```text
Every WorkspacePath belongs to exactly one Project.
```

一个 WorkspacePath 任意时刻只能属于一个 Project。

WorkspacePath 必须具有：

```text
path_id
canonical_path
project_id
git_state
exists
```

`path_id` 永远存在。

---

## 1.2 Project

```text
Every Project owns >= 1 WorkspacePath.
```

Project 不能空存在。

最后一个 WorkspacePath：

```text
被删除
或
被迁移到另一个 Project
```

后：

```text
Project 自动删除
```

Project 不存在：

```text
archived
completed
trashed
orphan
```

等生命周期。

---

## 1.3 Git

```text
Project owns Git identity.
WorkspacePath owns Git detectability.
```

即：

```text
Project.git_id
```

表示 Project 的 Git workspace family identity。

而：

```text
WorkspacePath.git_state
```

表示这个具体路径当前是否能检测到 Git。

`.git` 从某个 WorkspacePath 消失：

```text
detected → missing
```

不得自动：

```text
清除 Project.git_id
改变 WorkspacePath.project_id
拆分 Project
创建新 Project
```

---

## 1.4 ~/.git

如果：

```text
git root == user home
```

或：

```text
git common dir == ~/.git
```

则忽略该 Git evidence。

该 WorkspacePath 退化为普通 path identity。

不得因为 dotfiles Git repository 将整个用户 Home 归为一个 Project。

---

## 1.5 Workstream Paths

Workstream 的工作路径是：

```text
有序列表
```

不是 primary / secondary 两套集合。

例如：

```text
0 /repo/main
1 /repo/docs
2 /repo/service
```

天然定义：

```text
position 0 = 主工作路径
position > 0 = 次工作路径
```

必须满足：

```text
paths.length == 0
OR
paths[0] exists
```

不存在：

```text
无主路径 + 有次路径
```

---

## 1.6 Workstream 删除路径

删除：

```text
0 /A
1 /B
2 /C
```

中的 `/A` 后：

```text
0 /B
1 /C
```

`/B` 自然成为主路径。

不要求用户重新选择主路径。

删除 WorkstreamPath：

```text
同时移除该 Workstream 中属于这条 WorkstreamPath 的 Session bindings
```

但：

```text
不删除 Session 本身
不删除 Session Events
```

---

## 1.7 Workstream 添加路径

添加 WorkstreamPath：

```text
只增加路径
```

绝不自动扫描并导入这个路径下的历史 Sessions。

---

## 1.8 Session 加入 Workstream

Session 加入 Workstream 时：

```text
Session path 已在 WorkstreamPaths
→ 仅建立 binding

Session path 不在 WorkstreamPaths
且 Workstream 无路径
→ append，成为 position 0

Session path 不在 WorkstreamPaths
且已有路径
→ append 到末尾
```

---

## 1.9 Session 从 Workstream 移除

只移除：

```text
Session ↔ Workstream binding
```

不自动删除 WorkstreamPath。

---

## 1.10 Session → Project

Session 的 Project 唯一事实链：

```text
Session.workspace_path_id
        ↓
WorkspacePath.project_id
        ↓
Project
```

当前：

```text
sessions.project_id
```

可以继续存在，但只能作为 derived cache。

禁止手工赋值。

---

## 1.11 Session Project Refresh

只在：

```text
Session.workspace_path_id changed
```

或者：

```text
WorkspacePath.project_id changed
```

时刷新：

```text
Session.project_id
```

普通 Session event ingestion 不触发 Project 计算。

---

## 1.12 Workstream → Project

Workstream 不再直接拥有单一 Project。

其 Project projection：

```text
WorkstreamPath
    ↓
WorkspacePath
    ↓
Project
```

一个 Workstream 可以因为不同路径出现在多个 Project 中。

如果某 Project 包含 Workstream 的 `position=0` 路径：

```text
Project Detail 可显示“主关联”
```

否则：

```text
关联
```

不新增 `workstream_project_bindings` 表。

---

## 1.13 Workstream lifecycle

继续利用现有两个正交字段，避免无意义 schema 扩张：

```text
lifecycle
= active | completed

visibility
= normal | archived
```

语义：

```text
active / completed
只是基础状态分类
没有行为差异
可以随时切换

archived
= 回收站
```

恢复 archived Workstream：

```text
visibility → normal
```

不改变：

```text
lifecycle
paths
Session bindings
其他配置
```

因此自然恢复之前状态。

永久删除只能从 archived 状态执行。

---

# 2. NoEnding Home 契约

默认：

```text
~/.noending/
```

Windows：

```text
%USERPROFILE%\.noending\
```

结构：

```text
~/.noending/
├─ data/
│  └─ noending.db
├─ runtime/
├─ logs/
└─ workspace/
```

其中：

```text
data/
runtime/
logs/
```

属于 reserved app paths，不进入 WorkspaceResolver。

只有：

```text
workspace/
```

是正常 WorkspacePath。

默认工作目录：

```text
<NoEnding Home>/workspace
```

---

# 3. NoEnding Home bootstrap

因为数据库本身位于 NoEnding Home 内，所以 Home 位置不能只记录在数据库中。

使用一个极小的 OS-native bootstrap config：

```text
current_home
pending_home?
```

解析优先级：

```text
NOENDING_HOME
    ↓
bootstrap.current_home
    ↓
~/.noending
```

用户修改 NoEnding Home 时：

```text
不在当前进程直接切数据库
```

而是：

```text
写 pending_home
→ UI 提示重启生效
→ 下次启动、打开 DB 前执行迁移
→ 成功后 current_home = pending_home
→ clear pending_home
```

这样避免复制数据库后当前进程继续向旧 DB 写入产生分叉。

---

# 4. 修改 NoEnding Home 时 workspace 处理

只迁移应用拥有的数据：

```text
data/
runtime/
logs/
```

不能静默搬迁旧：

```text
<old-home>/workspace
```

中的用户文件。

新默认工作目录变成：

```text
<new-home>/workspace
```

旧 workspace 如果仍存在：

```text
继续作为普通 WorkspacePath 存在
```

---

# 5. Schema v12

当前 schema 为 v11。

Workspace Domain v0.2 使用：

```text
SCHEMA_VERSION = 12
```

## 5.1 projects

保留稳定：

```text
id
name
created_at
updated_at
```

新增：

```text
git_id TEXT NULL
name_customized INTEGER NOT NULL DEFAULT 0
```

旧：

```text
description
archived
```

暂时物理保留兼容，但退出新的产品语义。

要求：

```text
UNIQUE git_id WHERE git_id IS NOT NULL
```

---

## 5.2 git_identities

建议新增：

```text
git_identities

id               TEXT PRIMARY KEY
common_dir       TEXT NOT NULL UNIQUE
first_seen_at    TEXT NOT NULL
last_seen_at     TEXT NOT NULL
metadata         TEXT NOT NULL DEFAULT '{}'
```

其中：

```text
id
= NoEnding 生成的 Git family UUID
```

Project：

```text
projects.git_id
```

引用它。

v0.2 不追求完美解决“repository 移动后身份恢复”，只要求：

```text
同一个已识别 git common dir
→ 同一个 git_id
```

以后可以增加 remote / object fingerprint evidence。

---

## 5.3 workspace_paths

新增：

```text
workspace_paths

id                 TEXT PRIMARY KEY
canonical_path     TEXT NOT NULL UNIQUE
project_id         TEXT NOT NULL
git_state          TEXT NOT NULL
git_kind           TEXT NULL
exists             INTEGER NOT NULL
first_seen_at      TEXT NOT NULL
last_seen_at       TEXT NOT NULL
```

其中：

```text
id
= path identity 本身
= 由 canonical_path 确定性派生：path-<sha256(canonical_path) 前 32 hex>
```

> ⚠️ v0.2.1 修正（§42.2-E1）：原稿在这里同时要求 `id`、`path_id`、`canonical_path` 三个唯一键，
> 而 §1.1 把 `path_id` 定义为 “canonical path 的稳定身份” —— 那是同一事实的第三份副本。
> `path_id` 列删除；`id` 就是它，且必须确定性派生，因为 `migrate()` 全程 autocommit、
> v12 回填必须可重入（`storage/mod.rs:131-479`）。
> 下游引用（`sessions.workspace_path_id`、`workstream_paths.workspace_path_id`、
> 设计文档 §23 的 `Session.path_id`）指向的都是这个 `id`。

```text
git_state
= none | detected | missing

git_kind
= main | linked | unknown | null
```

`project_id`：

```text
NOT NULL
```

一个 WorkspacePath 必须属于一个 Project。

`git_state=none` 的取值时机（§42.3-M9）：`git` 二进制缺失、超时、非零退出、
dubious ownership 一律记为 `none`，不得冒泡成用户可见错误。

---

## 5.4 workstream_paths

新增：

```text
workstream_paths

id                   TEXT PRIMARY KEY
workstream_id        TEXT NOT NULL
workspace_path_id    TEXT NOT NULL
position             INTEGER NOT NULL
source               TEXT NOT NULL
created_at           TEXT NOT NULL
```

约束：

```text
UNIQUE(workstream_id, workspace_path_id)
UNIQUE(workstream_id, position)
```

source：

```text
user
session
launch
migration
```

---

## 5.5 sessions

新增：

```text
workspace_path_id TEXT NULL
```

迁移完成后的产品 invariant：

```text
所有新发现且拥有 cwd 的 Session
必须具有 workspace_path_id
```

旧历史 Session 如果确实没有 cwd，v12 migration 可以暂时：

```text
workspace_path_id = NULL
```

不得伪造为默认 workspace。

后台后续重新发现真实 cwd 时再修复。

现有：

```text
project_id
```

继续作为 cache。

---

## 5.6 session_workstream_bindings

新增：

```text
workstream_path_id TEXT NULL
```

新 binding：

```text
Session 有 WorkspacePath
→ 必须明确对应 WorkstreamPath
```

旧 binding migration 无法可靠推断时可以为 NULL。

后续重新绑定或路径 reconcile 时逐步 repair。

> ⚠️ v0.2.1 补充（§42.3-M1）：这张表的 PK 是 `(session_id, workstream_id)`
> （`storage/mod.rs:223`），所以一个 Session 对一个 Workstream 只有一行，
> `workstream_path_id` 的含义是“这行绑定由哪条 WorkstreamPath 带来”，
> 匹配是**精确相等**，不是前缀 / 最长匹配。不变式：
> `workstream_path_id IS NULL OR 它命中该 Workstream 的某条 workstream_paths`。
> Session 的 cwd 后来漂移出列表时置 NULL，不自动往用户 Workstream 里追加路径（§42.3-M2）。

---

## 5.7 Workstream

不增加新的 status / trashed_at 字段。

重新规范现有：

```text
lifecycle:
open       → active
completed  → completed
abandoned  → completed

visibility:
normal     → normal
archived   → archived
```

`visibility=archived` 即回收站。

> ⚠️ v0.2.1 修正（§42.2-E2）：`workstreams.lifecycle` 的列默认值是
> `DEFAULT 'open'`（`storage/mod.rs:166`），SQLite 无法用 ALTER 改列默认值，
> 本文件也没有 rebuild-and-copy 先例。因此：**只迁移数据、只改规范 DDL**，
> 升级库的历史列默认值保留为 `'open'`，产品写入路径一律显式带 lifecycle
> （现状已如此）。裸 `INSERT INTO workstreams` 不带 lifecycle 算违例，
> 由 §42.5-T3 的 CI grep 守住。
> `abandoned → completed` 是有损折叠；它的唯一生产者 `merge_workstreams`
> 同步退出注册（§42.2-E10），且它总是同时写 `visibility='archived'`，
> 所以回收站事实仍然保留。

---

# 6. v12 migration 原则

SQL migration 不调用 Git，不运行外部命令。

migration 只建立新结构和已有关系。

> ⚠️ v0.2.1 补充（§42.3-M4/M5）：这条原则带来两个必须写死的后果。
> 1. **迁移结束时 `workspace_paths.project_id` 必须已经非空**（§5.3 NOT NULL），
>    而 Git 还完全没看过。所以迁移建立的 Project 全部是 `git_id=NULL` 的
>    path-backed Project，之后由 Workspace Reconcile 升级或合并——这是设计意图，
>    不是待补的洞；但它意味着**升级后第一次启动会看到比最终态更多的 Project**。
>    UI 上不要在这一次启动里承诺“这就是你的 Project 集合”。
> 2. **迁移不持事务**（`migrate()` 全程 autocommit），且 §7.4 会自动删 Project、
>    连带删用户手工录入的 `project_resources`。所以破坏性分支执行前必须
>    先做整库文件备份（§42.3-M5）。

Git detection 在数据库成功打开后的：

```text
Workspace Reconcile
```

中执行。

---

# 7. v12 数据迁移顺序

## 7.1 保留 legacy Project identity

旧 Project：

```text
id 保留
name 保留
name_customized = true
```

因为旧 Project 都是用户主动创建/命名的，升级后不得被自动命名覆盖。

---

## 7.2 Session cwd

对所有：

```text
session.cwd != null
```

建立 WorkspacePath。

如果旧：

```text
session.project_id != null
```

且没有 identity 冲突：

```text
优先用旧 Project 作为该 WorkspacePath 初始 Project
```

然后：

```text
session.workspace_path_id = path.id
```

---

## 7.3 Workstream.default_cwd

对所有：

```text
default_cwd != null
```

建立 WorkspacePath，并创建：

```text
WorkstreamPath position=0
source=migration
```

如果旧：

```text
workstream.project_id != null
```

且 path 尚未归属其他更强 identity：

```text
用旧 Project 作为初始 Project
```

---

## 7.4 legacy Project without path

如果一个旧 Project 最终没有任何 WorkspacePath：

```text
它不符合 v0.2 Project invariant
```

自动删除。

不要为保存旧 Project 而制造虚假路径。

---

## 7.5 Project Resources

`project_resources` 不再承载 workspace identity。

migration 可以把：

```text
kind=workspace
kind=repository
```

且 URI 明确为本地路径的记录作为 path migration evidence。

HTTP URL / Git URL / document 等不得伪造成 WorkspacePath。

`project_resources` 本阶段暂时保留表结构，但 UI 不再允许用户通过它管理 Project。

---

# 8. Project 自动生成算法

统一入口：

```text
ensure_workspace_path(path)
```

流程：

```text
path
 ↓
canonicalize
 ↓
path_id
 ↓
lookup WorkspacePath
 ↓
detect Git
 ↓
resolve Project
 ↓
persist
```

---

## 8.1 已知 Path + 本次无 Git

```text
WorkspacePath 已存在
Git detection = none / failed
```

保持：

```text
WorkspacePath.project_id
Project.git_id
```

不变。

如果此前：

```text
git_state=detected
```

改为：

```text
git_state=missing
```

---

## 8.2 新 Path + 无 Git

创建：

```text
Project
git_id=null
```

然后：

```text
WorkspacePath → Project
```

非 Git Project 在正常情况下只有一条 WorkspacePath。

---

## 8.3 新 Path + Git

解析：

```text
git common dir
→ git identity
```

如果已有：

```text
Project.git_id = G
```

则：

```text
WorkspacePath → existing Project
```

否则：

```text
create git identity
create Project(git_id=G)
WorkspacePath → Project
```

---

## 8.4 已知 Path 从无 Git 升级为 Git

如果当前 Project：

```text
git_id=null
```

且 G 尚未属于其他 Project：

```text
current Project.git_id = G
```

Project.id 不变。

如果 G 已属于 Project B：

```text
将当前 WorkspacePath / Project 合并进 B 或选定 canonical Project
```

合并后零 WorkspacePath Project 自动删除。

---

## 8.5 已知 Path 检测到不同 Git family

例如：

```text
WorkspacePath → Project A
A.git_id = G1

当前检测 = G2
```

则这是强 identity change。

WorkspacePath：

```text
迁移到 G2 Project
```

如果 G2 Project 不存在：

```text
创建
```

然后：

```text
批量刷新引用该 WorkspacePath 的 Session.project_id
```

旧 Project 若无路径：

```text
自动删除
```

---

# 9. Git worktree discovery

有效 Git Project：

```text
git worktree list --porcelain
```

发现全部 worktree。

每条：

```text
ensure WorkspacePath
```

并归入同一 Project。

但：

```text
发现 WorkspacePath
≠
添加 WorkstreamPath
```

不得自动修改 Workstream 的路径列表。

---

# 10. WorkspacePath GC

WorkspacePath 不因：

```text
路径暂时不存在
.git 丢失
```

立即物理删除。

只有：

```text
不存在 Session reference
AND
不存在 WorkstreamPath reference
AND
已经不再是当前有效/发现的 Project worktree path
```

时允许 GC。

WorkspacePath 删除后：

```text
check Project.workspace_path_count
```

为 0：

```text
DELETE Project
```

---

# 11. Backend API 冻结

主 Agent Commit 0 后，所有 Agent 按以下接口工作。

## Project

保留：

```text
list_projects
get_project_detail
rename_project
```

> ⚠️ v0.2.1 修正（§42.2-E4/E5）：这份清单不完整。
> - `get_project_detail` **今天不存在**（`commands.rs` 里没有，
>   `ProjectDetail.tsx:56-58` 是拉全量列表再 `.find()`），它是新增。冻结形状：
>   `{ project, workspace_paths[], workstreams[{ workstream, is_primary }], sessions[] }`
> - `rename_project` 今天也不存在，实际是整对象写的 `update_project`
>   （`commands.rs:84-91`，且前端零调用点）。`rename_project` 是新增窄命令。
> - **`list_workstreams(project_id)` 必须一起冻结**：它是今天唯一的
>   Workstream→Project 成员查询（`storage/mod.rs:629-647`），前端 4 处在用。
>   它的 `project_id` 参数语义改为路径派生投影（含 position>0），
>   并新增 `list_project_workstreams(project_id)` 提供 §36 的“主关联/关联”。
> - `list_workstream_cards` 的 `project_id` / `project_name` 两列
>   冻结为“position-0 路径所属 Project 的投影”（§42.3-M19），列名列型不变。

退出产品 API：

```text
create_project
delete_project
assign_session_project
suggest_session_project
merge_workstreams
```

可以暂时保留内部 Rust helper，但不得再注册为正常 UI command。

> ⚠️ v0.2.1 补充（§42.2-E10/E11）：
> - `merge_workstreams` 原稿未提及。它是 `abandoned` 的唯一生产者
>   （`commands.rs:440-444`），也是还会写 `workstreams.project_id` 的用户路径，
>   前端零调用点 → 同样退出注册。
> - `record_session_project_evidence`（`ingestion/mod.rs:333-360`）不是 command，
>   但它跑在**每次发现新 Session** 的热路径上（`:250`），用 `Project.name`
>   子串匹配 cwd 写 affinity 证据。v0.2 之后它是假权威，必须停止调用
>   （表与函数保留为只读历史）。`ingestion::suggest_project_for_session`
>   （`:362-366`）已是死代码，一并退出。
> - `project_resources` 的三个 command（add/list/remove）本阶段一并退出注册：
>   §7.5 说 “UI 不再允许用户通过它管理 Project”，而 `ProjectDetail.tsx:189-241`
>   正是那个 UI。表保留，Rust helper 保留。

---

## Workspace settings

新增：

```text
get_workspace_settings
set_noending_home
```

返回至少：

```text
noending_home
default_workspace
pending_home
restart_required
db_path
```

---

## Workstream

`create_workstream` 新签名：

```text
title
description
initial_path?
```

不再接受：

```text
project_id
default_cwd
```

新增：

```text
list_workstream_paths
add_workstream_path
remove_workstream_path
reorder_workstream_paths

set_workstream_lifecycle
archive_workstream
restore_workstream
delete_workstream_permanently
```

---

## Session

保留：

```text
replace_session_bindings
```

但后端行为升级：

```text
新增 binding
→ ensure WorkstreamPath when necessary

删除 binding
→ 不删除 WorkstreamPath
```

移除：

```text
assign_session_project
suggest_session_project
```

---

# 12. PreparedLaunch 契约

现有：

```text
Prepare
→ Preview
→ launch_prepared
```

不能破坏。

Workspace v0.2 增加 stale inputs：

```text
Workstream ordered paths
resolved primary path
NoEnding default workspace
Session workspace path
```

以下任一变化必须使未消费 PreparedLaunch stale：

```text
主路径变化
路径 reorder 导致 position 0 变化
路径删除
Session cwd/path 变化
默认 workspace 变化
```

不能在 launch_prepared 时偷偷吸收变化。

> ⚠️ v0.2.1 补充（§42.3-M16/M17）：今天 `PreparedLaunch.cwd` **完全不在指纹里**，
> 而且 resume 分支更糟——`prepare_resume` 存 `cwd: session.cwd`
> （`launcher/mod.rs:189`），但真正 spawn 用的目录是在 `:368` **重新读库**拿的
> `session.cwd`，而 `session.cwd` 也不是指纹输入（`:526-563` 只哈希
> `last_activity_at` + cursor）。也就是说后台 discovery 在 Preview 与 Launch 之间
> 改写 cwd，就会在用户没看到的情况下换掉启动目录——这是一条**既存的
> Preview-Launch Identity 违例**，不是本阶段新引入的。
> §13/§30 把 fallback 链变成用户可见事实之后必须一起修：
> 1. resume 用 `prepared.cwd` spawn（不再回读库）；
> 2. `cwd` 作为指纹输入，new 与 resume 两种 mode 都进；
> 3. 新增的有序路径列表 / 主路径 / 默认 workspace 三项各自带标签参与哈希
>    （`b"ws_paths:"` / `b"primary:"` / `b"default_ws:"`），**列表保持顺序、不排序**，
>    每个值后加分隔符（现有实现相邻字段直接拼接，无分隔）；
> 4. 默认 workspace 来自 `NoEndingHome`（不在 DB 里），DB-only 的指纹看不见它的变化，
>    所以必须显式把它作为参数传进 `compute_state_fingerprint`。

---

# 13. Launcher cwd resolution

> ⚠️ v0.2.1 补充（§42.3-M21）：`NoEnding default workspace` 这一档在 bootstrap 时
> 必须真的 `create_dir_all` 出来。否则命中既存的跨平台不对称：macOS 的启动脚本
> `cd` 失败会**打印一句提示后回退 `$HOME`**（`platform/launcher.rs:42-53`），
> Linux 直接 spawn 失败（`:268-276`）——“你以为在默认 workspace，实际在 $HOME”，
> 而 `$HOME` 正是 §1.4 要排除的 dotfiles repo。
> 另外 §42.3-M15：共享默认 workspace 会削弱 LaunchIntent 的 cwd 匹配证据，
> 并发独立启动会合理地落入 `AMBIGUOUS` + 人工裁决，不得为此引入猜测。

## Workstream New Session

```text
explicit cwd
    ↓
WorkstreamPaths[0]
    ↓
NoEnding default workspace
```

## Standalone New Session

```text
explicit cwd
    ↓
NoEnding default workspace
```

## Resume

优先：

```text
原 Session.cwd
```

不可用时：

```text
目标 Workstream primary path
    ↓
NoEnding default workspace
```

发生 fallback 必须在 UI 明确显示。

---

# 14. 多 Agent 执行结构

整个阶段分三轮。

```text
Wave 0
Main Foundation

Wave 1
A + B + C + D 并行

Wave 2
E + F 并行

Wave 3
G Integrity Audit

Main Final Integration
```

> ⚠️ **所有 Agent（含 Main）开工前必读 §42。** §1–§41 是原始设计意图，
> §42 是 Main 对着 `1da873e` 逐条核对后的勘误（E）、缺失规则（M）、
> 执行结构修正和机械断言（T）。两处冲突时以 §42 为准，因为它有 file:line 依据。
> Wave 2 的前端拆分见 §42.4——§22 里“Agent F 独占全部前端”的写法已作废。

---

# 15. Main Agent — Wave 0 Foundation

Main 不把这一阶段一开始就分出去。

Main 先完成一个 foundation commit。

建议：

```text
refactor(workspace): establish workspace domain v0.2 contracts
```

职责：

1. 将本方案写入 docs。
2. `SCHEMA_VERSION 11 → 12`。
3. 建立新表 / 新 columns。
4. 写纯数据库 migration。
5. 在 `domain.rs` 定义所有新领域类型。
6. 建立模块骨架。
7. 冻结 Rust service/API signatures。
8. 冻结 TS shapes，但暂不做 UI。
9. 建立共享测试 fixture/helper。
10. 确保旧代码仍能编译。
11. **改写 `AGENTS.md` 的 Core Invariants**，让 Project/WorkspacePath/WorkstreamPath
    成为文档化的唯一权威（§42.1）。AGENTS.md 是本仓库每个 Agent 的第一份输入，
    它若还写着 “Never equate Project with … filesystem path” 和
    “default_cwd 永不是身份”，下一个 Agent 会用旧 invariant 否决新代码。
12. 加**迁移前整库文件备份**（§42.3-M5）。
13. 结构性切断旧列写入：`upsert_workstream_conn` 的 `ON CONFLICT DO UPDATE`
    去掉 `project_id` / `default_cwd`（§42.2-E6）。
14. 给 `Workstream` / `Session` / `WorkspacePath` 提供共享测试构造 helper，
    并机械修完 18 个集成测试文件的 struct literal（§42.3-M20）。

建议模块结构：

```text
src-tauri/src/workspace/
├─ mod.rs
├─ home.rs
├─ resolver.rs
├─ project.rs
├─ workstream.rs
└─ session.rs

src-tauri/src/storage/
├─ workspace.rs
├─ workstream_paths.rs
└─ session_paths.rs

src-tauri/src/commands/
├─ project.rs
├─ workspace.rs
├─ workstream.rs
└─ session_workspace.rs
```

Main 独占共享 wiring 文件：

```text
src-tauri/src/storage/mod.rs
src-tauri/src/domain.rs
src-tauri/src/commands.rs
src-tauri/src/lib.rs
```

子 Agent 不修改这些文件，除非 Main 明确授权。

Foundation 完成后：

```text
cargo fmt --check
cargo check
cargo test
pnpm build
```

全部通过。

记录：

```text
FOUNDATION_SHA=<exact sha>
```

随后所有 Wave 1 Agent 必须从这个 SHA 建 worktree。

不得从旧 HEAD 自行开分支。

---

# 16. Agent A — NoEnding Home + WorkspaceResolver

分支：

```text
agent/workspace-home-resolver
```

所有权：

```text
workspace/home.rs
workspace/resolver.rs
platform/paths.rs
相关 resolver unit tests
```

不负责 Project DB mutation。

输入：

```text
filesystem path
```

输出：

```text
WorkspaceObservation {
  canonical_path,
  path_id,
  exists,
  git_detection,
}
```

GitDetection 至少：

```text
none
detected {
  common_dir,
  worktree_kind,
  worktrees[]
}
missing
```

任务：

1. 实现 `~/.noending` 默认 Home。
2. 实现 bootstrap current/pending home。
3. 实现启动前 data-root migration helper。
4. reserved app paths 排除。
5. default workspace resolution。
6. path canonicalization / path_id。
7. Windows/macOS path normalization。
8. Git common-dir detection。
9. `~/.git` 排除。
10. `git worktree list --porcelain` parsing。
11. `.git` disappeared → missing observation。
12. 不直接创建 Project。

必须测试：

```text
~ expansion
relative → absolute
same path normalization
Windows separator/case behavior
home .git exclusion
normal git repo
linked worktree
.git missing
reserved ~/.noending/data exclusion
~/.noending/workspace allowed
```

提交：

```text
feat(workspace): add noending home and workspace resolver
```

交付 Main：

```text
commit SHA
public interfaces
tests run
known edge cases
```

---

# 17. Agent B — Project Projection + WorkspacePath Registry

分支：

```text
agent/project-projection
```

所有权：

```text
workspace/project.rs
storage/workspace.rs
commands/project.rs
```

只消费 Agent A 已冻结的 `WorkspaceObservation` interface，不修改 resolver。

任务：

1. `ensure_workspace_path()`。
2. WorkspacePath persistence。
3. Project auto-create。
4. Project.git_id ownership。
5. git identity persistence。
6. path-backed → git-backed upgrade。
7. Project merge。
8. WorkspacePath reassignment。
9. `.git missing` continuity。
10. zero-path Project auto-delete。
11. WorkspacePath GC。
12. `list_projects`。
13. `get_project_detail` projection。
14. `rename_project`。
15. user rename → `name_customized=true`。
16. default workspace 自动命名 `NoEnding Workspace`。
17. Project merge 时 custom name precedence。

不得实现：

```text
create_project UI semantics
manual Project assignment
Workstream paths
Session bindings
```

必须测试：

```text
new non-git path → one Project
same path ensure idempotent
new git path → git Project
two worktrees → same Project
path project upgrades to git Project without changing Project.id
.git missing does not detach path
different git family moves path
merge deletes zero-path Project
customized Project name survives merge/reconcile
one WorkspacePath never belongs to two Projects
zero-path Project cannot survive
```

提交：

```text
feat(projects): derive projects from workspace identities
```

---

# 18. Agent C — Workstream Paths + Lifecycle + Trash

分支：

```text
agent/workstream-domain
```

所有权：

```text
workspace/workstream.rs
storage/workstream_paths.rs
commands/workstream.rs
相关 Workstream integration tests
```

任务：

1. ordered WorkstreamPaths。
2. position invariant。
3. add path。
4. remove path。
5. reorder path。
6. 删除第一个 → 第二个自动成为 position 0。
7. `create_workstream(initial_path?)`。
8. lifecycle：

   ```text
   active | completed
   ```
9. archived 作为 recycle bin。
10. restore。
11. permanent delete。
12. permanent delete 不删除 Session。
13. permanent delete 清理：

    ```text
    workstream_paths
    session bindings
    Workstream-owned Context/Review data
    ```
14. 更新受影响 PreparedLaunch fingerprint input interface。
15. Workstream Project projection 从 paths 派生，不写 `workstreams.project_id`。

`default_cwd`：

```text
只作为 v12 migration 输入
```

新代码不再作为 authority 使用。

必须测试：

```text
empty paths valid
first path automatically primary
append secondary
remove first promotes second
remove middle compacts positions
reorder is deterministic
duplicate path rejected/idempotent
active ↔ completed changes no other data
archive preserves lifecycle/paths/bindings
restore preserves previous lifecycle
permanent delete only allowed archived
permanent delete preserves Sessions
```

提交：

```text
feat(workstreams): add ordered paths and recycle lifecycle
```

---

# 19. Agent D — Session Workspace + Binding Semantics

分支：

```text
agent/session-workspace
```

所有权：

```text
workspace/session.rs
storage/session_paths.rs
commands/session_workspace.rs
ingestion 中 Session discovery/path attach 的最小必要区域
replace bindings 相关实现
相关 Session integration tests
```

任务：

1. Session discovery：

   ```text
   cwd → WorkspacePath
   ```
2. 写：

   ```text
   session.workspace_path_id
   ```
3. 从 WorkspacePath 派生：

   ```text
   session.project_id
   ```
4. WorkspacePath.project_id 变化时支持批量刷新 Session cache。
5. 普通 event ingestion 不重复 resolve Project。
6. `replace_session_bindings` 新语义。
7. Session 加入 Workstream：

   * path 已存在 → 使用现有 WorkstreamPath。
   * 无路径 → append position 0。
   * 已有路径 → append last。
8. binding 记录：

   ```text
   workstream_path_id
   ```
9. Session unbind 不删除 WorkstreamPath。
10. 兼容 legacy `workstream_path_id=NULL`。
11. 旧 Session 无 cwd 不制造假 WorkspacePath。
12. 去除 manual Session Project assignment 的生产依赖。

必须测试：

```text
discovered Session gets WorkspacePath
Session gets derived Project
standalone Session still gets Project
WorkspacePath Project change refreshes all Sessions
event ingestion alone does not change Project
bind Session adds missing WorkstreamPath
first Session path becomes primary
later Session path appends secondary
unbind Session preserves path
binding records exact workstream_path_id
legacy cwd-less Session remains unresolved instead of default-workspace fabrication
```

提交：

```text
feat(sessions): derive workspace and project membership from paths
```

---

# 20. Wave 1 集成

Main 按顺序：

```text
A
→ B
→ C
→ D
```

cherry-pick。

原因：

```text
A 提供 physical resolver
B 提供 WorkspacePath / Project authority
C/D 消费这两层
```

Main 负责所有共享 wiring：

```text
lib.rs
commands.rs
domain.rs
storage/mod.rs
```

以及因 API freeze 导致的少量 glue。

Wave 1 完成后必须：

```text
cargo fmt --check
cargo check
cargo test
```

全部通过。

记录：

```text
WAVE1_SHA=<exact sha>
```

Agent E / F 必须从 `WAVE1_SHA` 新建 worktree。

---

# 21. Agent E — Launcher + PreparedLaunch Integration

分支：

```text
agent/workspace-launcher
```

所有权：

```text
launcher/mod.rs
launch cwd helpers
PreparedLaunch fingerprint logic
launch_cwd_test.rs
base_launch_flow_test.rs
相关 launcher tests
```

任务：

1. New Workstream Session cwd：

   ```text
   explicit cwd
   → primary WorkstreamPath
   → default workspace
   ```
2. Standalone：

   ```text
   explicit cwd
   → default workspace
   ```
3. Resume：

   ```text
   Session.cwd
   → Workstream primary
   → default workspace
   ```
4. fallback 明确进入 PreparedLaunch 数据，供 UI 显示。
5. path reorder / primary change stale PreparedLaunch。
6. path deletion stale。
7. default workspace change stale。
8. Session workspace path change stale Resume plan。
9. launch 成功后不要提前制造 Session。
10. 真实 Session 被 ingestion / LaunchIntent 匹配后：

    ```text
    bind Workstream
    ensure WorkstreamPath
    derive Project
    ```
11. 保持：

    ```text
    Preview content = exact Agent content
    ```

    现有 integrity contract 不变。

不得：

```text
恢复旧 launchNewSession frontend path
绕过 prepare → launch_prepared
```

必须测试：

```text
primary path wins
no Workstream path → default workspace
standalone → default workspace
explicit cwd wins
reorder primary makes plan stale
remove primary makes plan stale
resume uses original cwd
resume fallback visible
launch does not create phantom Session/path before discovery
matched real Session creates binding/path correctly
```

提交：

```text
feat(launcher): resolve sessions through workspace paths
```

---

# 22. Agent F — Frontend Workspace UX

分支：

```text
agent/workspace-ui
```

Agent F 独占：

```text
src/api.ts
src/types.ts
src/features/projects/**
src/features/workstreams/**
相关 Sessions UI
Settings workspace/data UI
```

本 Agent 不改 Rust。

## Project UI

Projects：

```text
无“新建 Project”
```

Project Detail：

```text
名称
Git 状态（派生展示）
Workspace Paths
Workstreams
Sessions
```

用户唯一 Project edit：

```text
重命名
```

移除：

```text
新建 Project
删除 Project
手工加 Project Resource
手工移动 Workstream
手工移动 Session
```

---

## Workstream UI

Workstream Detail 增加：

```text
工作路径
```

表现为 ordered list：

```text
主工作路径
/path/A

其他工作路径
/path/B
/path/C
```

支持：

```text
添加
移除
设为主路径 / reorder
```

删除路径前明确提示：

```text
将同时把该路径对应的 N 个 Session 从当前 Workstream 移除。
Session 历史不会删除。
```

Workstream 状态：

```text
进行中
已完成
```

菜单：

```text
移入回收站
```

回收站：

```text
恢复
永久删除
```

---

## New Workstream

移除：

```text
Project selector
```

改为：

```text
标题
描述
初始工作路径（可选）
```

---

## Sessions

Session Detail：

```text
显示 WorkspacePath
显示 Project
```

Project 只读。

绑定 Workstream 时：

```text
不让用户操作 Project
```

---

## Settings

增加：

```text
NoEnding Home
默认 Workspace
数据库路径
```

修改 Home：

```text
选择新路径
→ 显示“重启后迁移并生效”
```

明确提示：

```text
旧 workspace 中的用户文件不会被移动。
```

---

## UI copy

继续：

```text
简体中文为主
Project / Workstream / Session / Agent 等领域词保留英文
```

提交：

```text
feat(ui): make workspace projects automatic
```

---

# 23. Wave 2 集成

Main：

```text
E
→ F
```

E 先合，因为 Launcher 是业务 contract。

F 只消费最终 API。

之后运行：

```text
cargo fmt --check
cargo check
cargo test
pnpm build
```

记录：

```text
WAVE2_SHA=<exact sha>
```

---

# 24. Agent G — Integrity / Migration / Cross-platform Audit

Agent G 必须从：

```text
WAVE2_SHA
```

开始。

分支：

```text
agent/workspace-integrity
```

Agent G 不是功能开发 Agent。

职责：

```text
找 invariant 违例
补测试
只修明确 bug
不扩大产品范围
```

新增建议：

```text
src-tauri/tests/workspace_identity_test.rs
src-tauri/tests/project_projection_test.rs
src-tauri/tests/workstream_paths_test.rs
src-tauri/tests/workspace_migration_test.rs
src-tauri/tests/session_workspace_test.rs
src-tauri/tests/workspace_launch_flow_test.rs
```

---

# 25. Agent G 必测矩阵

## Project

```text
WorkspacePath exactly one Project
Project always >=1 WorkspacePath
zero-path Project auto-deleted
non-null git_id unique
path-backed → git-backed keeps Project.id
worktrees converge into one Project
```

## Git

```text
.git deletion only changes WorkspacePath git_state
Project.git_id survives
WorkspacePath.project_id survives
.git recovery restores detected
different new Git family can reassign path
~/.git ignored
```

## Workstream Paths

```text
ordered
position 0 primary
remove first promotes second
no secondary-without-primary state
add path does not import Sessions
remove path removes only matching bindings
```

## Sessions

```text
Session Project derived from WorkspacePath
standalone Session has Project
WorkspacePath reassignment batch-refreshes Sessions
manual project assignment impossible
```

## Lifecycle

```text
active ↔ completed no functional mutation
archive retains lifecycle
restore retains lifecycle
permanent delete only archived
Session history survives
```

## Launcher

```text
primary path resolution
default workspace fallback
PreparedLaunch stale correctness
```

## Home migration

```text
legacy app-data DB → ~/.noending/data
migration occurs before DB open
pending home applies on restart
old workspace not moved
new workspace becomes default
failed migration does not switch bootstrap pointer
```

## v11 → v12

构造真实 v11 fixture。

验证：

```text
old Project ids/names preserved where paths exist
legacy names marked customized
default_cwd → WorkstreamPaths[0]
session cwd → WorkspacePath
legacy project_id used only as initial migration evidence
Git reconcile can subsequently merge Projects
pathless legacy Project removed
archived Workstream still restores previous lifecycle
```

---

# 26. Migration 必须具有可证伪性

至少实现以下 regression：

```text
migration_is_idempotent

git_loss_does_not_split_project

path_to_git_upgrade_preserves_project_id

worktrees_merge_path_backed_projects

removing_primary_promotes_second_path

removing_workstream_path_unbinds_only_sessions_owned_by_that_path

workspace_project_change_refreshes_session_cache

archive_restore_preserves_workstream_state

permanent_delete_preserves_session_history

changing_noending_home_does_not_move_old_workspace
```

---

# 27. Agent G 输出格式

Agent G 最终必须提交：

```text
commit SHA

P0
P1
P2

tests added
tests passed
remaining known issues
```

若发现 P0/P1：

```text
直接修复 + test
```

若只是 P2 polish：

```text
报告 Main
不要扩大实现
```

建议提交：

```text
test(workspace): lock workspace domain v0.2 invariants
```

---

# 28. Main 最终集成任务

Main 合入 G 后进行最终代码审查。

重点 grep / 搜索：

```text
create_project
delete_project
assign_session_project
suggest_session_project
merge_workstreams
update_project
record_session_project_evidence
resolve_project_affinity
project_affinity_evidence
add_project_resource
list_project_resources
remove_project_resource

workstream.project_id
default_cwd

current manual Project select UI
```

要求：

```text
不再出现在新的生产业务路径
```

Legacy schema / migration code中出现允许。

> ⚠️ v0.2.1 补充：上面新增的 8 个搜索词都来自 §42.2-E4/E5/E10/E11 的实测——
> `update_project` 是整对象写（`commands.rs:84-91`），`record_session_project_evidence`
> 在 ingestion 热路径上（`ingestion/mod.rs:250`），`project_resources` 三个命令
> 是唯一还让用户手工挂路径的入口（`ProjectDetail.tsx:189-241`）。
> 机械断言见 §42.5（T1–T4），不只靠人眼 grep。

另外必须确认的三条“结构性防漂移”（§42.2-E6、§42.3-M3）：

```text
upsert_workstream_conn 的 ON CONFLICT DO UPDATE 集合里没有 project_id / default_cwd
UPDATE sessions SET project_id 只出现在派生与批量刷新两处
lib.rs 的 generate_handler! 里没有已退出注册的命令
```

---

# 29. 旧 authority 清理

Main 必须确认新的唯一 authority：

```text
Physical path
→ WorkspacePath

Project identity
→ Project.git_id + owned WorkspacePaths

Workstream cwd
→ ordered WorkstreamPaths

Session workspace
→ Session.workspace_path_id

Session Project
→ WorkspacePath.project_id
```

禁止出现：

```text
Session.project_id 与 WorkspacePath.project_id 各自独立写入

Workstream.project_id 与 WorkstreamPaths 各自独立写入

default_cwd 与 WorkstreamPaths 双 authority
```

---

# 30. 允许暂时保留的 legacy 字段

v0.2 可以物理保留：

```text
projects.archived
projects.description
workstreams.project_id
workstreams.default_cwd
```

但只能：

```text
migration read
compatibility read
```

禁止新业务写入。

未来 schema v13+ 再物理删除。

`sessions.project_id` 例外：

```text
允许写
但只能由 WorkspacePath projection 自动维护
```

---

# 31. Context / Assistant 兼容要求

本阶段：

```text
Context Intelligence default Off
Context Delivery default Off
Assistant hidden/experimental
```

全部保持。

不得重新设计：

```text
Context extraction
Context review
Context delivery
Assistant scope
```

如果 Context frozen code 依赖：

```text
workstream.project_id
default_cwd
visibility/lifecycle
```

Main 只做最薄 compatibility adapter。

不要为了本阶段重构 Context Domain。

---

# 32. CI Gate

最终必须通过：

```text
cargo fmt --check
cargo check
cargo test

pnpm build
```

> ⚠️ v0.2.1 修正（§42.2-E9）：`pnpm lint` **在本仓库不存在**——
> `package.json` 只有 `dev` / `build` / `tauri` 三个脚本，仓库里没有 eslint
> 配置。CI（`.github/workflows/ci.yml`）只有一个 job，矩阵为
> `macos-latest + windows-latest`，步骤是 `cargo check --all-targets`、
> `cargo test --all-targets`、`pnpm install --frozen-lockfile`、`pnpm build`；
> 没有 clippy，也没有 fmt。所以：`cargo fmt --check` 由 Main 每轮集成本地跑；
> **任何会调用 `git` 的测试必须在 Windows runner 上真的通过**（见 §42.3-M8/M9），
> 不能只在 macOS 上验证。

必须验证 GitHub Actions：

```text
run head_sha == final HEAD
status = completed
conclusion = success
```

不能用旧 commit 的绿色 CI 代替。

---

# 33. 手工 Dogfood Gate

Main 最终至少手工验证以下路径：

### Case A — 普通目录

```text
新建 Workstream
→ 选普通目录
→ 自动出现 Project
→ Workstream 出现在 Project
```

### Case B — Git repo

```text
新建 Workstream
→ 选择 repo
→ 自动 Project
```

### Case C — Git worktree

```text
另一个 Workstream
→ 选择同 repo worktree
→ 不产生第二个 Project
```

### Case D — Standalone Session

```text
New Session
无 Workstream
→ 默认 ~/.noending/workspace
→ Session 有 Project
```

### Case E — Workstream 无路径

```text
Workstream paths=[]
→ New Session
→ 默认 workspace
→ Session 真正出现后
→ 默认 workspace 成为 Workstream position 0
```

### Case F — Session 手工加入 Workstream

```text
Session cwd 不在 Workstream
→ bind
→ path append
→ Project projection 更新
```

### Case G — 删除主路径

```text
A / B / C
→ 删除 A
→ B 自动主路径
→ A Sessions 从 Workstream unbind
→ B/C Sessions 不受影响
```

### Case H — Trash

```text
active → archived → restore
→ active

completed → archived → restore
→ completed
```

### Case I — .git disappearing

```text
Git Project
→ 删除一个 worktree .git
→ Project 不拆
→ path 不换 Project
```

---

# 34. 建议最终 commit 序列

最终历史建议保持类似：

```text
1. docs(workspace): freeze workspace domain v0.2
2. refactor(workspace): establish workspace domain v0.2 contracts
3. feat(workspace): add noending home and workspace resolver
4. feat(projects): derive projects from workspace identities
5. feat(workstreams): add ordered paths and recycle lifecycle
6. feat(sessions): derive workspace and project membership from paths
7. feat(launcher): resolve sessions through workspace paths
8. feat(ui): make workspace projects automatic
9. test(workspace): lock workspace domain v0.2 invariants
10. fix(workspace): integration audit fixes        // only if needed
11. docs(workspace): record workspace domain v0.2 final state
```

不要为了追求 commit 数量强行 squash 各 Agent 的独立完整提交。

---

# 35. Worktree / Agent 执行纪律

上一阶段曾发生子 Agent 从旧 base 建 worktree 的问题，本阶段明确禁止。

Main：

```text
git rev-parse HEAD
```

保存 foundation SHA。

每个 Agent 的 worktree 必须：

```text
git worktree add \
  -b agent/<name> \
  <worktree-path> \
  <EXACT_BASE_SHA>
```

Agent 开工第一条检查：

```text
git rev-parse HEAD
```

必须等于 Main 给出的 SHA。

不等：

```text
立即停止
```

不得自己“猜正确 base”。

---

# 36. Agent 不自行同步 Main

Agent 工作期间：

```text
不 merge main
不 rebase main
不 cherry-pick 其他 Agent
```

跨 Agent dependency：

```text
由 Main 在 Wave 边界集成
```

这样每个提交的变更范围可审计。

---

# 37. Agent 交付格式统一

每个 Agent 最终回复 Main：

```text
Agent:
Base SHA:
Commit SHA:

Changed files:
- ...

Implemented:
- ...

Tests:
- command
- result

Known issues:
- ...

Out-of-scope observations:
- ...
```

禁止只回复：

```text
“完成了”
```

Main 必须根据 commit 实际 diff 独立复核。

---

# 38. 文件 ownership 原则

共享高冲突文件由 Main 独占：

```text
storage/mod.rs
domain.rs
commands.rs
lib.rs
Cargo.toml / lockfile
```

Agent 必须优先新增：

```text
独立 module
独立 storage impl
独立 command submodule
```

而不是不断向 monolith 追加。

前端由 Agent F 一次性负责，以避免多个 Agent 同时修改：

```text
api.ts
types.ts
shared modal
Project / Workstream / Session UX
```

---

# 39. 阶段完成定义

只有全部满足才可标记：

```text
Workspace Domain v0.2 — SEALED
```

完成条件：

```text
NoEnding Home                      ✅
Default workspace                  ✅
WorkspacePath registry             ✅
Path identity                      ✅
Git identity                       ✅
~/.git exclusion                   ✅
Git worktree discovery             ✅
.git loss continuity               ✅
Project auto generation            ✅
Project auto merge                 ✅
Zero-path Project auto deletion    ✅
Project rename only                ✅

Ordered Workstream Paths           ✅
Primary = position 0               ✅
Path reorder/remove semantics      ✅
Active / Completed                 ✅
Recycle bin / Restore              ✅
Permanent Workstream deletion      ✅

Session → WorkspacePath            ✅
Session → Project projection       ✅
Standalone Session Project         ✅
Binding path ownership             ✅

Launcher cwd resolution            ✅
PreparedLaunch integrity           ✅

v11 → v12 migration                ✅
Old user data preserved            ✅

macOS                              ✅
Windows                            ✅

cargo tests                        ✅
frontend build                     ✅
exact-head remote CI               ✅
```

---

# 40. 明确不进入本阶段

以下全部留到以后：

```text
Context Intelligence v0.2
Context injection redesign
Assistant
自动语义 Workstream 分类
用户自定义 Workstream Collection / Category
智能 Collection 推荐
自动把路径历史 Sessions 导入 Workstream
复杂 Git remote identity
跨机器 Project identity
GitHub integration
Project-level semantic context
```

尤其不能因为已经有 Git identity 就顺手扩成：

```text
GitHub Project manager
repository browser
branch manager
worktree manager
```

本阶段只负责：

```text
识别
归纳
关联
保持完整性
```

---

# 41. 主 Agent 最终执行原则

优先级：

```text
1. Data integrity
2. Single source of truth
3. Migration safety
4. Deterministic behavior
5. Cross-platform correctness
6. UX
7. Polish
```

遇到歧义时不得用启发式智能猜测。

基础规则：

```text
可以确定
→ 自动处理

不能确定
→ 保持已有关系 / 不做破坏性变化

永远不要为了“看起来更智能”
破坏路径、Project、Session 的历史连续性
```

本阶段完成后，NoEnding 的基础数据链应稳定为：

```text
Filesystem
    ↓
WorkspacePath
    ↓
Project

User intent
    ↓
Workstream
    ↓
Session

WorkstreamPath
负责把用户工作语义映射到物理 Workspace。
```

这套模型完成并经过实际 dogfood 后，再进入用户自定义 Workstream 分类以及后续智能化阶段。

---

# 42. 勘误、补漏与执行修正（v0.2.1，Main 对代码逐条核对后写回）

核对基线：`main@1da873e`（Core Workspace Experience v0.1 封板 HEAD）。
所有 file:line 都按该 HEAD 实测，不是推测。

## 42.1 优先级裁定：本方案 > AGENTS.md

**本方案与 `AGENTS.md` 的 Core Invariants 有两条正面冲突，用户 2026-09-19 明确裁定：以本方案为准。**

| AGENTS.md 原文 | 本方案 | 裁定 |
| --- | --- | --- |
| “Never equate Project with repository, cwd, workspace, or filesystem path” | §0 `Project = NoEnding 自动维护的物理 workspace family`；§1.1 WorkspacePath→Project 唯一事实链 | Project 的物理锚点就是 WorkspacePath。AGENTS.md 该条改写为：Project 是**应用派生**的物理 workspace family，用户不得手工赋值；它仍然不是 repository 的同义词（一个 Project 可以有 N 条路径，其中部分不是 repo） |
| “A Workstream may carry an optional **default working directory** (`default_cwd`) … It is never identity” | §1.5/§5.7/§7.3：`default_cwd` 只作 v12 迁移输入，被有序 `workstream_paths` 取代 | `default_cwd` 退出。`AGENTS.md:25` 整段替换为有序 WorkstreamPath 契约，并保留原句仍然成立的部分：**Workstream 本身仍然不是路径**，Session 仍然保留自己的权威 cwd |

执行要求：**Wave 0 foundation commit 必须同时改 `AGENTS.md`**。不允许出现“代码已经是 WorkspacePath 派生 Project，而 AGENTS.md 还写着 Project 与路径无关”的状态——那会让下一个 Agent 按旧 invariant 否决新代码。

不冲突、本阶段原样保留的 invariant：raw Agent session 文件只读；ingested events append-only；事件身份 app-owned；`processed_cursor` 只在原子 Sync 提交后前进；Sync 全有或全无；`user_explicit`/`user_edit` 不被静默覆盖；Context Delivery 只管出口；Launch Preparation Integrity；Preview-Launch Identity；Home 不推进 ReviewState；macOS/Windows first-class。

## 42.2 与代码现状不符的陈述（就地已修正的原文见括注）

**E1｜§5.3 同一行三个唯一键。** `workspace_paths` 同时要 `id` UNIQUE、`path_id` UNIQUE、`canonical_path` UNIQUE，而 §3 定义 `path_id = canonical path 的稳定身份`——`canonical_path` 本身已经是唯一键，`path_id` 是第三份同一事实。
修正：**删掉 `path_id` 列**，`workspace_paths.id` 直接就是 path identity，由 `canonical_path` **确定性派生**（`path-<sha256(canonical_path) 前 32 hex>`）。这不是省列，是为了 §42.3-M5 的可重入性：`migrate()` 全程 autocommit（`storage/mod.rs:131-479` 无任何事务），确定性主键让 `ensure_workspace_path` 和 v12 回填的重复执行天然幂等。
`sessions.workspace_path_id` / `workstream_paths.workspace_path_id` 引用的就是这个 `id`。
`git_identities.id` 反过来**不**做确定性派生（设计文档 §8 明确 “Git ID 不是 path hash”），它的幂等靠 `git_identities.common_dir UNIQUE` + `INSERT OR IGNORE` 后回查。

**E2｜§5.7 `open → active` 改不了 DDL 默认值。** `workstreams.lifecycle TEXT NOT NULL DEFAULT 'open'`（`storage/mod.rs:166`）。SQLite 无法用 ALTER 改列默认值，`CREATE TABLE IF NOT EXISTS` 对已存在的表无效，而 `storage/mod.rs` **没有任何 rebuild-and-copy 先例**（v1–v11 全部是 `CREATE TABLE IF NOT EXISTS` + 幂等 ALTER 列表 + `IF current_version < N` 回填）。重建 `workstreams` 会牵动 `session_workstream_bindings`、`context_items`、`context_conflicts`、`context_deliveries`、`workstream_review_state` 五个 FK 引用方，在 `PRAGMA foreign_keys=ON`（`:111`）下是一次高危手术。
修正：**不做表重建**。`UPDATE workstreams SET lifecycle='active' WHERE lifecycle='open'` 迁移数据；所有 Rust 写入点显式带 lifecycle（现状已经如此：`create_workstream` `commands.rs:157`、`upsert_workstream_conn` `:2134-2139` 都显式列出该列）；把**规范 DDL 里的 DEFAULT 改为 `'active'`**（新库正确），并在 `migrate()` 上方注释里写清“升级库的列默认值仍是历史 `'open'`，任何绕过 Rust 写入层的裸 INSERT 必须显式带 lifecycle”。补一条回归测试断言：裸 INSERT 不带 lifecycle 的老写法不会在产品路径中出现（用 §42.5-T3 的 grep 断言而非 schema 断言）。

**E3｜§5.1 `archived` “物理保留兼容”仍然被读。** `list_projects` 有 `WHERE archived = 0`（`storage/mod.rs:535`），`stats()` 有 `COUNT(*) … WHERE archived = 0`（`:2099`）。v0.2 之后没有任何生产者再写 `archived=1`，于是历史 `archived=1` 的 Project 会变成“仍然拥有 WorkspacePath、但永远不出现在列表里”的幽灵。
修正：v12 迁移执行 `UPDATE projects SET archived = 0`（Project 没有生命周期，§1.2），并且 `list_projects` / `stats()` **去掉 archived 过滤**。用户会看到自己从前归档过的 Project 重新出现——这是本方案的必然结果（它的路径还在），必须在 §43 落地记录里作为**可见行为变化**写明。

**E4｜§11 Project API 冻结清单漏了成员查询。** `list_workstreams(project_id)`（`storage/mod.rs:629-647`，`WHERE project_id = ?1` 在 `:632`）是当前**唯一**的 Workstream→Project 成员查询，前端 4 处在用（`ProjectsView.tsx:25`、`ProjectDetail.tsx:61`、`SessionDetailView.tsx:306`、`NewSessionModal.tsx:81`、`CommandPalette.tsx:43`）。§11 只冻结了 `list_projects` / `get_project_detail` / `rename_project`。
修正：§11 增加两行——`list_workstreams` 的 `project_id` 参数语义改为**路径派生投影**（`workstream_paths → workspace_paths.project_id`，含 position>0），并新增 `list_project_workstreams(project_id)` 返回 `(workstream, is_primary)`，供 §36 Project Detail 的“主关联/关联”用。

**E5｜`get_project_detail` 从来没有存在过。** §11 把它列为“保留”，但 `commands.rs` 里没有这个命令，`ProjectDetail.tsx:56-58` 是**拉全量 Project 列表再 `.find()`**。
修正：§11 标注 `get_project_detail` 为**新增**，并冻结形状：`{ project, workspace_paths[], workstreams[{workstream, is_primary}], sessions[] }`。前端仍然自己拉列表的写法由 F 换掉。

**E6｜§18-15 “不写 `workstreams.project_id`”缺机制。** `upsert_workstream_conn`（`storage/mod.rs:2134-2139`）无条件写 `project_id = ?2 … default_cwd = ?7`，而 `update_workstream`（`commands.rs:173-180`）是**整对象写**，`WorkstreamDetailView.tsx:123-135` 对每次改名/改描述都重发 `{...workstream, ...patch}`。所以旧列不会“stale-but-harmless”，它会被持续重新提交——正是 §29 禁止的双权威。
修正：**在存储层结构性切断**——`upsert_workstream_conn` 的 INSERT 列保留、`ON CONFLICT DO UPDATE` 集合里**移除** `project_id` 与 `default_cwd`，两列变成“创建时写入、之后不可变”的兼容读列。`Domain` 上 `Workstream::project_id` / `default_cwd` 保留为只读投影，任何新写入点在类型上就不存在。同理 `create_workstream` 新签名不再接收它们。

**E7｜§22 单个前端 Agent 的规模超上限。** 上一阶段同量级的全量前端 Agent 在 150 turn 上限处死掉，留下 21 个已 stage 未 commit 的文件由 Main 代劳。`src/` 现在 7849 行，§22 把 Projects / Workstreams / Sessions / Settings / api.ts / types.ts 全给了 F。
修正：见 §42.4 的 Wave 2 重划。**Main 先落一个 TS bridge commit 独占 `api.ts` + `types.ts`**，F1/F2 只在互不相交的目录里工作。

**E8｜§14/§35 worktree 由谁建没有说。** 上一阶段 5 个 Agent 的 worktree **全部**从 `b1b8efe`（陈旧 base）建出来。§35 只写了“Agent 开工第一条检查 `git rev-parse HEAD`”，但 `isolation: worktree` 的 base 由 harness 决定，Agent 无法控制。
修正：**所有 worktree 由 Main 亲手 `git worktree add -b agent/<name> .qoder/worktrees/<name> <EXACT_SHA>` 建好**，Agent 不再使用 `isolation`，改为直接在给定的绝对路径里工作；开工检查变成“路径下 `git rev-parse HEAD` 必须等于 Main 在 brief 里写死的 SHA，不等立即停止并回报”。`.gitignore` 已含 `.qoder/`（`1da873e` 前已加）。

**E9｜§32 的 CI 现实。** `.github/workflows/ci.yml` 只有一个 job、矩阵 `macos-latest + windows-latest`、步骤只有 `cargo check --all-targets` / `cargo test --all-targets` / `pnpm install --frozen-lockfile` / `pnpm build`。**没有 clippy，没有 `pnpm lint`（`package.json` 只有 dev/build/tauri 三个脚本，仓库无 eslint 配置）**。
修正：§32 的 “如果仓库存在 lint” 一句判定为不存在，删除该分支要求；`cargo fmt --check` 由 Main 在每轮集成本地跑（AGENTS.md 已要求），不进 CI。所有会调用 `git` 的测试必须在 **Windows runner 上通过**，见 §42.3-M8/M9。

**E10｜§11 没有处置 `merge_workstreams`。** 它是 `abandoned` 的**唯一**生产者（`commands.rs:440-444` 同时写 `lifecycle="abandoned"` + `visibility="archived"`），也是唯一还会写 `workstreams.project_id` 的用户路径之一，而前端**零调用点**（`api.ts:34-35` 无组件引用）。§11 的冻结清单和 §28 的 grep 清单都没有它。
修正：`merge_workstreams` 按 §11 “退出产品 API” 的同一处理——**从 `lib.rs` 反注册，保留 Rust helper 不删**，`api.ts` 的 wrapper 由 F1 删掉。这样 §5.7 的 `abandoned → completed` 折叠没有残留生产者，M2 的数据损失也不会继续发生。

**E11｜§28 的 grep 清单漏了真正危险的那条假权威路径。** `record_session_project_evidence`（`ingestion/mod.rs:333-360`）在**每一次发现新 Session 时**（`:250`，reconcile 热路径）用 `session.cwd` 与 **`Project.name` 的小写子串**匹配并写 `cwd_match` 证据。v0.2 之后 Project 名由路径 basename 派生，这个匹配会变成“从两份路径事实里推导出第三份成员关系”，而且它正是 §1.4 描述的 `~/.git` dotfiles 误归类的放大器。
修正：§28 的搜索清单加入 `record_session_project_evidence`、`project_affinity_evidence`、`insert_evidence`、`resolve_project_affinity`。产品路径要求：停止调用（`:250` 删调用点，函数与表保留为只读历史）。`suggest_session_project` / `assign_session_project` / `ingestion::suggest_project_for_session`（后者 `:362-366` 已经是死代码）一并退出注册。

**E12｜§1.13 与设计文档 §16 是两套字段。** 设计文档用 `status` + `trashed_at`，本方案 §1.13/§5.7 复用 `lifecycle` + `visibility`。
修正：本方案为准（零 schema 扩张优先）。设计文档 §15/§16/§17 就地改名，并在文末加对齐说明。

**E13｜§5.2 的 `workspace_paths.exists` 列名在 SQLite 里是语法错误。** `exists` 是保留关键字，`CREATE TABLE … (exists INTEGER NOT NULL)` 直接 parse 失败（实现期实测）。
修正：列名用 `exists_on_disk`，Rust 领域字段仍叫 `exists`（`WorkspaceObservation.exists` / `WorkspacePath.exists_on_disk` 在 `row_workspace_path` 处映射）。§5.2、§5.3 与 §32 的 grep 清单凡出现 `workspace_paths.exists` 之处，读取时都按 `exists_on_disk` 理解。Wave 1 任何写 SQL 的 agent 都必须用后者。

## 42.3 方案未规定、但实现必须有规则（M1…M24）

**M1｜`workstream_path_id` 是精确相等，不是前缀匹配。** §32（设计文档）用 `/repo` 与 `/repo/frontend` 举例，容易被读成“删除 `/repo/frontend` 要按最长前缀归属”。实际语义：一个 Session 只有一个 `workspace_path_id`（它自己 cwd 的那一行），binding 记录的就是这个 id，**精确等于**某条 `workstream_paths.workspace_path_id` 才叫“已在列表中”（§1.8 第一分支）。§1.8 第三分支（已有路径则 append）就是嵌套路径的真实结局：`/repo` 和 `/repo/frontend` 会**同时**在列表里。
锁死的不变式：`binding.workstream_path_id IS NULL OR EXISTS (workstream_paths WHERE workstream_id = b.workstream_id AND workspace_path_id = b.workstream_path_id)`。没有任何前缀/包含逻辑。

**M2｜Session cwd 漂移不改变 Workstream 的路径列表。** 重新发现会让 `sessions.cwd` 变化（`ingestion/mod.rs:144-158` 把 discovery 当 cwd 的 source of truth）。此时该 Session 的 binding.workstream_path_id 可能不再命中列表。规则：**不把新路径静默 append 进用户的 Workstream**（§1.7 “添加路径只增加路径”的对偶：路径列表只由用户动作或显式绑定增长）；若新 cwd 恰好已在列表里则改指过去，否则置 `NULL`（含义“该绑定不再由任何路径带来”），于是它不会被 §1.6 的删路径操作带走。这符合 §41 “不能确定 → 不做破坏性变化”。

**M3｜`sessions.project_id` 只能有一种写法。** 为了 §29 “禁止 Session.project_id 与 WorkspacePath.project_id 各自独立写入”落到实现上：该列**只在两处**被写——(a) `upsert_session` 内的一条 SQL，用 `project_id = (SELECT project_id FROM workspace_paths WHERE id = ?workspace_path_id)` 同语句派生；(b) WorkspacePath 改归属时的批量刷新。除此之外任何 `UPDATE sessions SET project_id` 都算违例（今天 `commands.rs:786-789` 就是这种写法，随 `assign_session_project` 一起退出）。
配套：`upsert_session` 的 `ON CONFLICT(agent, agent_session_id) DO UPDATE` 集合（`storage/mod.rs:803-808`）今天**故意不含** `project_id`；新列 `workspace_path_id` 要按 `COALESCE(excluded, 现值)` 加入，且当 `workspace_path_id` 变化时派生 `project_id` 必须同事务重算。

**M4｜零路径 Project 自动删除的 FK 顺序。** `project_resources.project_id` 与 `project_affinity_evidence.project_id` 都是 `NOT NULL REFERENCES projects(id)` 且**无 cascade**（`storage/mod.rs:155`、`:316`）。直接 `DELETE FROM projects` 在有资源的 Project 上必然 FK 失败，而 §7.4 要删的 legacy Project 恰恰大概率有资源。
修正：删除顺序固定为 `project_resources` → `project_affinity_evidence` → `projects`，并且 `projects` 的 `unindex("project", id)` 必须在事务提交之后（现有 `delete_project:581` 已是这个形状，照抄）。
附带修一个既存 bug：今天 `delete_project`（`:564-583`）**完全没碰 `project_affinity_evidence`**，所以删过 affinity 证据的 Project 现在就会 FK 失败——现有测试没插证据才没暴露（`launch_context_test.rs:546-569`）。v12 顺手补上并加回归。

**M5｜迁移前整库备份。** §7.4 会自动删 Project、连带删用户手工录入的 `project_resources`；§5.7 会把 `abandoned` 折进 `completed`（不可逆）。同时 `migrate()` 没有事务，中途失败会留下半迁移的库、`user_version` 停在旧值。
修正：`migrate()` 进入 v12 分支前，若 `user_version < 12` 且 `noending.db.pre-v12.bak` 不存在，则 `PRAGMA wal_checkpoint(TRUNCATE)` 后把主库文件复制为 `<db>.pre-v12.bak`（同目录；该目录已有 `*.legacy-20260913-*.bak` 的人肉搬迁先例）。备份失败 → **不执行**破坏性分支，直接返回错误让用户先处理。这是“宁可不开动也不丢数据”，符合 §41 优先级 1。

**M6｜永久删除的完整清单。** §18-13 的 “Workstream-owned Context/Review data” 必须展开，否则 `PRAGMA foreign_keys=ON` 下第一次真删就报错。固定顺序：
`context_conflict_events` → `context_conflicts`（含 `left_item_id`/`right_item_id` 引用）→ `context_item_revisions` → `context_items` → `context_deliveries` → `session_workstream_bindings` → `workstream_paths` → `session_binding_removals`（**无 FK**，`:228-233`，不删就是永久垃圾）→ `workstream_review_state`（唯一 `ON DELETE CASCADE`）→ `workstreams`。
不得触碰：`sessions`、`session_events`、`session_cursors`、`launch_intents`、`workspace_paths`、Agent 原始文件。
今天 `Db::delete_workstream`（`:781-786`）是一句裸 `DELETE FROM workstreams`，**零调用点**，一旦有引用就 FK 失败——由 C 替换成上述事务。

**M7｜Workspace Reconcile 的触发点与锁纪律。** §6 规定 Git detection 在“数据库成功打开后的 Workspace Reconcile”里跑，但没说谁触发、多久跑一次、能不能持锁。
规则：
- 触发点：(a) 启动后一次全量扫描（`lib.rs` setup 里的后台线程，紧挨现有 reconcile 线程，见 `lib.rs:82-117`）；(b) `ensure_workspace_path` 内部；(c) Project Detail 与 Settings 的显式刷新。
- **绝不持 DB mutex 跨 `git` 子进程**（AGENTS.md:79，且 ingestion 已示范逐次 lock/unlock：`ingestion/mod.rs:200-252`）。每个路径一次独立加锁读、解锁跑 git、再加锁写。
- 只对 `exists = true` 的路径跑 git；`exists=false` 只 stat 一次目录。
- 全量扫描要有总量上限与顺序稳定性（按 `path_id` 排序），避免每次启动顺序抖动导致 `last_seen_at` 大面积变化。
- **Reconcile 不得写 `workstream_paths`**（§9：发现 WorkspacePath ≠ 添加 WorkstreamPath），也不得推进任何 cursor。

**M8｜`canonical_path` 的跨平台规范必须写死——而且必须是纯词法的。** §16-7 只说了“Windows/macOS path normalization”，太薄；这是本阶段最容易在 CI 上翻车的地方，而且 `canonical_path` 是 UNIQUE 身份键。

**先纠正一个直觉方案**：设计文档 §4 写的是“canonicalize when possible”，即优先 `std::fs::canonicalize()`。**这条路不能作为身份键**，因为它对同一个输入目录会在不同时刻给出不同结果：
- 目录还不存在时 `canonicalize` 失败 → 退化成词法形式；目录后来被创建 → 身份翻转成解析后的形式。一个 UNIQUE 身份列在生命周期里变值，是这条链上最坏的 bug。
- 符号链接可以在任何时候被第三方创建（macOS `/tmp` 本来就是），同样让身份漂移。

规则（Wave 0 已实现为 `workspace::path_identity` / `workspace::normalize_path`，A/B/D/G 全部复用它，禁止第二份实现）：
1. **纯函数**：不碰文件系统、不看 symlink、不问 OS。输入字符串 → 输出 `canonical_path` 与派生 `id`，任何时候都一样。
2. 展开 `~` / `~\`（M23 之后全仓库只有这一个展开器）；相对路径以传入的 `base` 解析（无 base 则拒绝，不猜 `$HOME`）；折叠 `.` 与 `..`（词法，且在越过根时停住而不是抛错）；连续分隔符并一；去尾部分隔符（根除外）。
3. 分隔符：身份键内部统一用 `/`；`canonical_path` 列存**平台原生**形式（Windows `\`，类 Unix `/`）。
4. Windows：剥掉 `\\?\` 与 `\\?\UNC\` verbatim 前缀（后者还原为 `\\server\share\…`），盘符大写；**大小写折叠同时用于身份比较和存储身份**（§44 收敛前的原文是"只用于身份比较"，见 §44.1 的裁决理由），展示保留原大小写。macOS **不折叠**（APFS 默认大小写不敏感但保留大小写；折叠会让展示变错，而且身份已经由词法形式定义，折叠只带来歧义）。
5. 符号链接的代价是**明知故犯、且可自愈**：`/tmp/x` 与 `/private/tmp/x` 会是两条 WorkspacePath。它们各自的 Git 检测会给出同一个 `common_dir`（git 自己解析符号链接），于是 §8.3 的 git identity 收敛会把两条路径归进同一个 Project。**这条兜底成立的前提是 Git 收敛真的跑过**，所以 §33 的 dogfood Case 要包含“同一目录的符号链接拼法出现两次”。
6. `fs::canonicalize` 仍然允许用于**读 git 输出之后**（git 返回的 `common_dir` / worktree 路径要再过一遍本函数），但**不得**用它产生 `canonical_path`。
7. **测试**：macOS 上 `std::env::temp_dir()` 是 `/tmp`（符号链接），CI 的 macos runner 一定撞上。所有 resolver 测试断言 `canonical_path` 时禁止硬编码 temp 前缀，必须先过 `normalize_path` 再比。


**M9｜`git` 调用的加固。** 在用户目录里跑 git 有已知坑：
- 环境变量：`GIT_OPTIONAL_LOCKS=0`（不碰 index.lock）、`GIT_TERMINAL_PROMPT=0`（绝不等凭据）、`GIT_CONFIG_NOSYSTEM=1` 不设（尊重用户配置），`HOME`/`USERPROFILE` 保留。
- 非零退出、超时、找不到二进制、**dubious ownership（CVE-2022-24765 的 `safe.directory` 拒绝）** 全部归一为 `GitDetection::None` 或 `Missing`，**不得**冒泡成用户可见错误。`AppError` 序列化成纯字符串（`error.rs`），前端无法区分错误种类，所以“不是 repo”和“git 不存在”必须是 enum 变体而不是 `Err`。
- 只允许两条命令：`git rev-parse --git-common-dir`（+ `--show-toplevel`，用于 §1.4 的 home 判定）和 `git worktree list --porcelain`。任何写操作（`git init`/`fetch`/`config`）禁止。

**M10｜git 二进制定位复用 `exec_resolver`，不要 `Command::new("git")`。** `platform/exec_resolver.rs` 的 `candidate_dirs()`（`:27`，含 `/opt/homebrew/bin`、`~/.cargo/bin`、NVM/fnm/volta、Windows `AppData/Roaming/npm` + `PATHEXT`）、`candidate_file_names()`（`:65`）、`is_executable_unix()`（`:179`）存在的理由就是**从 Finder 启动的 macOS 应用不继承 shell PATH**——git 通常恰好只在 Homebrew PATH 里。现在这些是私有的且 `resolve()` 硬绑 `Agent` 枚举（`:145-149`）。
修正：A 抽出 `pub fn resolve_executable(name: &str) -> Option<PathBuf>`，`resolve(agent)` 改调它。这是本阶段唯一允许的既存 API 重构。

**M11｜进程 runner 同样复用。** `platform/exec_runner.rs:33 run_headless(&AgentCommand, timeout)` 已经做了双线程排空 + 硬超时 + `kill()`，形状正确；两个障碍：参数类型是 adapter 命名空间的 `AgentCommand`，以及 `:36-40` 在 `cwd=None` 时**硬编码回退 `/tmp`**（Windows 上错，对必须在指定目录里跑 git 的 resolver 更错）。
修正：抽出 `pub fn run(program, args, cwd: Option<&Path>, timeout_secs) -> Result<HeadlessOutput>`，`run_headless` 委托它；resolver 永远显式传 cwd。

**M12｜bootstrap config 的确切落点。** §3 只说“极小的 OS-native bootstrap config”。仓库无 toml/yaml 依赖，`dirs = "5"` 已在（`Cargo.toml:29`），`serde_json` 已在。
定死：`<dirs::config_dir()>/app.noending.desktop/home.json`（macOS = `~/Library/Application Support/app.noending.desktop/home.json`，Windows = `%APPDATA%\app.noending.desktop\home.json`，Linux = `~/.config/…`）。内容 `{"current_home": "...", "pending_home": "..."?}`。它在 NoEnding Home **之外**且与 Home 无关，所以不会因为搬迁而自我依赖。写失败/解析失败 → 回退 `~/.noending`，但必须在日志里显式报告，不得静默。

**M13｜Home 解析必须可注入，测试不得碰真实 `~/.noending`。** `NOENDING_HOME` 读取点只能有**一个**函数（`home::resolve_explicit_override()`），其余全部接收显式 `&Path`。原因：Rust 测试同进程并行，`std::env::set_var` 互相污染（且新版 Rust 里是 unsafe）。
硬要求：任何测试都不允许解析出真实用户 Home 下的路径；`home.rs` 的公共入口接受一个 `HomeRequest { explicit: Option<PathBuf>, bootstrap: Option<PathBuf>, home: PathBuf }`，测试自己传三件套。

**M14｜NoEnding Home 的搬迁范围比 §2 列的广。** 现在 `app_data_dir()` 里实际有：`noending.db`（本机实测 95 MB）、`noending.db-wal`/`-shm`、`context-bundles/`（launcher 写，`launcher/mod.rs:66-71`）、以及历史 `.bak`。`app_data_dir` 在 **7 个地方**被独立重新推导：`commands.rs:929-939`（`get_app_info` 报给 Settings 的 `db_path`）、`:1090`、`:1109`、`:1165`、`:1191`、`:1218`、`:1474`（后者经 `assistant/mod.rs:223`）。
修正：这些点全改成读一份启动时解析并 `app.manage()` 的 `NoEndingHome` 结构，不允许再各自 `app.path().app_data_dir()`。`context-bundles/` 归入 `<home>/runtime/context-bundles`。搬迁时 `-wal`/`-shm` 必须先 checkpoint 再随主库一起移动。**`get_app_info` 的 `db_path` 在搬迁完成前不能报新路径**（否则 UI 说谎）。

**M15｜共享默认 workspace 会打断 LaunchIntent 自动匹配。** §13/§33 Case E 让 `~/.noending/workspace` 成为所有“无 Workstream 路径”的启动的共同 cwd。而 `launcher/mod.rs:944-961` 的匹配打分是：`intent.cwd` 与 `session.cwd` 互为前缀 `+3.0`，`<60s` `+2.0`，判定要求 `best - second >= 2.0`（`:967-995`）。两个并发独立启动 → 两个 intent 的 cwd 完全相同 → 各 +3.0+2.0 → 差值 0 → 双双 `AMBIGUOUS`。
处理（§41：不确定时不做破坏性变化）：接受 `AMBIGUOUS` + 现有 `resolve_launch_intent` 人工裁决是**正确**行为，不引入猜测。两条改进进实现：(1) 打分时若 `intent.cwd == 默认 workspace`，cwd 证据降为 `+1.0`（共同目录不携带区分度，不该拿满分）；(2) 平分时优先 `selected_workstream_ids` 非空的 intent（用户明确选过 Workstream 是更强证据，符合 AGENTS.md “用户绑定强于自动分类”）。必须加测试锁死“两个并发 standalone 启动 → 两个 AMBIGUOUS → 人工裁决后各自绑定正确 Workstream 且互不串”。

**M16｜Resume 的 `prepared.cwd` 今天根本不用于 spawn——这是既存的 Preview-Launch Identity 违例。** `prepare_resume` 存 `cwd: session.cwd.clone()`（`launcher/mod.rs:189`），但 `launch_prepared_with` 的 resume 分支在 `:368` **重新读库** `session.cwd` 并忽略 `prepared.cwd`；而 `session.cwd` 不在指纹输入里（`:526-563` 只哈希 `last_activity_at` + cursor）。也就是说：后台 discovery 在 Preview 与 Launch 之间改写 cwd，就会在用户没看到的情况下换启动目录。
本阶段 §13/§30 把 fallback 链变成用户可见事实，所以这条必须一起修：resume 用 `prepared.cwd` 启动，`cwd` 进入指纹（两种 mode 都进）。这是 §12 “以下任一变化必须使未消费 PreparedLaunch stale” 的真正前提。

**M17｜指纹哈希缺字段分隔。** `compute_state_fingerprint`（`launcher/mod.rs:459-568`）在多处直接相邻 `update()` 字符串（如 `:504-512` 的 item/revision 六连），字段边界可移位的理论碰撞既存。新增输入必须自带标签：`b"ws_paths:"`、`b"primary:"`、`b"default_ws:"`、`b"cwd:"`，并在每个值后 `update(b"|")`。有序路径列表**保持列表顺序参与哈希、不排序**（与 `:490` 对 `effective_workstream_ids` 的既有处理一致；顺序本身就是语义）。

**M18｜搜索索引跟着换权威。** `index_workstream`（`storage/mod.rs:2007-2014`）把 `w.project_id` 写进 `search_index.parent_id`，`index_project`（`:1998-2005`）产 `kind='project'` 行；`SearchView.tsx:45` 与 `CommandPalette.tsx:81` 按 `parent_id`/`kind` 路由。E6 之后 `w.project_id` 冻结在创建值，搜索结果会指向过期成员关系。
修正：`index_workstream` 的 `parent_id` 改取 **position-0 路径的 Project**（可空）；Project 自动创建/改名/合并/删除时分别 `index_project` / `unindex`；v12 迁移末尾对全部 workstream 与 project 重新索引（`backfill_search_index` 已有幂等先例，`lib.rs:74`）。

**M19｜卡片 `project_name` 在多 Project 下的定义要冻结（§8.1.1 式契约）。** 一个 Workstream 现在可能属于多个 Project，而 `WorkstreamCardData.project_id/project_name`（`types.ts:58,66`）被 Home、Sidebar、卡片、ContextUpdates 等约 10 个界面消费（`commands.rs:270-274`、`:583-587`）。
冻结：两列含义变为 **“position-0 路径所属 Project” 的投影**，无路径或路径无 Project 时为 `null`；列名与类型不变，只改文档注释和 `commands.rs` 的取数 SQL。Project Detail 里的“主关联/关联”由 M4/E4 的 `is_primary` 提供，不用卡片字段。

**M20｜新字段的机械 fixture 改动归 Main。** `Workstream`/`Session` 结构体新增字段会让全部 18 个集成测试文件的 struct literal 编译失败（Rust literal 必须列全字段；`project_id: None, default_cwd: None, lifecycle:"open"` 的字面量散落在 `base_experience_test.rs:71-89`、`context_review_test.rs:12-24,381-461`、`launch_cwd_test.rs:20-34`、`launch_context_test.rs:16-43`、`sync_integrity_test.rs`、`llm_cli_test.rs`、`workstream_cards_test.rs:44-61`、`replace_bindings_test.rs:46-51` 等处）。
要求：foundation commit 内一并改完，并给 `Workstream`/`Session`/`WorkspacePath` 各加一个测试友好构造 helper（沿用现有 `ws_row(...)` 风格，放 `storage` 或一个 `tests` 可复用的 `pub fn` in crate），否则四个 Agent 会各自再造一套 fixture 工厂——上一阶段 `useWorkstreamCards` 的教训。
特别注意 `context_review_test.rs:66-67` 的裸 `INSERT INTO workstreams (id, title, description, lifecycle, visibility, created_at, updated_at)`（不含 `default_cwd`）：它同时是 E2 说的“绕过 Rust 写入层”的唯一实例，加列不会破坏它，但改列默认值后它写入的是 `'open'` → 迁移前构造 v11 fixture 时**这是特性**，v12 迁移测试正好用它。

**M21｜默认 workspace 必须真实存在。** `~/.noending/workspace` 在 bootstrap 时 `create_dir_all`。若不建就交给 §13 当 cwd，会命中既存的不对称：macOS 的 bash 脚本 `cd` 失败会**打印一句提示后回退 `$HOME`**（`platform/launcher.rs:42-53`），Linux 直接 spawn 失败（`:268-276`）——即“你以为在默认 workspace，实际在 $HOME”，而 `$HOME` 正是 §1.4 要防的 dotfiles repo。这条要在 A 的实现注释里写明。

**M22｜取消 “新建 Project” 之后的空态与导航文案。** §22 要求 Projects 无“新建 Project”，但入口有两个：`ProjectsView.tsx:68` 页头按钮 + `:77` EmptyState action + `:99-116` Modal，以及 `Sidebar.tsx:121` 的 “+” + `:143-166` Modal，还有 `Sidebar.tsx:123` 的 “尚未创建”。Project 现在永远是自动派生的，空态文案必须改成“还没有 Project——打开一个 Session 或给 Workstream 选一个工作目录后会自动出现”，并且**不提供任何创建按钮**。`ProjectDetail.tsx:196-198` 的 “Project 不依赖任何路径” 与 `:82-83` 的 “Project 只是可选的组织层” 两句在新模型下是错的，F1 一起改。

**M23｜两个波浪号展开器只能留一个。** `platform/paths.rs:60 expand_tilde(&str) -> PathBuf`（不处理 `~\`）与 `launcher/mod.rs:762 expand_tilde(&str) -> String`（处理 `~\`，且自称“唯一权威展开点”）。§16-1 把 `platform/paths.rs` 给了 A，而 launcher 归 E。
修正：A 合并为 `platform::paths::expand_tilde`（取 launcher 版本的语义，即支持 `~\`）+ 让 `launcher` 调用它；E 不得再引入第三份。归 Wave 1 集成时 Main 处理，避免 A/E 同时改 launcher。

**M24｜不给 `launch_intents` 加 `workspace_path_id`。** §12 把 “Session workspace path” 列为 stale 输入，容易顺手在 intent 上再加一份路径事实。intent 已经有 `cwd`（`storage/mod.rs:287`），它就是启动时刻的原始证据；WorkspacePath 由 discovery 之后的 Session 行承载。**第四个路径存储列在本阶段禁止出现**。

## 42.4 执行结构修正

Wave 0 / Wave 1 / Wave 3 与 §14、§15、§24 一致。改动只有两处：

**（1）Wave 2 拆成 bridge + 两个前端 Agent。**

```text
Main: refactor(ui): bind workspace domain types to the frontend   ← 独占 api.ts / types.ts / routes.ts
  │
  ├── E  launcher + PreparedLaunch（纯 Rust，见 §21）
  ├── F1 projects/** + workstreams/**（含 NewWorkstreamModal、回收站、有序路径 UI）
  └── F2 sessions/** + settings/** + layout/Sidebar.tsx
```

bridge commit 由 Main 落：所有新命令的 `api.ts` wrapper、`types.ts` 新类型、`Project`/`Workstream`/`Session` 字段调整，以及 `src/app/routes.ts` 的路由变更。F1/F2 之后**不允许**再改这三个文件；需要新接口时在交付报告里向 Main 提出。
F1/F2 可能同时需要 `src/components/`（Modal、EmptyState）：只读复用，改公共组件的需求回报 Main。

**（2）worktree 与 build 缓存。**
worktree 由 Main 建（E8）。四个/三个 Agent 并发跑 `cargo` 时共享 `CARGO_TARGET_DIR=<repo>/.qoder/shared-target`（依赖只编一次；cargo 自己的目录锁会把并发 build 排队，这是可接受的等待，比各自冷编译 4 遍快一个量级）。前端各自 `pnpm install --frozen-lockfile`（pnpm store 硬链接，代价低）。

**（3）Agent 交付报告增加一节硬性内容。** §37 模板基础上，每个 Agent 必须回答 §28/§29 的五个问题（能否丢历史 / 能否重试两次生效 / 能否破坏 SourceReference 或 Audit / 能否让推断覆盖用户意图 / macOS+Windows 是否都正确）。上一阶段的经验：不写出来的 Agent 不会检查。

## 42.5 封板前的机械断言（T1…T4）

Main 在最终集成时加进 `.github/workflows/ci.yml`（只动 CI，不碰 `src/`）：

- **T1**：`create_project|delete_project|assign_session_project|suggest_session_project|merge_workstreams` 在 `src-tauri/src/lib.rs` 的 `generate_handler!` 中出现次数为 0；`src/` 中 `api.createProject|api.deleteProject|api.assignSessionProject|api.mergeWorkstreams` 调用点为 0。
- **T2**：`UPDATE sessions SET project_id` 与 `SET project_id = ?` 只允许出现在 `storage/session_paths.rs`（M3）与迁移代码里。
- **T3**：`workstreams` 的裸 `INSERT`（不含列 `lifecycle`）在产品代码中出现次数为 0；`lifecycle = "open"`/`'open'` 在产品写入点出现次数为 0。
- **T4**：`Command::new("git")` 字面量为 0（必须走 M10 的 resolver）；`src-tauri/src/workspace/` 之外不出现 `--git-common-dir` / `worktree list`。

## 42.6 明确不做（本阶段发现的既存问题，只记录不修）

**N1｜launcher 持 DB mutex 跨子进程 spawn，违反 `AGENTS.md:79`。** `commands.rs:1223` 的 `with_db` 把整个 `launch_prepared`（含 `:324`/`:376` 的 `spawn`）包在一个 `MutexGuard<Db>` 里；签名 `launch_prepared_with(&self, db: &Db, …, spawn)` 强制借用跨过 spawn。macOS 上这意味着 Terminal 打开窗口期间所有 DB 命令阻塞。
现状是**安全但不合理**：指纹复检（`:236`）与 spawn 同锁，天然无竞态。要修就得在 spawn 前后释放锁，那会把指纹复检变成真正的 TOCTOU 边界，需要在 `insert_launch_intent`（`:312`）之前重新加锁复检。**本阶段不动**（§40 精神：不顺手扩大范围）；v12 的 launcher 改动全部保持在同一锁内。

**N2｜`ContextMutation::CreateWorkstream` 的 TS 类型是谎。** `types.ts:159` 把 `project_id` 写成必填 `string`，而 Rust 侧是 `Option<String>`（`sync/mod.rs:72`）且 `extractor.rs:595` 永远发 `None`。渲染路径为零，不影响行为。F1 顺手改类型即可，不单独开工。

**N3｜`delete_workstream` 零调用点 + FK 必炸**，见 M6，由 C 替换而不是“保留观察”。

**N4｜`projects.name` 至今无任何 UNIQUE**，§37 自动命名（repo basename / `NoEnding Workspace`）会产生同名 Project。这是**允许**的：v0.2 的 Project 身份是 `git_id` + 拥有的路径，名字只是展示。不给 `name` 加约束。

---

# 43. 执行记录（Main 逐 Wave 追加，Agent 不写）

## 43.1 Wave 0 — Foundation

```text
FOUNDATION_SHA=eb930432cf66f392cafd7f85175fc1ebcfe75150
commit: refactor(workspace): establish workspace domain v0.2 contracts
gates : cargo fmt --check / cargo check --all-targets / cargo test --all-targets (221 passed, 0 failed, 5 ignored) / pnpm build
```

`FOUNDATION_SHA` 是**契约锚点**（代码从这里开始可编译、可测）。Wave 1 的 worktree 实际 base
是派发那一刻的 main HEAD（至少包含本节文档，agent 才能读到 §42/§43），Main 必须把**该
base 的完整 SHA 写进每个 agent 的 prompt 并在交付报告里回显**（§42.2-E8）。

落地范围＝§15 的 14 项 + §42.3-M5/M8 + §42.2-E13。与方案的偏差（后续 Wave 必须按此口径，不要按原稿）：

1. **`workspace_paths.exists` 列名改为 `exists_on_disk`**（E13）。领域字段仍叫 `exists`。
2. **`workstreams.lifecycle` 的列默认值仍是历史 `'open'`**。`CREATE TABLE IF NOT EXISTS`
   无法改已存在表的默认值（E2 预言成立）：新建库拿到 `DEFAULT 'active'`，升级库拿到
   `DEFAULT 'open'`。因此**任何写 `workstreams` 的 SQL 必须显式给 lifecycle**，
   §42.5-T3 就是这条的机械守卫。
3. **结构性冻结比 §15-13 更严**：`upsert_workstream_conn` 的 `DO UPDATE` 集合只剩
   `title / description / lifecycle / visibility / updated_at`，`INSERT` 侧仍写
   `project_id`、`default_cwd`（只为新建库留初始值），所以**升级后的旧行不可能被
   整对象写回污染**，而新建 Workstream 的这两列在 v0.2 里恒为 `NULL`。
   `upsert_workstream_cannot_move_between_projects_or_retarget_cwd` 与
   `default_cwd_is_frozen_after_creation` 是两个反向断言测试。
4. **`sessions.project_id` 有 FK 到 `projects(id)`**，所以派生语句天然不可能指向不存在的
   Project；测试 `session_without_cwd_gets_no_workspace_path` 因此用一个真实 Project 表达
   “无 cwd 的 Session 只是缓存值，不是派生值”。
5. **`list_session_bindings` 多了 `workstream_path_id` 一列**（§11 未列，但 §5.6 的语义要求
   前端能区分“由哪条路径带来”）。TS 形状已同步冻结。
6. **`delete_project` 顺带修了既存 FK bug**（M4 附带项）：以前不删
   `project_affinity_evidence`，任何有证据的 Project 都删不掉。
7. **Wave 0 契约测试**在 `src-tauri/tests/workspace_v12_test.rs`（9 个）：身份确定性、
   position 稠密性、派生缓存单向性、迁移折叠 + 重放 + 备份、binding 精确解析失败→NULL。
   Wave 1+ 不得删这些测试来“让实现通过”。

## 43.2 Wave 1 — A / B / C / D 并行交付与 Main 集成

```text
WAVE1_SHA=9a56649681731327366d8e6f1952eb14a4997782
base    =7cce6fb0f3ec931ea6ed86036e20fce9a234bea4
agents  =A 2cf1472 c29c85a 0aac5eb · B 12e2f1f 8471f51 · C 762850c 42c3af4 34e8d8c · D f2ee362
gates   =cargo fmt --check / cargo check --all-targets (0 warning) / cargo test --all-targets (339 passed, 0 failed, 5 ignored) / pnpm build
```

九笔 commit **零冲突**（§38 的文件 ownership 生效）。Main 的集成工作在 `9a56649` 一笔里：启动时序、命令注册、launcher 收口、`workspace/wiring.rs` 的唯一 `WorkspaceAttaching` 装配。

集成期发现并已修的两处跨 Agent 不一致：

1. **`sessions.project_id` 出现了第四个写入文件**（B 的零路径 Project 自动删除 vs D 的 §42.5-T2 可执行守卫）。守卫是对的，B 的写法破坏了“缓存只有一个写入者”。修法：Project 消亡时的失效语句收敛进 `storage/session_paths.rs::clear_sessions_project_for_project_conn`，两条删除路径都调它——“谁能写这列”仍然一文件一答案。
2. **`identity::join` 在路径被 `.`/`..` 折叠回 base 时留下尾分隔符**（A 报告）。`/repo/x` 与 `/repo/x/` 会同时存在于 UNIQUE 的 `canonical_path` 上，即一个目录两个 WorkspacePath、进而两个 Project。`join` 折叠时直接返回 root（裸盘符 `C:` 例外，它必须保留分隔符），并在 `path_key` 里再兜一层，加了契约测试 `a_collapsed_path_never_grows_a_trailing_separator`。

Main 对 agent 提出项的裁定：

| 提出 | 裁定 |
| --- | --- |
| B#5(b) `upsert_project_conn` 的 `git_id` 可被整对象写改目标 | **接受**。改为 `COALESCE(git_id, ?5)`：一次写入后不可变更，唯一合法写手是 `adopt_git_identity_conn`（first-set-only）。契约测试 `a_whole_object_project_write_can_neither_clear_nor_retarget_git_identity` |
| B#5(a) `delete_project_and_children` 委托给 `delete_zero_path_project_conn` | **拒绝**。两者策略不同：legacy 删除要“ detach 并删资源”，零路径删除必须**拒绝**仍拥有路径的 Project。合并会让后者失去守卫 |
| B#6 / D#4 §42.5-T2 的 grep 写法 | **接受**。`SET project_id = ?` 会误报 `UPDATE workspace_paths SET project_id`；T2 必须与 `UPDATE sessions` 连读（见 §43.3） |
| D#1 binding diff 归 `workspace::session`，launcher 变 shim | **接受并已落地**（`9a56649`）。两个引擎只有一个会长路径列表，正是 §29 禁止的双 authority |
| C#2 `Db::delete_workstream` 是裸 `DELETE` | **接受**。改为走 `purge_workstream_data_conn` 的完整 FK 顺序；产品入口仍是 `delete_workstream_permanently`（要求 archived） |
| C#4 `sync/merge.rs` 仍写 `project_id` | **接受**。写 `None`，`ContextMutation` 的字段形状按 §31 保持不动 |
| A#c / D#5 `WorkspaceObserving::observe` 无法表达“这不是一个路径” | **本阶段接受 A 的 `is_observable` 哨兵**，因为唯一的写入 door `ensure_path_outcome` 已经拒绝空 canonical 与 path_id 不一致的观察，并有 B 的 `Lying` 测试钉住。改成 `Option<WorkspaceObservation>` 触及 40+ 调用点（含 A/B 各自的测试），收益是防“未来有人忘记检查”。记录为 §43.4 的 Wave 2 待办 |
| D#4 自动分类是否 repair NULL claim | **维持现状**：sync 写 `workstream_path_id = NULL` 合法（legacy 形状），不自动向上 repair，见 M25 |

## 43.3 对 §42 的增补（实现期新发现的规则，Wave 2/3 必须遵守）

**E14｜§42.3-M12 自相矛盾。** 它把指针文件写成 `<dirs::config_dir()>/app.noending.desktop/home.json`，同一句又说 macOS 在 `~/Library/Application Support/…`——macOS 上 `config_dir()` 是 `~/Library/Preferences`。A 按括号里的路径实现（`platform::paths::resolve_app_support_dir()`：macOS `data_dir()`、Windows `config_dir()`），因为 M14 依赖“pre-v0.2 的库已经在那里”。本条以 A 的实现为准。

**E15｜§42.3-M6 的永久删除清单仍不完整。** `project_affinity_evidence.workstream_id` 是**第六条**指向 `workstreams` 的 FK，且被删 Workstream 的 `context_item` 的 search 行也要清。C 在 `purge_workstream_data_conn` 里补齐，并用 `permanent_delete_clears_every_row_the_workstream_owns` 钉住。注意补法：affinity 行是**置 NULL 保留**（§42.2-E11 说它是只读历史），不是删除。

**E16｜§18-2 高估了 schema 的约束力。** `UNIQUE(workstream_id, position)` 只禁止“两个 position 0”，`{1,2}` 没有 0 在 SQLite 里合法。真正维持稠密性的是 `remove_workstream_path_conn` 同事务里的 recompact。任何绕过该 helper 直接 DELETE 的写法都会造出无 primary 的列表。

**E17｜§42.2-E10 说 `merge_workstreams` 是 `abandoned` 的生产者——v12 之后不再成立**（Wave 0 已把它改写成 `completed`）。它退出注册的真实理由是：它在**不产生 Revision 的情况下**把 Context item 在 Workstream 之间搬走，违反“每一次 Context 变化都必须留下可审计的 Revision”。T1 的 grep 保留它，理由改成本句。

**M25｜NULL claim 只清不修。** 漂移检测可以自动把 `workstream_path_id` 从有值置 NULL（含义“不再由任何路径带来”）；反向——从 NULL 变成有值——**只在用户重新绑定 / 重新保存时**发生。因为向上 repair 会把一条绑定从“路径删除够不着”变成“跟着路径一起被删”，那是用户不可见的损失（D 提出，Main 采纳）。

**M26｜Project merge 按身份分两种。** path-backed（`git_id IS NULL`）Project 在遇到 Git 家族时**整体**并入对方（它的全部路径都是同一家族的成员证据）；git-backed Project 只失去**自己证据变了**的那条路径——它的其余路径按身份属于另一个家族，搬走等于凭空创造成员关系。

**M27｜WorkspacePath GC 是“整轮”决策，不是逐行的。** “不再是任何 live Project 的 worktree”需要本轮所有 worktree 列表的集合；逐行判定会 livelock（删掉一个可 prune 的 worktree，同一份列表下轮又把它建回来，无限循环——B 在测试里撞到了）。实现为 sweep 级：本轮亲自观察到目录不在，且没有任何在册 Project 的 worktree 列表仍声称它，才删。

**M28｜`ensure_workspace_path` 返回 `ProjectionEffect`，由事务的持有者在 commit 之后应用。** `PRAGMA foreign_keys=ON` 下 unindex/index 必须在事务外做（§42.3-M4），而“谁持有事务”只有调用方知道。所以：在别人的事务里请用 `ensure_path_outcome` + `apply_projection_effect`，不要指望写入方顺手把搜索行建好。

**M29｜`get_project_detail` 的 Sessions 走权威链**（`workspace_path_id → path`），不走缓存列。只有一个 legacy 缓存值、没有 cwd 的 Session 因此不出现在 Project 详情里（它仍出现在 `list_sessions(project_id)`）。这是有意的：详情视图不能同时是两种真相。

**N5｜自动命名用“被观察路径”的 basename。** 如果一个仓库的 linked worktree 先被观察到，Project 名就是那个 worktree 目录名而不是仓库名。N4 已说明名字不承载身份，本阶段不改（改成用 `common_dir` 的父目录需要对裸仓库和 worktree 都猜）。

## 43.4 用户可见的行为变化（dogfood 与 §22 文案要按这个口径）

1. **归档变成单向动作。** `archive_workstream` 不再是“翻转”，`restore_workstream` 才是取消归档。现有 `WorkstreamDetailView.tsx` 的“取消归档”按钮调的是 archiveWorkstream，F1 必须改路由。永久删除只在 `visibility === "archived"` 时提供。
2. **Session→Project 不再能手工指定。** 没有可解析 cwd 的 Session 就不显示 Project（以前手工标签会撑着它）。§7.4 的意图，但用户会先看到“历史里有些 Session 没有 Project 了”。
3. **`archived = 1` 的 legacy Project 会重新出现**（E3：v0.2 的 Project 没有生命周期，隐藏一个仍拥有路径的 Project 是幽灵不是状态）。
4. **pre-v0.2 的数据库会被接管。** `noending.db` 原本在 `…/Application Support/app.noending.desktop/`，不在 `~/.noending/data` 里；逐字实现 §16 会让首次 v0.2 启动打开一个空库，用户的 History 看起来被删了——正是 §41 要避免的失败。`adopt_legacy_data_dir` 把它 move（跨卷时 copy 且保留原件）过去，失败时留在旧 Home 并写进 notes。
5. **上下文 bundle 的位置从 `app_data_dir()/context-bundles` 变成 `<home>/runtime/context-bundles`**，Settings 的“数据目录”一行现在叫 NoEnding Home 并额外显示默认工作目录。

## 43.5 Wave 2 待办（派发时必须带上）

- **E**：§12 指纹必须加入 (a) Workstream 有序路径列表（`workspace::workstream::workstream_launch_paths(db, id).fingerprint_input()`）、(b) `default_ws`（来自 NoEnding Home，不是 DB，必须作为独立带标签输入传入）、(c) Session 侧 `sessions.workspace_path_id`。C 明确警告：**在 E 加进去之前，改路径不会让 PreparedLaunch 变 stale**，§12 的“预览即所得”只是名义成立。另 M16（resume cwd 的既存违约）与 §42.3-M24（intent 不加第四个路径列）同时生效。
- **F1**：Projects 无“新建”；有序路径 UI；回收站（archive/restore/永久删除）；`WorkstreamPathRow` 形状；删掉 `mergeWorkstreams` / `createProject` / `deleteProject` wrapper。
- **F2**：Sessions 表去掉 Project 手工选择；Settings 的 Home/默认工作目录 + `set_noending_home` 的“重启后生效”提示。
- **bridge**：`createWorkstream(title, description, initialPath?)`；`WorkstreamCardData.path_count`；`WorkspaceSettings.home_source`。

## 43.6 真实语料彩排（Main 在 Wave 2 期间做的额外一步）

§26 要求迁移“可证伪”，但 fixture 只能证明 fixture 里想到的分支。于是把用户的
**真实库复制一份**（95 MB / 475 Sessions / 10 716 events / 396 bindings / 354
context items / 0 Project / 0 resource / 0 evidence），跑了两轮：

**第一轮（迁移本身）暴露了一个只有真实 v11 库才会暴露的 P0**：`CREATE UNIQUE
INDEX … ON projects(git_id)` 排在幂等 ALTER 列表**之前**，而
`CREATE TABLE IF NOT EXISTS` 不能给已存在的表加列——所以任何**升级**的库都在
`Db::open` 阶段直接失败（`no such column: git_id`）。之前的 10 个迁移测试全部是
“新建库 + 把 user_version 拨回 11”，库里本来就有 `git_id`，所以一个都没抓到。
修复＝把索引挪到 ALTER 之后；并把 `workspace_v12_test` 的 legacy fixture 改成
手写的**真 v11 表结构**（已验证可证伪：把顺序改回去，它以生产环境同一条错误信息
失败）。落 `8bb4290`。

彩排结果（`tests/v12_dogfood_test.rs`，默认 skip，只有指到临时目录下的副本才跑）：
§1.1 / §1.2 / §5.7 / §1.10 / §42.3-M1 全部成立，events / bindings / cursors /
context_items / launch_intents 数量不变，`.pre-v12.bak` 在位。

**第二轮（Workspace Reconcile，真正跑 git）**：scanned 27 · moved 0 ·
discovered 8 · failed 0 · GC 0。Git 家族合并确实发生了（一个 Project 拿到 8 条
路径、另一个 2 条；两个同名 `Deepseek-harness` 因为不同家族而**正确地**没被合并）。

但落地的 Project 列表长这样，这是 §33 dogfood 必须先面对的事实：

```text
393 sessions · 1 path  · Tmp               ← /private/tmp，占绝对多数
 27 sessions · 2 paths · Stock_quant (git)
 17 sessions · 1 path  · Workspace (git)
  5 sessions · 1 path  · Testgodot / Deepseek-harness (git)
  3 sessions · 8 paths · Noending (git)
  1 session  · 1 path  · ×19 个一次性目录（其中 "Realtime-voice-chat" 同名 4 个、
                          ".claude" 这种隐藏目录、微信沙盒容器 Data）
```

模型是**忠实**的（用户确实在那些目录里跑过 Agent），但对话式一次性目录会生成一堆
单 Session、同名、无信息量的 Project。可选的收敛口径（都属于**产品决策**，本阶段
§40 不许我顺手加智能）：
(a) 名字冲突时带上父目录做消歧（纯展示，不动身份）；
(b) 对 scratch/临时目录（`$TMPDIR`、`/private/tmp`、沙盒容器）不建 Project，或
折叠成一个“其他”；
(c) Projects 列表按 Session 数排序 + 默认隐藏 1-Session Project。
Main 的建议：先做 (a)（零风险、纯展示），(b) 需要用户点头，(c) 属于 Wave 2 之后。

**用户裁定（2026-09-19，真机看过侧栏之后）：三个选项都不做，同名就同名。**
所以 §43.6 的 (a)/(b)/(c) 到此关闭，不要再提议。真界面上看到的形状就是最终口径：
`Realtime-voice-chat` 出现 4 次、`Deepseek-harness` 出现 2 次（不同 Git 家族，
§8.3 正确地没合并），以及 `To` / `Nin` / `Ke` / `Jian` 这类两三字母的名字——
它们是 `~/Documents/Codex/<日期>/<slug>` 里真实的目录 basename，不是命名截断。
Project 列表**忠实反映"用户在哪些目录跑过 Agent"**，噪声由用户自己承担，
不换取消歧、也不换隐藏。

（Main 曾按"同名加数字后缀"实现过一版 `display_names`：库里 `name` 不动，
读时按整个集合派生标签，`name_customized` 的 Project 永不参与编号。该实现**未提交、
已完整还原**，记录在此只为说明：这条路的正确形状是"派生显示标签"而不是
"写回名字"——写回会让删掉一个兄弟 Project 时偷偷重命名其余的。）

## 43.7 Wave 2 — E / F1 / F2 交付与 Main 集成

各 agent 的交付（base 均为派发时记录的 HEAD，已用 `git diff --stat` 核对越界）：

```text
F1 agent/ui-projects-workstreams  7ff2d79 a875daa ac4cb71 19ee6b6   projects/** workstreams/**
F2 agent/ui-sessions-settings     d5f2857 3bb9863                   sessions/** settings/**
E  agent/launcher-workspace        8d2c168  (base 7a207e9)           launcher/** + 3 个测试文件
```

合入顺序 F2 → F1 → E，**零冲突**（三者文件集互不相交，这条纪律第二次生效）。Main
补三个 commit：

```text
65cef68 把 NoEnding Home 注入所有启动/同步入口（E 的 §21-2/3 在生产里此前是惰性的）
18bcff4 兜底目录不得进入 Workstream 路径列表（M30）
35c8c98 前端桥接收尾 + 预览显示启动目录来源
```

**WAVE2_SHA = `35c8c98`**。Wave 2 gate：`cargo fmt --check` 通过、`cargo check
--all-targets` **0 warning**、`cargo test --all-targets` **359 passed / 0 failed**、
`pnpm build` 通过。

集成期核查出的三处“报告与代码不符”（这就是 Main 必须自己跑一遍的理由）：

1. **E 说 `latest_session_cwd_for_workstreams` 零调用者**——生产路径确实为零，但
   `tests/workstream_cards_test.rs` 有 3 处，而且那个测试钉的正是被 §13 废除的旧
   authority（“Workstream 的默认目录 = 最近 Session 的 cwd”）。函数与测试一起删除；
   留着它等于留第二个 cwd authority 的活证据。
2. **E 说 `CwdResolution` 已实现**——后端属实，且已进指纹；但**没有任何 UI 读它**，
   所以 §13「发生 fallback 必须在 UI 明确显示」在 E 交付时仍然只是名义成立。Main 在
   `LaunchPreviewRows.tsx` 补 `CwdRow`：显示“启动来源：哪一层”，并把后端的 `note`
   以 warning 色渲染。用户看到 `~/.noending/workspace` 时不可能自己推断出那是降级。
3. **F1 说 Rust 已经在发 `path_count`**——核实为真（`commands/workstream.rs:189,265`），
   已补进 `WorkstreamCardData`。

顺带发现并修掉的桥接谎言（不属于任何 agent 的错，是 Wave 0/1 拆分留下的）：
`ContextMutation` 的 TS 镜像把 Rust 的 `source_refs: Vec<String>` 写成了
`source_ref: string`（5 个 variant 全错），`create_workstream.project_id` 写成非空；
`LaunchResult` 少了 `launch_intent_id`。另外 `get_app_info` 与
`get_workspace_settings` 都在报同一个 Home，只有前者读的是**真正打开的那个 db**——
把那个优点并进 settings，`get_app_info` / `AppInfo` 整体退役（§29 单一 authority）。

## 43.8 对 agent「方案有错」论点的裁决

**E-(a)「每个新输入都必须写明谁注入它」— 采纳，写成 M31。** §21 只说“launcher 要用
默认工作目录”，没说 Home 怎么进到 launcher；结果就是 E 只能做成“带兼容入口的显式参数”，
在它的 lane 里永远无法闭环，表现为“半完成但全绿”。这不是 E 的实现问题，是方案的派发模板
问题：以后凡是往指纹/Launch 里加输入，条目必须写成
`输入 → 数据来源 → 由哪个 Main-owned 文件注入`，缺最后一环视为未派发。

**E-(b)「兜底目录会被洗进 Workstream 路径列表」— 采纳，已实现为 M30。** 这是 Wave 2
最重要的产品发现。落法：`record_user_binding_growing(..., grow_path_list)`，
launcher 在 `source == DefaultWorkspace` 时传 `false`；`apply_match` 多收一个
`&LaunchWorkspace`，比较 `sessions.workspace_path_id` 与默认工作目录的 identity，
相同则不增长。绑定本身照旧是用户强写（`explicit_launch_selection`，不动 §42.3 的
tombstone / strong-set 判据——新增一个 source 常量要改 4 处 authority 判断，漏一处
就把用户绑定降级，风险更大）。回归测试：
`a_default_workspace_fallback_binds_without_teaching_the_workstream_that_path`，
含一个“用户手打的目录仍然增长列表”的反向对照。

**E-(c)「M15 的措辞和它引用的代码互相矛盾」— E 对，代码错，已按方案的句子实现。**
`§42.3-M15` 写“双双 AMBIGUOUS”，分支却只标 `scored[0]`，等于把另一个候选藏进
PENDING。保留 E 的版本（所有并列候选都标 AMBIGUOUS），并把它当作 §42.3-M15 的正式
语义：这条不再回头。测试 `concurrent_default_workspace_launches_stay_ambiguous` 是锚。

**E-(d)「§12 该哈希的是“解析结果”，不是 cwd 字符串」— 采纳，写成 M32。** 目录字符串
不变但已不可用（被删、挂载点消失）是最关心的失效模式，只哈希 `cwd` 永远不会发现。
现在进指纹的是 `cwd` + `source` + `workstream_id` + `path_position` + `fallback`，
外加 `session_path` / `claim`。§12 的文字按这个口径理解，不再单独改（历史条目）。

**E-(e)「explicit 层的目录不存在时该怎么办」— 不采纳硬拒绝，维持“照办 + note”。**
用户亲手打的目录被 NoEnding 换掉，比在一个不存在的目录里启动更糟（那是替用户做决定）。
`platform/` 已经知道 macOS `cd` 失败会静默落回 `$HOME`（M21），所以这里的诚实做法是
预览就说“这个目录不存在”，Launch 不追加解释。真要硬拒绝，那是命令层的产品决定，
需要 §22 文案一起改，归到 Wave 3 之后。

**E-(f)「删掉无后缀兼容变体，让编译器禁止没有 Home 的 launcher」— 推迟，理由记录。**
集成后生产路径对无后缀变体的调用为 **0**（已核），剩下的调用全在测试里（≈50 处，
`prepare_new` / `launch_prepared` / `apply_match` 等）。它们的风险是“将来有人误用”，
不是“现在错”。删它要一次性动 4 个测试文件，收益是编译期防呆——值得做，但不该塞在
Wave 2 收尾里做，因为它会掩盖同一轮里真正需要看的 diff。列入 §43.9 Wave 3 之后的收尾项。

**F2-1「§43.4-2 与 M29 冲突：派生链没有 Project 的 Session 该显示什么」— F2 对，取“弱化显示旧标签”。**
§43.4-2 说“不显示 Project”，M29 说详情页只走权威链。但列表里一个 legacy Session 身上
带着 v11 时期手工挂的标签，直接抹掉等于让用户以为数据丢了（§41 的失败模式）。折中：
标签保留但 dimmed + 标注为旧数据，不做任何可点击的 Project 归属；派生链一旦有值就替换它。
这条替换 §43.4-2 的口径。

**F2-2「NoEnding Home 与产品里的“首页”撞名」— 采纳 F2 的担忧，但不改词。**
UI 里“首页”= Home view，Settings 里“NoEnding Home”= 数据主目录，两个 “Home” 撞在
一次会话里确实会误读。裁决：保留 **NoEnding Home** 作为领域词的英文原名（§2 词表、
`~/.noending`、bootstrap pointer 都用它），Settings 里紧跟一句“NoEnding 存放数据的
主目录”，与“首页”并列出现时以中文说明为准。改名（“数据主目录”）会牵动
`workspace/home.rs` 的文档、迁移注释和用户已见过的路径，收益不及成本；若 §33 dogfood
里真的读到歧义反馈，再改。

**F1-1「§22 的删除警告过度承诺」— 采纳。** §22 的措辞暗示“删除路径会带走它的
Session”，实际语义是：只有**通过这条路径进来**的绑定会被带走（`workstream_path_id`
claim 命中的那些），无 claim 的绑定不动。文案改为“由这条路径带来的 N 个 Session”，
并且 N 必须由后端算，不在前端猜。M30 之后这个数字还多了一类兜底 Session。

**F1-2「§11 的 `get_project_detail.sessions[]` 没有上限」— 属实，Wave 3 之后处理。**
核实：`workspace/project.rs:1008-1013` 的 `ProjectDetail.sessions: Vec<Session>` 无
LIMIT。§43.6 的真实语料里 “Tmp” 一个 Project 就挂 393 个 Session，整个 `Session` 行
（含 `raw_path`、`title`）一次进 IPC。v0.2 阶段先接受（详情页可用，只是胖），但
§33 dogfood 若在 Windows 上表现明显，就改成 `sessions(limit, offset)`。不允许的解法是
前端截断——那会让“Project 有多少 Session”这个数自己撒谎。

## 43.9 Wave 3（Agent G）派发时必须带上

1. base = **`35c8c98`**（WAVE2_SHA）。G 只做 §24 / §25 / §26 的审计与跨平台，
   不新增智能（§40）、不改 Context Intelligence / Delivery（§31）。
2. 已生效的新规则：**M30**（兜底目录不进路径列表）、**M31**（新输入必须写明注入者）、
   **M32**（指纹哈希解析结果而非 cwd 字符串）、§42.3-M15 的“全部并列候选 AMBIGUOUS”。
3. 必查项（Main 自己没查完的）：
   * `list_sessions(projectId)` 仍在按 `sessions.project_id` **缓存**过滤
     （`storage/mod.rs:1277-1286`：`AND project_id = ?{}`，Main 已核实）。这是把派生列
     当 authority 用。§28 的 grep 名单里没有它，所以没人负责；G 要给出结论：要么改成
     走 `workspace_path_id → project_id` 的权威链，要么删参数。
   * 无后缀 launcher 变体的删除（E-(f)），以及删除后 4 个测试文件的等价性。
   * §42.5 T1–T4 落进 `.github/workflows/ci.yml`：T1 数 `generate_handler!` 的条目
     （不是注释里的提及），T2 必须和 `UPDATE sessions` 配对，否则正则会把注释也算进去。
   * §28 全量 grep 时，`default_cwd` 允许出现在 legacy schema / migration / 冻结列的
     guard 测试里；`project_resources` 表本身保留（legacy 读），命令不注册。
4. G 不改 UI 文案；文案问题按 §43.8 的口径报给 Main。

## 43.10 Wave 3 — Agent G：中途断线，Main 接手收尾

**G 没有交付 §27 报告。** 它在 150 turn 上限处被强制中断，最后一句是"One more
falsifiability check…"。所以本节全部结论由 Main 从 diff 与自己的复核重推，
**不接受任何未验证的转述**——包括 G 自己的 commit message。

断线时的现场：

```text
f9f2912 fix(workspace): stop reading derived caches and spellings as authorities   ← 完好
2b33ed0 test(workspace): lock the §25 matrix rows that had no assertion behind them ← 完好
未提交: src-tauri/src/launcher/mod.rs  1674 行 → 447 行，
        prepare_new / apply_match / resolve_new_cwd / try_match_launch_intents_in /
        compute_state_fingerprint_in 等核心函数全部消失
```

那是第二次"字符串手术边界过贪吞掉整段函数"（Main 自己在 Wave 1 也犯过一次，见
§43.2 的 `try_match_launch_intents` 事故）。处理方式：先把未提交 diff 存成补丁
（`/tmp/g-launcher-wip.patch`，2047 行，没丢东西），再 `git checkout --` 从 HEAD 还原，
然后才动任何别的东西。**注意 `2b33ed0` 里的 launcher 删除是完整且正确的**——断掉的
是它之后一次更进一步的未提交尝试。

合入：cherry-pick `f9f2912` `2b33ed0` → `ea1d8b5` `4da541d`，零冲突。
**WAVE3_SHA = `4da541d`**。

## 43.11 G 抓到的三个 invariant 违例（其中一个 Main 漏了）

1. **`add_context_item` 读冻结列 `workstreams.project_id` 并 `touch_project`**
   —— 这条在 §28 的 grep 名单里，**是 Main 的失职**：我把名单交给 G 跑，却没有自己
   先跑。它让一次 Context 编辑去按一个可能已被 §7.4 删除、§8.3 合并掉的成员关系
   重排 `list_projects`。现在改用 §42.3-M19 的 position-0 投影，这也清掉了那个退役列
   在 production 里的最后一个读者。
2. **`list_sessions(project_id)` 以派生缓存为查询谓词**（§43.9-1）—— 与
   `get_project_detail` 走的权威链不一致，正是 M29 判定"一个视图不能有两个真相"的那个
   形状：一条 legacy 行身上挂着 v11 手工标签但没有可解析 cwd 时，列表说它属于这个
   Project，详情页否认。改成 `EXISTS (… workspace_paths …)` 走链。
3. **"同一个位置吗"这类比较用了 `path_key`**（折叠分隔符但**保留大小写**）—— 在
   Windows 卷上是错的关系，而且有三处一旦判错就**无法自愈**：§2 的 reserved 集合
   （`data/` 的大小写变体会变成 WorkspacePath，进而变成围着 App 自己数据库的
   Project）、§1.4 的 Home 级 Git 排除（一个 toplevel 拼成 `C:\Users\ME` 的 dotfiles
   仓库会把整个用户 Home 变成一个 Project）、Home 迁移（大小写变体通过"和当前相同"
   的守卫，然后把 `data/` 搬到它自己身上）。新增 `identity::identity_key` /
   `same_location` / `ReservedPaths::contains_with`，遵守 M8.4：**只为比较折叠，
   从不为显示折叠**。非 Windows 逐字节不变 → 不动任何已存 identity，不触发迁移。

  `path_identity` **仍然不折叠**——改它会改已存的 `workspace_paths.id`，那是数据
  迁移，G 正确地报告而没有擅自做。→ 记为 **M33**。**§44 裁决：M33 在 Windows 侧
  关闭**（Windows 从未跑过 NoEnding，没有已存 id 可改），macOS 侧仍然不变。

顺带：`registry_is_consistent` 里一条恒真断言（`GROUP BY id HAVING
COUNT(DISTINCT project_id) > 1`，而 `id` 是主键，永远不可能 >1）换成了 §1.1 真正
要查的东西——每行的 id 是否就是它自己的 `canonical_path` 推导出来的那个。

新增机械守卫（§42.5 T1–T4，落在 `session_workspace_test.rs` 里 T2 的旁边）：T1 数
`generate_handler!` 的**条目**（Main 亲证可证伪：把 `create_project` 塞回注册表，
`retired_commands_are_not_registered` 立刻失败）；T3 用配对括号取 INSERT 列表；T4
对注释盲。G 还报告它对每条新断言做了 mutation 测试，**两条怎么改都不红的断言被删掉
而不是留着凑数**——这正是 §26 要的可证伪性，也是我在 brief 里要求的东西。

## 43.12 §28 / §29 最终核查（Main 亲自跑，2026-09-19）

退役命令在生产业务路径中的残留：**全部为注释或已退役定义**，且不再有注册。
`suggest_session_project` / `record_session_project_evidence` /
`suggest_project_for_session` / `merge_workstreams` 保留为只读历史入口，
且 T1 现在机器保证它们回不到 `generate_handler!`。

`UPDATE sessions SET project_id` 的写者集合**闭合**：src/ 里只剩
`session_paths.rs:161`（Project 失去最后一条路径时清成 NULL）这一条显式语句，
加上 `upsert_session` 语句内派生与 v12 迁移——正是 AGENTS.md 说的三个写者，
其余命中全是文档注释。前端已无手工 Project **指派** UI。

**遗留一条，记为 M34（Main 发现，未在本阶段修）**：
`sessions/SessionsView.tsx:98` 在**客户端**用 `s.project_id`（派生缓存）过滤，
第 200 行的下拉是筛选器不是指派器（合法），但它把 G 刚在服务端消掉的"两个真相"
在前端重新引入了：列表会按缓存把一个 Session 归到某个 Project，而详情页按链否认。
不在本阶段顺手修的理由要说清楚：正确的修法是**删掉客户端过滤**、把 `projectId`
交给已经走链的 `listSessions` 服务端查询，这会改变筛选的取数时机（每次切 Project
要重取），属于 UX 决策而不是不变量修复——半成品式地在客户端再拼一份链投影才是
更糟的选择。留到 §33 dogfood 之后与 Project 噪声口径（§43.6 的 a/b/c）一起定。

Wave 3 gate：`cargo fmt --check` 通过、`cargo check --all-targets` **0 warning**、
`cargo test --all-targets` **379 passed / 0 failed / 6 ignored**、`pnpm build` 通过。
CI（`.github/workflows/ci.yml`，已存在）在 **macOS + Windows 双平台**跑
`cargo test --all-targets`，因此 T1–T4 作为测试即被 CI 强制，**不需要改 CI 文件**。

## 43.13 真机可视化验证（Main，2026-09-19 20:00–20:18）

前面所有结论都是"测试通过 + 人眼读码"。这一节是**第一次真的把应用跑起来看**。

安全前提：`NOENDING_HOME=/tmp/noending-ui-verify` + 真实库的**副本**（db+wal+shm
三件套一起，M14）。原件全程未动（验证后确认 95084544 字节 / mtime 02:05 不变），
临时 Home 与副本用完即删。原生窗口驱动被系统权限挡住（`screencapture` 需屏幕录制、
`osascript` 需辅助访问），所以页面级验证走的是"临时把 invoke 打到
`public/ne-preview.json`（数据来自那份已迁移的副本）+ 浏览器渲染"，
用完 `git checkout index.html` 并删除 `public/`。

**端到端确实成立**：应用启动 → v11→v12 迁移 → Application Reconcile
（521 sessions discovered）→ Workspace Reconcile（`reconciled 27 paths, 9 discovered`）
→ 派生出 **27 个 Project / 36 条 WorkspacePath / 475 个 Session 全部有锚点**。
第二次启动报 `36 paths, 0 moved, 0 discovered` —— 真实数据上的重放幂等。

首页、Project 详情、Sessions、Settings、新建 Session 模态框都正常渲染，
产品语义也对：Project 详情页明说"你能改的只有名字；目录、成员 Workstream 与
Session 都由工作路径决定"；Sessions 页明说"按 Project 筛选只是换一种看法"。
`CwdRow` 的降级提示肉眼确认生效。

### 修掉的

**M34**（`SessionsView` 客户端按缓存过滤）：改为要求物理锚点
`!!s.workspace_path_id && s.project_id === projectId`，与 Project 徽章同一判据。
行为验证：筛 `Tmp` → 表格 393 行，与"有锚点的 Session 数"精确相等，
计数行正确显示"显示 393 / 共 400 条"。

### 新发现

- **M35｜§9 worktree 发现时没有做存在性观察 —— 已修。** `adopt_sibling_worktrees`
  原来把 `false` 当 `exists` 写死进去，于是首次渲染会对一个**就在盘上**的目录显示
  "目录不存在"，要等下一次 reconcile 才纠正。修法守住分层：`workspace/` 不自己碰文件
  系统，而是经 `WorkspacePolicy::exists_on_disk` **问**；观察实现放在 resolver
  （唯一被允许读盘的层），`HomePolicy` 与 `UnrestrictedWorkspace` 都委托过去。
  该方法是**故意没有默认实现**的——没接到真实主机的策略必须自己明说答案，
  不能继承一个关于用户磁盘的猜测。回归测试
  `adopted_worktrees_report_the_existence_that_was_observed` 两侧都断言（在盘上的
  兄弟工作树必须报存在；目录已消失的注册仍然是合法的 `exists=false` 观察，
  既不丢弃也不谎报存在），并已证伪：把 `false` 写回去，它以生产环境同一条断言失败。
  （Main 一度怀疑是前端 bug，查下来 UI 忠实渲染了后端的假数据，责任在后端。）
- **`runtime[f] !== null` 应为 `!= null`（`LaunchPreviewRows.tsx`）—— 已修。** 字段缺失
  （`undefined`）时会被当成"已 override"，界面上就打出
  `Provider undefined · Reasoning undefined`。真实后端总是把 `Option` 序列化成
  `null`，所以这是脆弱性而非现役 bug——但一个字符就能封掉。
- **Settings 右栏在 ~1440px 宽度下塌陷**：值徽章节（"首页"/"关闭"）被挤成一字一行
  竖排，提示文字断成"继 续最近的"。原生窗口更宽时未见此现象，属于窄视口下的
  响应式缺陷。
- **§43.6 的 Project 噪声在真界面上比数字更刺眼**：侧栏出现 4 个
  `Realtime-voice-chat`、2 个 `Deepseek-harness`（不同 Git 家族，按 §8.3 **正确地**
  没合并），以及 `To` / `Nin` / `Ke` / `Jian` / `She` / `Gei` 这类两三个字母的名字。
  查过库：这些是 `~/Documents/Codex/<日期>/<slug>` 里**真实的目录 basename**，
  不是命名截断——`auto_project_name` 是忠实的，脏的是数据本身。这把 §43.6 的
  选项 (b)（临时/一次性目录不建 Project）从"可选优化"变成了"界面可用性问题"。
- **4 个 Workstream 里 3 个是零路径**——M30 保护的那个状态在你的真实语料里
  是常态而不是边角。

## 44. Windows path identity 收口（封板前最后一项，2026-09-19）

### 44.1 为什么现在可以折叠

要修的不只是 `395066e` 的 Windows CI 红灯，而是一个 domain 洞：`same_location`
说两个 Windows 拼法是同一个目录，`path_identity` 说不是（`identity.rs:134` 折叠，
`identity.rs:161` 不折叠）。同一个问题有两个答案，正是 §42.5 T 系列要禁掉的形状。

§43.11 把这件事推给 M33 的唯一理由是"改它会改已存的 `workspace_paths.id`"。这个
理由只对有数据的平台成立：

```text
macOS    已有真实 v12 数据   → 逐字节不变，由 §44.5 的向量测试证明
Windows  从未运行过 NoEnding → 没有已存 id 可改
```

**前提一旦为假，本节就不是修复而是引入 bug，而且失效是静默的**：
`ensure_workspace_path_conn`（`project.rs:295`）按新规则重算 id → 查不到旧行 → 走
"全新路径"分支先 `create_project_row`、再 `insert_workspace_path_conn`
（`INSERT OR IGNORE`，`storage/workspace.rs:170`）被 UNIQUE 键吞掉。结果是一个目录
两个 Project、旧行原地不动、**没有任何错误返回**。`registry_is_consistent`
（`project.rs:1147`）能抓到这种行，但它是**只有测试在跑的守卫**（production 无调用
者），不会替用户发现。所以：Windows 首发前必须确认没有人跑过 v12；若跑过，本节
改写成 v13 backfill（重算 `workspace_paths.id` 及其全部外键引用），不是就地折叠。

### 44.2 冻结的定义

```text
canonical_path  = 保存与展示 = 保留用户原始大小写（不做任何 lowercase 写入）
location identity:
  Unix / macOS  = 分隔符归一 + 保留大小写
  Windows       = 分隔符归一 + 大小写折叠
```

`same_location` 与 `path_identity` 必须读同一个 location key；`path_identity_with`
/ `same_location_with` 是显式 style 的注入缝（沿用 `identity_key` /
`is_within_with` / `normalize_path_with` 已有的约定），生产入口仍只读宿主 style。
不新增第二套算法，不引入 `location_key` 列，不升 v13。

### 44.3 对用户方案的三处修正

**C1 — 两个红灯测试不按 §7 的 cfg 分叉修。** 本仓库的约定是**测试显式钉住
style**，而不是让"在哪个平台跑"决定语义：`ResolverContext.style`
（`resolver.rs`，`inert()` 里就是 `None = 宿主`）、`NoEndingHome::new_with_style`
都已存在，`home.rs:1042` 的模块自述写着 "every lexical test pins its style
instead of inheriting the host's"。cfg 分叉会让 Windows 分支只在 CI 上跑、Unix
分支在 Windows runner 上失守。改法：`normalization_and_exclusions_end_to_end`
显式 `style: Some(PathStyle::Unix)` 并补一段 Windows；
`relocation_request_writes_pending_only` 需要的缝还不存在，因此
`request_relocation` 补 `request_relocation_with(..., style)`，原函数退成宿主
wrapper。两处都断言**手写的**期望拼写，不重复调用被测函数——§7.1 警惕的自证。

**C2 — 要翻的断言不止两个，是五处。** 除 §7 那两个，还有
`identity.rs:585`（`assert_ne!(path_identity("/a/b"), path_identity("/a/B"))`，
Windows 宿主上折叠后会相等）、`identity.rs:670-673`（Windows 大小写变体保持分裂
的 `assert_ne!` 连同注释）、`workspace_identity_test.rs:160-168`（同一条，注释里
写的"Main 应该在任何 Windows 发布前重读这个取舍"就是本次裁决），以及 §44.5 的向量
测试本身——它必须走 `path_identity_with(..., PathStyle::Unix)`，否则在 Windows
runner 上自己变红。附带一个假门：`get_workspace_path_by_canonical`
（`storage/workspace.rs:123`）以拼写为键，在"拼写不再决定身份"之后会漏掉大小写
变体的已存行；唯一调用者是 `tests/launcher_workspace_test.rs:92`，让它走 identity
门，然后删掉这个 accessor。

**C3 — 折叠是"合并"，而合并不自愈；§44.1 的前提之外还有两种会被错误合并的真实
情况。** 规则 5 的兜底（Git `common_dir` 收敛）只对**分裂**成立。Windows 上
(a) 逐目录大小写敏感的 NTFS 目录（`fsutil file setCaseSensitiveInfo`，WSL 创建的
目录默认如此）和 (b) 宿主为 Windows 而被观察的是 POSIX 拼法
（`/home/u/Repo` 与 `/home/u/repo`）会被折成一个 WorkspacePath——`identity_key`
只看 style，不看路径形状。**仍然采纳 style 驱动而不是形状嗅探**：一旦"看起来像
Windows 路径才折叠"，`same_location` 与 `path_identity` 重新分叉，本节要关的洞又
开。接受它的理由是窄且可见：这两种情况下 NoEnding 在宿主上拿不到 `exists`/git
证据，而 `canonical_path` 保留首次拼写，UI 会显示成一个和用户目录对不上的路径。
这条写进 §44.6 的验收，不藏在实现注释里。

**C4 — §13 禁止动 Git identity，但 §9 的"一个 Project"必须动它才成立。**
`git_identities` 是按 `common_dir` 字符串精确查的
（`storage/workspace.rs::ensure_git_identity_conn`，`WHERE common_dir = ?1` +
`common_dir UNIQUE`），而 git 把调用它时用的那个 cwd 原样吐回来：同一个仓库以
`C:\Code\Repo\.git` 和 `c:\code\repo\.git` 两次被观察，就是两行 git identity、两个
`git_id`；§8.5 把"换了 family"当作强证据，把这条 WorkspacePath 迁去第二个
Project。**折叠 WorkspacePath 身份反而让分裂更容易发生**，§9 的
`windows_case_aliases_cannot_create_two_projects` 在只改 identity 的方案下必红。
裁决：`ensure_git_identity_conn` 的创建顺序改成"精确字符串 → location 关系 →
才插入"（`same_location`，因此 Unix 上等价于原来的比较，不动任何已存
`workspace_paths.id` / `projects.git_id`）。这是本次唯一的第二处 production 语义
变化，和被它绕过的 §13 一条禁令一起记在这里，不算"顺手改"。
顺带半处：`resolver::try_observe` 原来用宿主 style 查 reserved，注入
`PathStyle::Windows` 的测试因此只半生效，改成读 `ctx.style()`。

### 44.4 本次不动

schema、`workspace_paths` 表结构、Project 合并策略、Git 检测与 worktree 发现、
Home 迁移流程、launcher cwd 分层、Context、Assistant、Project UI、Workstream
生命周期。production 侧只碰三处，都有上面的理由：`identity.rs` 的 key 收敛本身、
§44.3-C1 的 `request_relocation_with` 缝（不改行为，只把已有的宿主读取换成参数）、
§44.3-C4 的 git identity 查找顺序（外加 `try_observe` 的 reserved 读 `ctx.style()`）。§12 的 TS shim 顺手清掉（Rust
`create_workstream` 已经只收 `title/description/initial_path`，`projectId` 是纯
死参数，`defaultCwd` 名不副实；只有一个调用点
`src/features/workstreams/NewWorkstreamModal.tsx:48`）。

### 44.5 Gate

macOS 本地：`cargo fmt --check`、`cargo check --all-targets`、
`cargo test --all-targets`、`pnpm build`。identity 回归至少要有：Windows 大小写
/ 分隔符 / UNC 三组同 identity（`path_identity_with(..., Windows)`，macOS runner
上就能跑完）、Unix 大小写保持分裂、以及**Unix v12 向量**——六个
`canonical → path-<hex>` 字面量由**独立实现**（`sha256("noending:workspace-path:v1:"
+ path_key)` 取前 16 字节）算出，不是跑一遍现有代码抄答案；它们在改动落地**之前**
先红过一次（`/Users/dev/./projects//noending` 那条就是），再对旧实现绿，才叫钉住
了兼容性。

push 之后只看新 HEAD 的 exact-head 运行，两个平台都要绿，且 **Windows 的
`frontend install` / `frontend build` 必须真的执行过**——`395066e` 那次是
`cargo test` 先红导致这两步被跳过，绿灯不能靠跳过。

### 44.6 验收

```text
Windows:  C:\Repo / c:\repo / C:/REPO  → 一个 WorkspacePath、一个 Project
macOS:    ~/Repo 与 ~/repo 仍然词法分裂；六个 v12 向量的 id 一字不变
显示:      canonical_path 保留用户拼写（首次写入者，后续观察只更新 exists/git）
比较:      same_location 与 path_identity 同源
不需要:    v13、Windows 历史迁移、location_key 列
```

外加 §44.1 的前提确认与 §44.3-C3 的已知合并风险。

### 44.7 落地记录（Main，2026-09-19）

改动：`identity.rs` 新增 `path_identity_with` / `same_location_with`，
`path_identity` 改为 hash `identity_key`（同一 location key），
`same_location` 走同一个注入缝；`resolver::try_observe` 的 reserved 查询与
`ensure_git_identity_conn` 的查找顺序按 §44.3-C4 调整；`request_relocation_with`
新增；`get_workspace_path_by_canonical` 删除，唯一调用点
`launcher_workspace_test.rs::path_id` 改走 identity 门（§44.3-C2）。
§8 的四个 key 测试落在 `identity.rs`，§9 的两个 registry 测试落在
`workspace_project_test.rs` 的 `#[cfg(windows)]` 段。TS 侧 §12 完成，
`createWorkstream(title, description, initialPath?)`。

macOS 本地 gate：`cargo fmt --check` / `cargo check --all-targets`（0 warning）/
`cargo test --all-targets` **386 passed, 0 failed, 6 ignored** / `pnpm build` 绿。

可证伪性，逐条交代：

- **亲证**：删掉 `ensure_git_identity_conn` 的 location 查找，
  `one_git_family_is_found_before_it_is_created` 立刻红（两次调用拿到两个 uuid）。
- **亲证，而且是以另一种方式**：§44.5 的六个向量先红过一次——我把
  `/Users/dev/./projects//noending` 当成"同一个 key"，而 `path_key` 只去尾分隔符、
  不折叠 `.` 和连续分隔符（那是 `normalize_path` 的活）。六条向量本身、以及"去尾
  分隔符不算第二个身份"那条，独立实现与 Rust 逐字节一致——所以被删掉的是我加错的
  断言，不是被放宽的期望。
- **本地无法证伪，只能靠 CI**：`#[cfg(windows)]` 那两个 registry 测试在 macOS 上
  编译成空。我把它们临时翻成 `cfg(not(windows))` 跑过 `cargo check` +
  `--list`，确认它们是真的会被编译、真的会跑的测试体，但**它们的红绿只有
  windows-latest 说得出**——所以 §44.5 要求去那次运行的日志里点名读它们。

前提的证据：`gh release list` 为空（从未发布过任何构建），加上 §44.1 记录的用户
声明"Windows 从未运行过 NoEnding"。M33 的 Windows 侧随本节关闭，macOS 侧保持
"折叠即迁移"的原判。
