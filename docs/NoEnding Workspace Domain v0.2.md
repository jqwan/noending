# NoEnding Workspace Domain v0.2

> **命名对齐（v0.2.1）**：本文 §15–§17 原本用 `status` + `trashed_at` 描述 Workstream
> 生命周期。实施方案（`NoEnding Workspace Domain v0.2 — 多 Agent 并行执行方案.md` §1.13/§5.7）
> 决定复用现有两个正交字段以避免 schema 扩张，本文已就地改为该口径：
>
> ```text
> status     ≡ lifecycle   （active | completed）
> trashed_at ≡ visibility  （normal | archived；archived 即回收站）
> ```
>
> 同理，§3–§4 的 `path_id` 与实施方案 §5.3 的 `workspace_paths.id` 是同一件事，
> 落地后只有 `id` 一列（确定性派生自 canonical path）。
> 实现期的勘误与缺失规则一律看实施方案 §42。

## 1. 目标

这一阶段的目标不是增加智能能力，而是建立稳定、可解释的 Workspace 领域模型，使 NoEnding 能够自动理解：

```text
物理工作路径
    ↓
WorkspacePath
    ↓
Project
    ↓
Workstream
    ↓
Session
```

同时明确以下原则：

```text
Project
= 应用自动维护的物理工作空间归纳

Workstream
= 用户定义的持续工作单元

Session
= Agent 的真实执行记录

WorkspacePath
= 物理文件系统与上述领域对象之间的桥梁
```

Project 不再承担用户自定义语义分类。

未来如需要按“客户 / 主题 / Release / 业务”等维度组织 Workstream，应单独设计 Collection / Category 一类用户语义分类功能，并允许未来智能辅助。

---

# 2. NoEnding Home

NoEnding 使用一个统一的数据根目录：

```text
~/.noending/
```

Windows 对应：

```text
%USERPROFILE%\.noending\
```

默认结构：

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
~/.noending
= NoEnding Home

~/.noending/data
= 应用持久化数据

~/.noending/workspace
= 默认工作路径
```

`data / runtime / logs` 属于应用内部保留路径，不得参与 Project / WorkspacePath 自动发现。

`workspace` 是正常用户工作路径，可以正常产生 WorkspacePath 和 Project。

用户可以修改 NoEnding Home。

因为应用启动前必须先知道数据库位置，所以 Home 路径不能只存在数据库内部。需要一个极小的 bootstrap 配置保存当前 NoEnding Home。

解析顺序建议：

```text
NOENDING_HOME
    ↓
OS bootstrap config
    ↓
~/.noending
```

修改 NoEnding Home 时，应用数据可以迁移，但已经存在用户文件的旧 `workspace` 不应静默移动。

新的默认 workspace 改为：

```text
<new-noending-home>/workspace
```

旧 workspace 此后只是普通 WorkspacePath。

---

# 3. WorkspacePath

WorkspacePath 是整个 Workspace Domain 的基础事实。

建议领域模型：

```rust
WorkspacePath {
    id,
    path_id,
    canonical_path,
    project_id,
    git_state,
    exists,
    first_seen_at,
    last_seen_at,
}
```

其中：

```text
id
= NoEnding 内部稳定 UUID

path_id
= canonical path 的稳定身份

canonical_path
= 规范化后的绝对路径

project_id
= 唯一所属 Project

git_state
= none | detected | missing
```

核心约束：

```text
一个 WorkspacePath 在任意时刻只能属于一个 Project。
```

关系为：

```text
Project 1 ───── N WorkspacePath
```

不允许 WorkspacePath 同时属于两个 Project。

---

# 4. Path Identity

任何工作路径都必须首先得到 path identity。

处理：

```text
input path
    ↓
expand ~
    ↓
absolute
    ↓
normalize separators
    ↓
canonicalize when possible
    ↓
canonical_path
    ↓
path_id
```

`path_id` 永远存在。

Git 是否存在不能影响 path identity。

例如：

```text
/Users/me/code/noending
```

即使：

```text
.git 存在
.git 被删除
重新 git init
```

它仍然是同一个 WorkspacePath / path_id。

---

# 5. Project

Project 是应用自动维护的物理工作空间聚合实体。

建议模型：

```rust
Project {
    id,
    git_id,
    name,
    name_customized,
    created_at,
    updated_at,
}
```

其中：

```text
id
= NoEnding UUID，永远稳定

git_id
= 可为空
= Project 级 Git workspace family identity

name
= 默认自动生成，可被用户修改

name_customized
= 防止后续自动发现覆盖用户名称
```

Project 没有：

```text
active
completed
archived
trashed
```

这样的生命周期状态。

Project 的存在完全由 WorkspacePath 决定。

核心 invariant：

```text
Project MUST own >= 1 WorkspacePath.
```

一旦最后一个 WorkspacePath 被重新归属或删除：

```text
WorkspacePath count = 0
→ 自动删除 Project
```

因此不需要 orphan Project 生命周期。

---

# 6. Project 的用户能力

Project 完全由应用管理。

用户不能：

```text
手动新建 Project
手动为 Workstream 选择 Project
手动为 Session 选择 Project
手动把路径移动到任意 Project
```

用户唯一正常编辑能力是：

```text
重命名 Project
```

Project 的删除也不是普通用户直接删除领域对象。

如果 Project 下仍然存在 WorkspacePath，Project 必须存在。

只有 WorkspacePath 被合法移除或重新归属，导致 Project 无路径后，Project 自动消失。

---

# 7. Git Identity

`git_id` 属于 Project，而不是 WorkspacePath。

准确关系：

```text
WorkspacePath
    ↓ Git detection
Git family evidence
    ↓
Project.git_id
```

也就是：

> WorkspacePath 负责发现 Git identity，Project 负责持有 Git identity。

同一个非空 `git_id` 全局只能对应一个 Project。

数据库层应建立：

```sql
UNIQUE(projects.git_id)
WHERE git_id IS NOT NULL
```

---

# 8. Git ID 不是 path hash

不建议把：

```text
hash(.git path)
```

直接当长期 Project identity。

更稳妥的方式是由 NoEnding 为确认过的 Git family 分配稳定 UUID：

```text
git_id = git-xxxx
```

并维护 Git identity evidence，例如：

```text
common_dir
main worktree
known worktrees
remote fingerprint（辅助）
first_seen_at
last_seen_at
```

Git evidence 用于识别、恢复和合并。

Project.id 本身永远不会因为：

```text
普通目录 → Git repo
```

而变化。

---

# 9. 排除 ~/.git

必须特殊处理用户 Home 级 Git 仓库。

如果检测结果为：

```text
git root == user home
```

或者：

```text
git common dir == ~/.git
```

则：

```text
忽略 Git identity
仅使用 path identity
```

防止 dotfiles 仓库把整个 Home 下的目录错误归纳成一个 Project。

---

# 10. `.git` 被删除

`.git` 的存在状态属于 WorkspacePath，而不是 Project。

例如：

```text
Project NoEnding
git_id = G1

P1 ~/code/noending
git_state = detected

P2 ~/worktrees/noending/core
git_state = detected
```

如果用户删除：

```text
~/worktrees/noending/core/.git
```

则只发生：

```text
P2.git_state:
detected → missing
```

保持：

```text
P2.project_id = Project NoEnding
Project.git_id = G1
```

不允许因为 `.git` 缺失：

```text
清空 Project.git_id
新建 Project
拆分 Project
```

核心规则：

```text
Loss of .git on a WorkspacePath
MUST NOT by itself detach that WorkspacePath
or change its Project.
```

---

# 11. `.git` 恢复

如果 WorkspacePath 后续重新检测到原 Git family：

```text
missing → detected
```

Project 不发生变化。

如果重新检测到不同 Git family：

```text
old Project.git_id = G1
current detected Git family = G2
```

说明这个路径的物理含义发生变化。

此时 WorkspacePath 可以迁移到 G2 所属 Project。

若 G2 尚不存在，则创建新 Project。

迁移后：

```text
旧 Project 仍有 WorkspacePath
→ 保留

旧 Project 0 WorkspacePath
→ 自动删除
```

---

# 12. Project 的生成逻辑

Project 不由用户创建，而由 WorkspacePath 自动 ensure。

完整流程：

```text
Working Path
    ↓
Canonicalize
    ↓
path_id
    ↓
ensure WorkspacePath
    ↓
detect Git
    ↓
resolve Project
```

具体规则如下。

| 场景                               | 行为                        |
| -------------------------------- | ------------------------- |
| 已知 path，无 Git                    | 保持原 Project               |
| 新 path，无 Git                     | 创建 path-backed Project    |
| 新 path，有 Git，git_id 已存在          | WorkspacePath 加入该 Project |
| 新 path，有 Git，git_id 不存在          | 创建 git-backed Project     |
| 已知 path，之前无 Git，现在发现 Git         | 原 Project 升级持有 git_id     |
| 已知 path 发现的 git_id 已属于另一 Project | 自动 merge Project          |

所谓：

```text
path-backed Project
```

仅表示：

```text
Project.git_id = null
```

Project 本身没有不同类型。

---

# 13. Project Merge

例如：

```text
Project A
git_id = null
└─ P1

Project B
git_id = null
└─ P2
```

后来发现：

```text
P1
P2
```

属于同一 Git family：

```text
G1
```

系统选定一个 canonical Project，例如 A：

```text
A.git_id = G1

P2.project_id:
B → A
```

于是：

```text
Project A
├─ P1
└─ P2
```

Project B：

```text
WorkspacePath count = 0
```

自动删除。

用户自定义名称优先于自动名称。

Project merge 不需要额外 lifecycle。

---

# 14. Git Worktree

Git Project 可以通过：

```text
git worktree list --porcelain
```

发现同一 Git family 下的关联路径。

例如：

```text
~/code/noending
~/worktrees/noending/core
~/worktrees/noending/windows
```

它们拥有不同：

```text
path_id
```

但属于同一个：

```text
Project.git_id
```

因此：

```text
Project NoEnding
├─ WorkspacePath P1
├─ WorkspacePath P2
└─ WorkspacePath P3
```

Git worktree discovery 可以自动增加 Project 的 WorkspacePath。

但必须遵守：

```text
ProjectPath discovery
≠
WorkstreamPath addition
```

发现一个 worktree 不能自动给任何 Workstream 增加工作路径。

---

# 15. Workstream

Workstream 仍然是 NoEnding 的核心连续性单位。

建议模型：

```rust
Workstream {
    id,
    title,
    description,
    status,
    trashed_at,
    created_at,
    updated_at,
}
```

不再以：

```text
project_id
default_cwd
```

作为核心事实。

它们被 WorkstreamPaths 取代。

---

# 16. Workstream 状态

Workstream 有两个正交概念：

```text
status
= active | completed
```

以及：

```text
trashed_at
= null | timestamp
```

`active / completed` 只是基础分类。

两者：

```text
没有真实功能差异
用户可以随时切换
```

进入回收站：

```text
trashed_at = now
```

不会改变：

```text
status
paths
Session bindings
其他配置
```

恢复：

```text
trashed_at = null
```

自然恢复原来的 active / completed 状态。

---

# 17. Workstream 回收站和永久删除

UI：

```text
Workstreams

进行中
已完成
回收站
```

只有：

```text
trashed_at IS NOT NULL
```

的 Workstream 可以彻底删除。

永久删除：

```text
删除 Workstream
删除 WorkstreamPaths
删除 Session ↔ Workstream bindings
删除 Workstream-owned Context / Review 等数据
```

但不得删除：

```text
Session
Session Events
Agent 原始历史
WorkspacePath
```

除非 WorkspacePath 后续满足自己的 GC 条件。

---

# 18. WorkstreamPaths

Workstream 的工作路径不是 primary/secondary 两组，而是：

> 一个有序列表。

模型：

```rust
WorkstreamPath {
    id,
    workstream_id,
    workspace_path_id,
    position,
    source,
    created_at,
}
```

例如：

```text
0  ~/worktrees/noending/core
1  ~/code/noending
2  ~/code/docs
```

语义：

```text
position = 0
→ 主工作路径

position > 0
→ 次工作路径
```

不需要单独保存：

```text
role = primary / secondary
```

---

# 19. Workstream Path invariant

必须始终满足：

```text
paths.length == 0
OR
paths[0] is the primary path
```

绝不允许：

```text
没有主路径
但存在次路径
```

因为只要列表非空：

```text
第一个自然是主路径
```

---

# 20. Workstream Path 删除

例如：

```text
0 /A
1 /B
2 /C
```

删除 `/A`：

```text
0 /B
1 /C
```

`/B` 自动成为主路径。

不需要用户额外指定新的主路径。

删除中间路径：

```text
0 /A
1 /C
```

重新压紧 position 即可。

---

# 21. Workstream Path reorder

用户把：

```text
/C
```

设为主路径，本质就是：

```text
0 /C
1 /A
2 /B
```

Session binding 不改变。

需要触发：

```text
Launcher cwd resolution refresh
Project projection refresh
PreparedLaunch stale
```

因为主工作路径已经发生变化。

---

# 22. Workstream 与 Project

Workstream 不再直接拥有单个：

```text
project_id
```

Workstream 可能通过不同 WorkspacePath 同时出现在多个 Project。

例如：

```text
Workstream W

0 /repo/noending
1 /repo/docs
```

如果：

```text
/repo/noending → Project A
/repo/docs     → Project B
```

那么：

```text
Project A → Workstream W
Project B → Workstream W
```

其中 Project A 可以显示：

```text
主关联
```

Project B：

```text
关联
```

但这些都是 projection。

唯一事实来源：

```text
WorkstreamPath
    ↓
WorkspacePath
    ↓
Project
```

不需要单独 `workstream_project_bindings`。

---

# 23. Session

Session 是独立执行事实。

建议模型逐渐演进为：

```rust
Session {
    id,
    agent,
    agent_session_id,
    cwd,
    path_id,
    project_id,
    ...
}
```

其中：

```text
path_id
= authoritative workspace relation

project_id
= derived / cached
```

Session 不允许用户手工选择 Project。

---

# 24. Session → Project

Session 的 Project 永远从：

```text
Session.path_id
    ↓
WorkspacePath.project_id
    ↓
Project
```

派生。

因此即使是 standalone Session：

```text
Session
没有 Workstream
cwd = /repo/noending
```

仍然可以属于：

```text
Project NoEnding
```

Project 可以自然展示所有：

```text
Workstream Session
Standalone Session
```

---

# 25. Session 什么时候刷新 Project

Session 不应该每次同步事件都重新计算 Project。

只有两类事实变化需要刷新：

```text
Session.path_id changed
```

或者：

```text
WorkspacePath.project_id changed
```

因此规则：

```text
Session path changed
OR
WorkspacePath → Project mapping changed
    ↓
refresh Session.project_id
```

典型触发点：

```text
Session 首次发现
新建 Session 被 LaunchIntent 匹配
Resume 后发现 cwd 发生变化
WorkspacePath 从 path-backed 升级为 Git Project
Project merge
WorkspacePath 因新 Git evidence 改归另一 Project
```

普通：

```text
Session events ingested
```

不触发 Project refresh。

---

# 26. WorkspacePath reconcile

建议所有 WorkspacePath 更新统一走：

```rust
reconcile_workspace_path(path_id)
```

职责：

```text
刷新 exists
刷新 Git detectability
识别 Git family
ensure / merge Project
更新 WorkspacePath.project_id
批量刷新引用该 path 的 Session.project_id
刷新受影响的 Workstream Project projection
```

避免 Launcher、Session discovery、Git resolver 各自实现一套 Project 逻辑。

---

# 27. Session 加入 Workstream

用户可以随时把已有 Session 加入 Workstream。

流程：

```text
bind Session → Workstream
```

如果：

```text
Session.cwd / path_id
```

已经存在于 WorkstreamPaths：

```text
只建立 binding
```

如果不存在：

```text
WorkstreamPaths 为空
→ append 后自然成为 position 0

WorkstreamPaths 非空
→ append 到列表末尾
```

然后触发 Project projection 更新。

---

# 28. Workstream 新建 Session

Workstream 新建 Session 的 cwd：

```text
用户本次显式 cwd
    ↓
WorkstreamPaths[0]
    ↓
NoEnding default workspace
```

如果 Workstream 没有任何路径：

```text
cwd = ~/.noending/workspace
```

当真实 Session 被发现并与 LaunchIntent 匹配后：

如果该路径不在 WorkstreamPaths：

```text
paths empty
→ 添加为 position 0

paths not empty
→ append
```

然后：

```text
Session binding
WorkspacePath ensure
Project reconcile
```

最好在真实 Session 出现后再持久化，不要仅按“点击 Launch”提前污染 Workstream。

---

# 29. Standalone New Session

没有 Workstream 时：

```text
用户指定 cwd
    ↓
NoEnding default workspace
```

默认：

```text
~/.noending/workspace
```

Standalone Session 依然：

```text
Session.path_id
→ WorkspacePath
→ Project
```

因此它仍然可以产生 / 加入 Project。

---

# 30. Resume Session

优先：

```text
Session.cwd
```

如果原 cwd 已不可用：

```text
Workstream 主路径（如果存在唯一目标 Workstream）
    ↓
NoEnding default workspace
```

发生 fallback 时应明确告诉用户。

---

# 31. Session 从 Workstream 移除

用户移除一个 Session：

```text
只删除 Session ↔ Workstream binding
```

不自动删除 WorkstreamPath。

因为 WorkstreamPath 是 Workstream 自己的持续工作配置，不应该因为最后一个 Session 被移除而自动消失。

---

# 32. 从 Workstream 移除工作路径

这是一个强操作。

规则：

```text
remove WorkstreamPath
    ↓
移除 Workstream 中属于该路径的所有 Session bindings
    ↓
删除 WorkstreamPath
    ↓
重新编号 positions
    ↓
如果 position 0 被删
第二条自然成为新的主路径
    ↓
刷新 Project projection
```

为了避免路径嵌套产生误删：

```text
/repo
/repo/frontend
```

建议：

```text
session_workstream_bindings
```

增加：

```text
workstream_path_id
```

明确记录 Session 是通过哪条 WorkstreamPath 加入 Workstream 的。

这样删除 `/repo/frontend` 时，只移除：

```text
binding.workstream_path_id = frontend
```

的 binding。

---

# 33. 增加路径不会自动增加 Session

Workstream 新增：

```text
/repo/backend
```

只增加：

```text
WorkstreamPath
```

不会去扫描并自动把 `/repo/backend` 下所有历史 Session 加进 Workstream。

这符合：

```text
路径管理
≠
Session 语义归属
```

用户可以后续手动加入 Session。

未来可以做智能建议，但不能作为基础行为。

---

# 34. WorkspacePath 删除

WorkspacePath 是系统观察到的物理事实，不应该轻易物理删除。

只有：

```text
0 Session references
AND
0 WorkstreamPath references
```

才允许真正删除 WorkspacePath。

如果实际目录不存在但仍有历史引用：

```text
exists = false
```

即可。

随后：

```text
DELETE WorkspacePath
    ↓
Project WorkspacePath count - 1
    ↓
如果为 0
DELETE Project
```

---

# 35. Project 自动删除

Project 的删除条件最终不再直接看：

```text
Session count
Workstream count
```

而是更基础的：

```text
WorkspacePath count == 0
```

因为：

```text
Session
→ WorkspacePath

Workstream
→ WorkstreamPath
→ WorkspacePath
```

只要仍然存在合法引用，WorkspacePath 就不会被删除。

因此：

```text
Project 0 WorkspacePath
```

自然意味着它没有任何有效物理锚点。

此时自动删除 Project。

---

# 36. Project Detail

Project 页面应该逐渐变成真正的物理 workspace 总览：

```text
NoEnding

Workspace Paths
────────────────────

~/code/noending
Git main worktree

~/worktrees/noending/core
Git worktree

~/worktrees/noending/windows
Git worktree


Workstreams
────────────────────

Core Workspace Experience
主关联

Windows Support
关联


Sessions
────────────────────

Codex   Core Workspace Experience
Claude  standalone
...
```

Project 页面不提供：

```text
新建 Project
手动选择 Workstream
手动添加 Session
```

只提供：

```text
重命名 Project
```

以及合理的路径状态展示。

---

# 37. Project 自动命名

普通目录：

```text
/Users/me/research
→ Research
```

Git Project：

优先基于主 workspace / repo root basename：

```text
/Users/me/code/noending
→ NoEnding
```

默认 workspace：

```text
~/.noending/workspace
→ NoEnding Workspace
```

用户重命名后：

```text
name_customized = true
```

后续 Git 检测、worktree 增加、Project merge 不再自动覆盖名字。

---

# 38. 当前字段迁移方向

当前：

```text
Project.archived
Workstream.project_id
Workstream.default_cwd
Session.project_id
```

都需要重新定义。

目标：

```text
Project.archived
→ 退出领域模型

Workstream.project_id
→ deprecated / compatibility only
→ 最终删除

Workstream.default_cwd
→ migration 为 WorkstreamPaths[0]
→ 最终删除

Session.project_id
→ 保留作为 derived cache 可以接受
→ 不允许成为独立事实
```

现有：

```text
project_resources
```

也应重新审视。

Git workspace path 不应再依赖通用 Resource 表，而应该由正式 WorkspacePath 模型承担。

普通文档 / URL / artifact Resource 是否继续保留，可以在后续单独判断，不阻塞 Workspace Domain。

---

# 39. 核心领域 invariant

最终必须通过测试锁死：

```text
1.
Every WorkspacePath belongs to exactly one Project.

2.
Every Project owns at least one WorkspacePath.

3.
A Project with zero WorkspacePaths is deleted automatically.

4.
Every non-null git_id belongs to exactly one Project.

5.
WorkspacePath owns Git detectability;
Project owns Git identity.

6.
Loss of .git on a WorkspacePath does not by itself
change WorkspacePath.project_id or Project.git_id.

7.
A Workstream has an ordered list of paths.
If non-empty, index 0 is always its primary path.

8.
A Workstream may never have secondary paths without
a primary path.

9.
Session Project membership is always derived from
Session.path_id → WorkspacePath.project_id.

10.
Workstream Project membership is always derived from
WorkstreamPath → WorkspacePath.project_id.

11.
Removing a Session from a Workstream does not remove
the WorkstreamPath.

12.
Removing a WorkstreamPath removes only Session bindings
belonging to that WorkstreamPath.

13.
Adding a WorkstreamPath never automatically imports Sessions.

14.
active / completed has no behavioral difference.

15.
Trash preserves the Workstream's previous status, paths,
bindings and configuration until permanent deletion.

16.
Permanent Workstream deletion never deletes Session history.
```

---

# 40. 最终领域关系

```text
NoEnding Home
│
└── default workspace


Project
│
│ 1
│
└────────── N WorkspacePath
                │
                ├──────── Session
                │
                │
                └──────── WorkstreamPath
                              │
                              │ N
                              │
                              1
                          Workstream
```

Git 是：

```text
WorkspacePath
    ↓ detect
Git evidence
    ↓
Project.git_id
```

而不是：

```text
WorkspacePath.git_id
```

作为最终领域身份。

---

# 41. 实施阶段

建议将下一阶段正式命名为：

```text
Workspace Domain v0.2
```

推荐执行顺序：

```text
Domain invariants + migration design
    ↓
NoEnding Home / default workspace
    ↓
WorkspacePath registry
    ↓
WorkspaceResolver
    ↓
Project auto-generation / merge
    ↓
Git / worktree detection
    ↓
Workstream ordered paths
    ↓
Session path / Project refresh
    ↓
Session ↔ Workstream binding path ownership
    ↓
Workstream lifecycle / trash / permanent deletion
    ↓
Launcher cwd resolution
    ↓
Project / Workstream UI
    ↓
Migration tests + integrity tests
```

这一阶段不涉及：

```text
Context Extraction
Context Injection
Assistant
智能 Workstream 分类
用户自定义语义分类
```

这些继续冻结。

---

# 42. 产品模型最终定义

一句话定义四个核心对象：

```text
Project
= NoEnding 自动维护的物理 workspace family

WorkspacePath
= 一个具体、稳定识别的物理工作路径

Workstream
= 用户长期持续推进的一件工作

Session
= 某个 Agent 的一次真实执行
```

最终：

```text
Physical workspace
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
将两者连接起来。
```

这就是 Workspace Domain v0.2 的最终基础模型。
