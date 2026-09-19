# NoEnding Session Lifecycle & Deletion v0.1

## 0. 阶段目标

为 NoEnding 中收录的 Session 建立完整生命周期：

```text
Normal
  ↓
Trash
  ├─ Restore
  │
  └─ Permanent Delete
          ↓
   删除 Agent 源 Session
          ↓
   删除 NoEnding Session 数据
```

最终语义：

```text
Trash
= 可逆隐藏。
= 不删除 Agent 源 Session。
= 不删除 NoEnding Session 数据。

Permanent Delete
= 删除当前 Agent 源 Session。
= 删除当前 NoEnding Session 及其 Session-owned 数据。
= 不保存 Session tombstone。
= 不永久 blacklist agent_session_id。

Future Rediscovery
= 如果用户后来恢复 Agent 源 Session，
  NoEnding 将其作为全新的 Session 再次发现和摄入。
```

---

# 1. 核心领域 invariant

本阶段首先冻结以下规则。

```text
1. Trash is reversible.

   Trash → Restore
   必须保留同一个 NoEnding Session.id。


2. Trashing a Session never deletes Agent source data.


3. A trashed Session is inactive inside NoEnding:

   - 默认列表隐藏
   - Project / Workstream 默认列表隐藏
   - 搜索隐藏
   - 不允许 Resume
   - 不继续 ingest / sync


4. Discovery may observe a trashed source Session,
   but MUST NOT restore it or update it automatically.


5. Permanent deletion is only allowed from Trash.


6. Permanent deletion order:

   Agent source deletion
       ↓ success
   NoEnding Session purge

   源删除失败时不得 purge NoEnding Session。


7. Core MUST NEVER perform:

   remove_file(session.raw_path)

   Source deletion is always Agent-adapter-owned.


8. Permanent deletion keeps no Session tombstone.

   完成以后 NoEnding 不再保存：
   - Session.id
   - agent_session_id
   - title
   - cwd
   - raw_path
   - deletion record


9. A genuinely restored Agent source Session may be discovered again.

   old:
   Agent X → NoEnding S1 → permanently deleted

   later:
   restored Agent X → NoEnding S2

   S1 != S2


10. Rediscovery never restores old:

    - Workstream bindings
    - Session events
    - cursors
    - SyncRuns
    - launch history
    - Session provenance payload


11. Workstream Context is independent of Session lifetime.

    Permanent Session deletion does not delete surviving
    Workstream Context items/revisions.


12. Context provenance referring to a deleted Session must be redacted,
    not left dangling and not redirected to a future S2.
```

---

# 2. Schema v13

Workspace Domain v0.2 已经 sealed 为 schema v12。

本阶段：

```text
SCHEMA_VERSION = 13
```

迁移是增量迁移，不修改 WorkspacePath / Project identity。

---

# 3. Session Trash 数据模型

不要增加：

```text
visibility
+
trashed_at
```

两个 authority。

Session 使用单一字段：

```sql
ALTER TABLE sessions
ADD COLUMN trashed_at TEXT;
```

定义：

```text
trashed_at IS NULL
→ Normal

trashed_at IS NOT NULL
→ Trash
```

Domain：

```rust
Session {
    ...
    trashed_at: Option<String>,
}
```

TypeScript 同步：

```ts
interface Session {
  ...
  trashed_at: string | null;
}
```

---

# 4. 临时删除协调表

永久删除跨越：

```text
SQLite
+
filesystem
```

无法做真正的数据库事务。

因此新增一个**临时协调表**：

```sql
CREATE TABLE session_deletion_jobs (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL UNIQUE
                REFERENCES sessions(id),

    state       TEXT NOT NULL,

    plan_json   TEXT NOT NULL,
    last_error  TEXT,

    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
```

允许状态：

```text
prepared
deleting_source
failed
stale
```

它不是：

```text
deletion history
tombstone
blacklist
```

只用于协调正在执行的永久删除。

---

# 5. 删除成功后不保留 deletion job

永久删除最终成功的同一个 SQLite transaction 中：

```text
DELETE session_deletion_jobs
DELETE sessions
```

一起提交。

因此完成之后数据库中：

```text
不存在 Session
不存在 deletion job
不存在 tombstone
```

NoEnding 不知道：

```text
这个 Agent Session 曾经被删除过。
```

---

# 6. Trash 操作

新增：

```rust
trash_session(session_id)
```

前置条件：

```text
Session exists
trashed_at == null
```

执行：

```text
sessions.trashed_at = now
```

并：

```text
unindex Session from FTS/search
```

不能修改：

```text
Session Events
Cursor
Bindings
WorkspacePath
Project
WorkstreamPath
Agent source file
```

---

# 7. Restore

新增：

```rust
restore_session(session_id)
```

前置：

```text
Session exists
trashed_at != null
无 active session_deletion_job
```

执行：

```text
trashed_at = NULL
reindex Session
```

不创建新的 Session。

因此：

```text
S1
Normal → Trash → Restore
```

始终还是：

```text
S1
```

Bindings / Events / cursors 全部继续存在。

---

# 8. Trash 后停止 ingestion

这一点必须在 backend 锁死，不能只靠 UI。

当前：

```text
discover
→ ensure_session_row
→ ingest
→ sync
```

修改为：

```text
discover
→ find existing Session
→ if trashed_at != null:
      return existing unchanged
      DO NOT ingest
      DO NOT sync
      DO NOT update metadata
```

即使 Agent source Session 在 Trash 期间继续变化：

```text
NoEnding 不更新它。
```

恢复后，下一次 reconcile：

```text
重新从 cursor/source 状态继续
```

如 Agent source 在 Trash 期间发生 truncate/rewrite，现有 ingestion rewrite detection 负责正常处理。

---

# 9. 防止正在执行的 ingestion 穿透 Trash

当前 ingestion 是多阶段的：

```text
read source
→ extraction
→ DB commit
```

因此可能出现：

```text
T0 ingestion 开始
T1 用户 Trash
T2 ingestion commit
```

如果不加保护：

```text
已经进 Trash 的 Session
仍然会写 Events / Context
```

这是 invariant violation。

所以最终写入前必须重新检查：

```text
session exists
AND
trashed_at IS NULL
```

若已 Trash：

```text
abort / no-op commit
```

不能写：

```text
session_events
cursor
sync_run
context mutations
```

---

# 10. Trash 后禁止 Resume

所有 Resume 入口必须检查：

```text
session.trashed_at == null
```

包括：

```text
prepare_resume_session
launch_resume_session legacy path
launch_prepared validation
```

已经 prepare 的 Resume：

```text
Prepare
→ Session moved to Trash
→ launch_prepared
```

必须返回：

```text
PreparedLaunch stale
```

不能启动。

---

# 11. Session 列表 scope

当前 `list_sessions` 默认只能看到正常 Session。

建议新增：

```rust
enum SessionListScope {
    Active,
    Trash,
    All,
}
```

默认：

```text
Active
```

SQL：

```text
Active
→ trashed_at IS NULL

Trash
→ trashed_at IS NOT NULL

All
→ no trash predicate
```

Project / Workstream / Home：

```text
默认只显示 Active
```

---

# 12. Search

Trash：

```text
unindex Session
```

Restore：

```text
reindex Session
```

所以普通 Search 不需要增加复杂 filter。

Trash 页面直接查询 DB，不走 FTS。

---

# 13. Agent Source Deletion Capability

扩展：

```rust
trait AgentAdapter
```

增加：

```rust
fn prepare_source_session_deletion(
    &self,
    session: &Session,
) -> Result<SourceDeletionPlan>;

fn execute_source_session_deletion(
    &self,
    plan: &SourceDeletionPlan,
) -> Result<SourceDeletionOutcome>;
```

默认实现：

```text
Unsupported
```

Core 不能自己删除文件。

---

# 14. SourceDeletionPlan

建议：

```rust
SourceDeletionPlan {
    version: u32,

    agent: Agent,
    agent_session_id: String,

    targets: Vec<SourceDeletionTarget>,
}
```

Target：

```rust
SourceDeletionTarget {
    path: String,
    kind: String,

    file_identity: String,
    size: u64,
    sha256: String,
}
```

`sha256` 是准备永久删除时计算的完整文件 hash。

永久删除是低频操作，优先完整性，不需要为 hash 节省几毫秒。

---

# 15. 当前三个 Adapter 的第一版实现

当前 Codex / Claude Code / Pi discovery 都以单个 JSONL 文件作为 Session source。

因此 v0.1 可以实现：

```text
Codex
→ exact rollout-*.jsonl

Claude Code
→ exact discovered *.jsonl

Pi
→ exact discovered *.jsonl
```

但逻辑必须存在于各自 Adapter。

不能 Core 假设：

```text
所有 Agent 永远都是单文件 JSONL。
```

---

# 16. Adapter prepare 验证

在返回删除计划之前，Adapter 必须证明：

```text
1. raw_path 指向 regular file
2. 不是 symlink
3. content fingerprint 属于当前 Agent
4. 文件中解析出的 Agent session id
   == session.agent_session_id
5. 文件就是 discovery 会识别的 Session source
```

任意一项无法确认：

```text
SourceDeletionUnsupported / Unsafe
```

不猜。

---

# 17. 不限制为标准 Agent 根目录

NoEnding 当前支持用户自定义 ingest source。

因此不能只允许：

```text
~/.codex
~/.claude
~/.pi
```

否则用户从自定义 source 摄入的 Session 永远不能永久删除。

安全边界应该是：

```text
Adapter 能严格验证：
这个文件就是这个 Agent Session
```

而不是：

```text
这个文件恰好在 ~/.codex 下。
```

---

# 18. 不删除 symlink source

如果：

```text
session.raw_path
```

是 symlink：

```text
Permanent source deletion unsupported
```

v0.1 不尝试推断：

```text
删 symlink？
删 target？
```

避免语义不明确。

---

# 19. Prepared Permanent Deletion

复用 PreparedLaunch 的核心思想：

> What you confirm is what gets deleted.

流程：

```text
Session in Trash
      ↓
prepare_session_permanent_delete
      ↓
freeze SourceDeletionPlan
      ↓
store session_deletion_job
      ↓
Preview
      ↓
execute_session_permanent_delete(job_id)
```

Frontend 永远不提交：

```text
path
agent_session_id
targets
```

只提交：

```text
job_id
```

删除范围完全 backend-owned。

---

# 20. Prepare 返回影响预览

例如：

```text
PermanentDeletionPreview {
    job_id,

    session_title,
    agent,

    source_targets,

    event_count,
    binding_count,
    sync_run_count,
    context_delivery_count,
    context_revision_redaction_count,
    launch_intent_count,
}
```

UI 明确展示：

```text
将永久删除：

Agent source
• ~/.codex/.../rollout-x.jsonl

NoEnding
• 1 个 Session
• 2,431 个 Events
• 2 个 Workstream bindings
• 14 个 Sync runs
• ...

保留：
• Workstream
• WorkstreamPath
• WorkspacePath
• Project
• Workstream Context 内容

部分 Context 的“来源”将显示为：
“来源会话已永久删除”
```

---

# 21. Execute 时重新校验 source

Prepare 与 Execute 之间文件可能变化。

所以执行前 Adapter 必须重新检查：

```text
file identity
size
sha256
agent_session_id
```

结果三类：

```text
Exact match
→ 删除

Target already absent
→ 当作 source deletion success

Target exists but fingerprint differs
→ STALE
→ 不删除
→ 要求重新 Prepare
```

尤其禁止：

```text
路径相同
但内容已经是另一个 Session
→ 仍然删除
```

---

# 22. 删除 source 失败

例如：

```text
Windows sharing violation
permission denied
file changed
```

行为：

```text
Session 继续留在 Trash
NoEnding 数据完全保留
job.state = failed / stale
last_error = ...
```

UI：

```text
源会话删除失败
[重试]
[取消永久删除]
```

不能 purge NoEnding 数据。

---

# 23. Crash recovery

可能发生：

```text
job.state = deleting_source
源文件刚删除
App crash
```

下次启动：

```text
不要自动继续删除文件。
```

将遗留：

```text
deleting_source
```

转换为：

```text
failed
last_error = previous deletion was interrupted
```

让用户点：

```text
重试
```

重试时：

如果 source 已经不存在：

```text
execute_source_session_deletion
→ AlreadyAbsent
→ 正常进入 NoEnding purge
```

因此整个过程可恢复。

---

# 24. Source 删除成功后的 NoEnding Purge

只有 Adapter 返回：

```text
Deleted
OR
AlreadyAbsent
```

后才能执行。

整个 NoEnding purge 必须在**单个 SQLite transaction** 中完成。

---

# 25. 当前 schema 的 Session 直接依赖

当前代码中至少存在：

```text
session_events              FK Session
session_cursors             FK Session
session_workstream_bindings FK Session

session_binding_removals    logical ref
sync_runs                   logical ref
launch_intents              matched_session_id
project_affinity_evidence   nullable FK Session
context_deliveries          FK Session
```

以上必须纳入 purge。

不能只：

```sql
DELETE FROM sessions
```

---

# 26. Context provenance 处理

Context 不跟随 Session 删除。

例如：

```text
Session S1
→ Decision: "Use SQLite"
```

永久删除 S1：

```text
Decision 继续存在
```

但 source reference 不能 dangling。

在删除 Events / SyncRuns 前：

```text
找出所有属于该 Session 的：
- SyncRun ids
- SessionEvent refs
```

然后处理相关：

```text
context_item_revisions
```

---

# 27. Context provenance redaction

匹配：

```text
revision.sync_run_id ∈ Session SyncRuns

OR

revision.source_ref
指向该 Session Event

OR

metadata provenance/source_refs
包含该 Session Event ref
```

处理成：

```text
source_type = "deleted_session"
source_ref = NULL
sync_run_id = NULL
```

metadata 中：

```text
删除：
- event ref
- copied raw excerpt
- session id
- source message evidence
```

保留：

```text
authority
actor
status audit
其他不依赖 Session 的 metadata
```

---

# 28. Deleted provenance UI

`get_context_revision_source` 遇到：

```text
source_type = deleted_session
```

返回：

```text
来源会话已被永久删除
```

不要返回：

```text
Session id
agent_session_id
title
raw_path
```

这不是 tombstone。

只是：

```text
这个 Context 原来存在来源，
现在该来源已不存在。
```

---

# 29. NoEnding purge 顺序

建议 transaction 中固定：

```text
1. collect Session Event refs / SyncRun ids

2. redact Context provenance

3. DELETE context_deliveries
   WHERE session_id = ?

4. DELETE project_affinity_evidence
   WHERE session_id = ?

5. DELETE session_workstream_bindings
   WHERE session_id = ?

6. DELETE session_binding_removals
   WHERE session_id = ?

7. DELETE sync_runs
   WHERE session_id = ?

8. DELETE launch_intents
   WHERE matched_session_id = ?

9. DELETE session_cursors
   WHERE session_id = ?

10. DELETE session_events
    WHERE session_id = ?

11. DELETE session_deletion_jobs
    WHERE session_id = ?

12. DELETE sessions
    WHERE id = ?
```

commit 成功后：

```text
unindex("session", session_id)
```

---

# 30. 不删除这些实体

永久删除 Session 不得自动删除：

```text
Workstream
WorkstreamPath
WorkspacePath
Project
Context Item
Context Revision 内容
```

---

# 31. WorkspacePath GC

Session purge 后：

```text
WorkspacePath 的 Session ref count
可能减一。
```

但不要直接：

```text
DELETE WorkspacePath
```

只触发现有 WorkspacePath GC / reconciliation。

只有满足现有 invariant：

```text
0 Session refs
AND
0 WorkstreamPath refs
AND
符合 GC 条件
```

才允许删除。

Project 仍由现有：

```text
last WorkspacePath gone
→ auto delete Project
```

规则处理。

---

# 32. Future Rediscovery

永久删除成功后：

```text
Session row gone
job row gone
Agent source gone
```

如果用户后来从备份恢复源 JSONL：

```text
discovery sees:
(agent, agent_session_id)
```

数据库中不存在 matching Session。

当前 ingestion 本来就：

```rust
Session {
    id: new_id(),
    ...
}
```

所以自然产生：

```text
S2
```

无需任何 tombstone / special case。

---

# 33. Rediscovered S2 的行为

S2：

```text
重新读取 source
重新建立 Events
重新创建 Cursor
重新解析 cwd
重新映射 WorkspacePath
重新派生 Project
```

但：

```text
old bindings          不恢复
old SyncRuns          不恢复
old event ids         不恢复
old Session.id        不恢复
```

它是一轮完全新的 NoEnding ingestion lifecycle。

---

# 34. Context 不能自动重新链接

即使：

```text
S1.agent_session_id
==
S2.agent_session_id
```

原 Context provenance：

```text
deleted_session
```

仍然保持：

```text
deleted_session
```

不能偷偷重新指向 S2。

否则历史会被重写。

---

# 35. Trash UI

Sessions 页面建议：

```text
Sessions                         [回收站]
```

正常列表：

```text
trashed_at IS NULL
```

回收站：

```text
trashed_at IS NOT NULL
```

Row：

```text
Codex
Workspace Domain work
/repo/noending

移入回收站：2 小时前

[恢复]
[永久删除]
```

---

# 36. Session Detail

正常 Session：

```text
危险操作

[移入回收站]
```

注意必须与：

```text
从 Workstream 移除
```

明确区分。

文案：

```text
从 Workstream 移除
= 只修改这个 Workstream 的成员关系。

移入回收站
= 在 NoEnding 中全局隐藏该 Session。
  Agent 原始会话不会被删除。
```

---

# 37. Permanent Delete UI

用户点击：

```text
永久删除
```

先 Prepare。

Modal 展示冻结 plan。

关键警告：

```text
此操作会同时删除 Agent 保存的原始会话。

删除完成后：
• NoEnding 中该 Session 的事件、绑定和摄入历史都会消失。
• Workstream Context 内容不会因此删除。
• 如果你以后从备份恢复原始 Agent 会话，
  NoEnding 会将它作为一个新的 Session 再次收录。
```

按钮：

```text
取消
永久删除
```

不需要要求用户输入文字确认。

一次明确 preview + 二次 destructive action 足够。

---

# 38. Adapter capability UI

如果 Adapter 不支持安全 source deletion：

```text
永久删除暂不可用

NoEnding 无法确认如何安全删除这个 Agent 的源 Session。
你仍可以把它保留在回收站。
```

不要提供：

```text
“仅删除 NoEnding 数据”
```

否则源 Session 下一次 discovery 又会回来，产品语义混乱。

---

# 39. 修改 AGENTS.md invariant

当前：

```text
Raw Agent session files are read-only.
Never modify or delete them.
```

修改为：

```text
Raw Agent session data is read-only during normal operation.

The only exception is an explicit user-confirmed permanent
Session deletion executed through the owning Agent adapter
against a frozen, revalidated SourceDeletionPlan.

Core code must never delete Session.raw_path directly.
```

---

# 40. Non-goal：不是 secure erase

产品不能承诺：

```text
磁盘扇区安全擦除
OS snapshot 清除
Time Machine 清除
云备份删除
SSD secure erase
日志历史全面擦除
```

所以 UI 用：

```text
永久删除
```

不要用：

```text
安全擦除
不可恢复擦除
```

永久删除指：

> 删除 NoEnding 管理的数据和当前 Agent source Session。

---

# 41. Backend API

新增：

```text
trash_session(session_id)

restore_session(session_id)

prepare_session_permanent_delete(session_id)

execute_session_permanent_delete(job_id)

cancel_session_permanent_delete(job_id)

get_session_deletion_job(session_id)
```

Session lists：

```text
list_sessions(..., scope)
```

scope：

```text
active
trash
all
```

---

# 42. Permanent deletion state rules

```text
Normal
→ permanent delete
❌ forbidden

Trash
→ prepare
✅

Trash + prepared
→ restore
❌
先 cancel deletion

Trash + failed
→ restore
✅ only after cancel job

Trash + stale
→ reprepare / cancel

Trash + deleting_source
→ UI disabled until operation returns
```

---

# 43. Concurrency invariant

任何 Session write path：

```text
ingestion
sync
binding auto-classification
launch matching
```

在 commit 前都必须确认：

```text
Session exists
AND
trashed_at IS NULL
```

删除和后台摄入不能并行把 Session 写回来。

---

# 44. Migration v12 → v13

迁移内容只包括：

```text
ADD sessions.trashed_at

CREATE session_deletion_jobs
```

现有 Session：

```text
trashed_at = NULL
```

全部正常。

不修改：

```text
Session ids
Events
WorkspacePath
Project
Workstream
Context
```

属于低风险 additive migration。

---

# 45. 测试矩阵

必须锁以下行为：

```text
trash_session_preserves_source_and_data

restore_keeps_same_session_id

trash_is_hidden_from_default_session_list

trash_is_hidden_from_project_and_workstream_lists

trash_is_removed_from_search

discovery_does_not_restore_trashed_session

discovery_does_not_mutate_trashed_session

inflight_ingest_cannot_commit_after_trash

trashed_session_cannot_resume

prepared_resume_becomes_stale_after_trash


permanent_delete_requires_trash

core_never_deletes_raw_path

adapter_rejects_wrong_agent_source

adapter_rejects_wrong_agent_session_id

adapter_rejects_symlink_source

source_change_after_prepare_makes_plan_stale

source_delete_failure_preserves_noending_session

already_absent_source_can_complete_purge


permanent_delete_removes_events

permanent_delete_removes_cursor

permanent_delete_removes_bindings

permanent_delete_removes_binding_removals

permanent_delete_removes_sync_runs

permanent_delete_removes_context_deliveries

permanent_delete_removes_project_affinity

permanent_delete_removes_matched_launch_intents

permanent_delete_removes_session_search_index

permanent_delete_preserves_workstream

permanent_delete_preserves_workstream_path

permanent_delete_preserves_workspace_path_when_still_referenced

permanent_delete_preserves_context_content

permanent_delete_redacts_context_provenance

successful_delete_leaves_no_deletion_job

successful_delete_leaves_no_session_tombstone


restored_source_is_discovered_again

rediscovered_source_gets_new_noending_session_id

rediscovered_source_does_not_restore_old_bindings

rediscovered_source_does_not_relink_deleted_context_provenance
```

---

# 46. Agent adapter tests

Codex / Claude / Pi 每个至少测试：

```text
prepare correct source

wrong content fingerprint rejected

wrong session id rejected

source changed after prepare → stale

exact plan → file deleted

already absent → success

unrelated sibling file untouched
```

必须用 temp fixture。

不得对本机真实：

```text
~/.codex
~/.claude
~/.pi
```

做删除测试。

---

# 47. Dogfood Gate

实际使用前手工验证：

### Case A

```text
Active Session
→ Trash
→ Session 消失
→ Agent source 文件仍存在
```

### Case B

```text
Trash
→ Restore
→ 同一个 Session.id
→ 原 bindings/events 仍存在
```

### Case C

```text
Trash
→ Permanent Delete
→ Preview 路径正确
→ Agent source 消失
→ NoEnding Session 消失
```

### Case D

```text
source delete permission failure
→ Session 仍在 Trash
→ Events/Bindings 都还在
```

### Case E

```text
Permanent Delete 成功
→ 手工从备份恢复 source file
→ Sync
→ 新 Session.id
→ 重新完整摄入
→ 无旧 Workstream bindings
```

---

# 48. 多 Agent 执行方案

推荐：

```text
Wave 0 — Main
冻结 v13 schema / domain / adapter contracts

Wave 1 并行
A — Session lifecycle + queries
B — Source deletion adapter capability
C — Session purge + provenance redaction

Wave 2 并行
D — ingestion / launcher race protection
E — frontend Trash / Permanent Delete UX

Wave 3
F — integrity / destructive-operation audit

Main
final integration + dogfood + exact-head CI
```

---

# 49. Main — Wave 0

Main 先提交：

```text
refactor(sessions): establish lifecycle deletion contracts
```

负责：

```text
SCHEMA_VERSION 13
sessions.trashed_at
session_deletion_jobs

Domain structs
SourceDeletionPlan
SourceDeletionTarget
SourceDeletionOutcome

AgentAdapter method signatures

Command signatures
TS shapes
shared fixtures
```

Wave 0 必须保持：

```text
cargo check
cargo test
pnpm build
```

全部通过。

---

# 50. Agent A — Session Lifecycle

负责：

```text
trash_session
restore_session

SessionListScope
list filters

Project / Workstream projections
排除 Trash Session

Search unindex/reindex
```

不碰 Agent source deletion。

---

# 51. Agent B — Adapter Source Deletion

负责：

```text
AgentAdapter deletion capability

Codex
Claude Code
Pi

source fingerprint
source identity validation
symlink refusal
stale validation
exact-file deletion
```

只允许 Adapter 层调用 filesystem delete。

---

# 52. Agent C — Permanent Purge + Provenance

负责：

```text
session_deletion_jobs storage

Prepare preview counts

execute coordination

purge_session_data_conn

Context provenance redaction

all dependency cleanup
```

不得直接删除 source file。

只调用 Adapter capability。

---

# 53. Agent D — Ingestion / Launcher Integrity

负责：

```text
Trash ingestion suppression

commit-time trash guard

sync_session trash rejection

Resume rejection

PreparedLaunch stale

LaunchIntent interaction
```

重点查竞态。

---

# 54. Agent E — Frontend

负责：

```text
Sessions → 回收站

Session danger zone

Restore

Permanent deletion preview

failed / stale retry UI

Adapter unsupported state

clear Chinese copy
```

不碰 Rust。

---

# 55. Agent F — Final Integrity Audit

重点做破坏性审计：

```text
有没有任何 Core remove_file(raw_path)

有没有永久 tombstone

有没有 source delete failure 后误 purge

有没有 dangling FK

有没有 dangling Context source_ref

有没有 archived Session 仍被 ingestion 写入

有没有 rediscovery 复用旧 NoEnding Session.id

有没有 permanent deletion 删除 WorkstreamPath / Context 内容
```

有 P0/P1：

```text
直接修 + test
```

---

# 56. 建议 commit 序列

```text
1. docs(sessions): freeze session lifecycle deletion v0.1
2. refactor(sessions): establish lifecycle deletion contracts
3. feat(sessions): add trash and restore lifecycle
4. feat(adapters): add safe source session deletion
5. feat(sessions): add prepared permanent deletion
6. fix(ingestion): suppress trashed session writes
7. feat(ui): add session recycle bin and deletion preview
8. test(sessions): lock deletion integrity invariants
9. fix(sessions): final deletion audit fixes
10. docs(sessions): record v0.1 final state
```

---

# 57. CI Gate

最终：

```text
cargo fmt --check
cargo check
cargo test --all-targets
pnpm build
```

remote：

```text
macOS success
Windows success
head_sha == final HEAD
```

由于源删除涉及文件系统，Windows 尤其必须验证：

```text
file lock failure
already absent
read-only / permission failure
```

---

# 58. 阶段完成条件

全部满足才标记：

```text
Session Lifecycle & Deletion v0.1 — SEALED
```

完成矩阵：

```text
Session Trash                         ✅
Restore same Session identity         ✅
Trash hidden from normal UX           ✅
Trash ingestion suppression           ✅
Trash Resume blocked                  ✅

Prepared permanent deletion           ✅
Adapter-owned source deletion         ✅
Codex support                         ✅
Claude Code support                   ✅
Pi support                            ✅
Source stale detection                ✅
Crash/retry semantics                 ✅

NoEnding Session purge                ✅
Bindings cleanup                      ✅
Events/cursors cleanup                ✅
Sync history cleanup                  ✅
Launch history cleanup                ✅
Context deliveries cleanup            ✅
Context provenance redaction          ✅

No tombstone after success            ✅
No permanent blacklist                ✅
Future rediscovery                    ✅
Rediscovery gets new Session.id       ✅
No old binding resurrection           ✅

macOS                                 ✅
Windows                               ✅
exact-head CI                         ✅
```

---

# 59. 最终产品定义

一句话：

```text
Trash
= “我现在不想让 NoEnding 管理这个 Session。”

Permanent Delete
= “把这次 Session 从 NoEnding 和 Agent 两边都删除。”

Rediscovery
= “如果这个外部 Session 将来真的再次存在，
   NoEnding 把它当成一次新的事实重新收录。”
```

永久删除之后：

```text
没有 tombstone。
没有 blacklist。
没有 S1 → S2 link。
```

只有仍然属于 Workstream 自己的 Context 可以继续存在，并以：

```text
来源会话已永久删除
```

表达其来源已经消失。
