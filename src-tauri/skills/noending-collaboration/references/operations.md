# NoEnding 协作操作

使用同一个已确认 Home。以下 `ne` 替换为访问入口提供的绝对 `cli` 路径；内置入口已绑定 Home，不追加 `--home`。ID、PATH、TEXT、FILE、REVISION、CURSOR 是待替换参数，按当前 shell 正确传参，不将用户文字拼成可执行代码。

## 定位与关系

| 目的 | CLI | MCP |
| --- | --- | --- |
| 核对数据目录 | `ne status` | `status` |
| 定位项目 | `ne project resolve --path PATH` | `project_resolve {path}`，使用调用方实际绝对目录 |
| 查询对象详情 | `ne task/session/project show ID`，三者选一 | `object_get {kind, id}` |
| 项目任务 | `ne task list --project ID --scope all` | `object_list {kind: "task", project, scope: "all"}` |
| 任务关联会话 | `ne session list --task ID --scope all` | `object_list {kind: "session", task, scope: "all"}` |
| 项目候选会话 | `ne session list --project ID --scope all` | `object_list {kind: "session", project, scope: "all"}` |
| 任务关联项目 | `ne task project list ID` | `object_get {kind: "task", id}`，读取关联项目 |

会话 `task_ids` 表示跟踪的任务。列表默认每页 20 项、最多 50 项；按 `next_cursor` 继续时保持筛选条件，CLI 用 `--cursor`、MCP 用 `cursor`。不要把第一页当作完整集合。`scope=all` 包含已归档对象；新增跟踪需另行核实任务未归档。

## 创建、调整与跟踪

| 目的 | CLI | MCP |
| --- | --- | --- |
| 创建任务 | `ne task create --title TEXT --description TEXT --project ID`，`--project` 可重复 | `task_create {title, description, projects: [ID]}` |
| 编辑标题／描述 | `ne task update ID --title TEXT --description TEXT`，仅传需修改字段 | `task_update {id, title, description}`，仅传需修改字段 |
| 替换项目关联 | `ne task project set ID --project ID`，`--project` 可重复 | `task_projects_set {id, projects: [ID]}` |
| 添加会话跟踪 | `ne task attach TASK_ID --session SESSION_ID` | `task_session_set {task, session, tracked: true}` |
| 取消会话跟踪 | `ne task detach TASK_ID --session SESSION_ID` | `task_session_set {task, session, tracked: false}` |
| 归档／恢复任务 | `ne task archive ID` / `ne task unarchive ID` | `object_archive {kind: "task", id, archived: true/false}` |

创建可以不关联项目，但新增会话跟踪要求任务关联会话当前项目。跨项目任务用多项目关联承载，无需为每个项目复制任务。任务标题不能为空，描述传空字符串可清空；未提供字段保持原值。

`task project set` / `task_projects_set` 替换完整项目列表，空列表解除全部项目关联；不会移动会话，也不会清除已有跟踪。`attach` 增加指定关系，保留其他跟踪；`detach` 只移除指定关系。这些命令不向 Agent 发送文字、不触发执行。归档任务不会自动归档关联会话。

## 进展与摘要

| 目的 | CLI | MCP |
| --- | --- | --- |
| 读取任务摘要 | `ne task summary ID` | `summary_get {kind: "task", id}` |
| 读取关联会话摘要 | `ne session summary ID` | `summary_get {kind: "session", id}` |
| 检索任务内历史 | `ne search --query TEXT --task ID --scope all` | `session_search {query, task, scope: "all"}` |
| 完整消息与前后文 | `ne session messages ID --from ORDINAL --to ORDINAL --limit 8` | `session_messages {id, from, to, limit}` |
| 目标覆盖状态 | `ne freshness --task ID` | `freshness {kind: "task", id}` |
| 检查单个已知会话 | `ne sync --session ID` | `sync {session: ID}` |
| 扫描来源发现新会话 | `ne source list`，再 `ne sync --source ID` | `source_list`，再 `sync {source: ID}` |
| 增量整理任务摘要 | `ne summary update task ID` | `summary_update {kind: "task", id}` |

消息范围使用从 1 开始的 ordinal，不是 `message_id`；长消息按 `part` 顺序完整拼接。搜索片段只用于定位，作为结论依据时读取完整消息。摘要的 `pending_messages` 只统计已入库且未覆盖的消息，`null` 表示未知；零不证明源文件最新。`refreshed_at` 和覆盖状态用于判断提取，不能用可能来自手动编辑的 `updated_at` 代替。

同步和提取分别执行；项目／任务没有独立同步参数。任务摘要提取会处理关联会话中需要更新的材料，并等待结果入库，但可能部分失败；不因此刷新无关对象或更改提取配置。

手动同步进展时，先读完整任务摘要及 `revision`，在完整事项列表中融合已有结论：

```json
{"items":[{"title":"事项标题","description":"当前确认的进展","details":["必要依据或后续"],"title_locked":false}]}
```

CLI 用 `ne summary set task ID --file FILE --revision REVISION`，FILE 是独立 UTF-8 JSON 临时文件，用后删除；MCP 用 `summary_set {kind: "task", id, revision, content: {items: [...]}}`。写入替换完整事项列表，示例不能直接覆盖旧内容。保留仍有效事项及锁定标题、`title_locked=true`；普通 `summary set` / `summary_set` 不能切换标题锁。CLI/MCP 不接受空事项列表。版本冲突时重读并融合，不强行覆盖。

尚未入库的当前讨论可在目标明确时直接整理写入。手动编辑不会推进提取消费位置，不得声称已吸收全部新增消息或已完成模型提取。

## 待办标题锁（仅任务摘要）

| 目的 | CLI | MCP |
| --- | --- | --- |
| 锁定任务事项标题 | `ne summary lock task ID --item NUMBER --revision REVISION` | `task_summary_title_lock {id, item: NUMBER, revision: REVISION, locked: true}` |
| 解锁任务事项标题 | `ne summary unlock task ID --item NUMBER --revision REVISION` | `task_summary_title_lock {id, item: NUMBER, revision: REVISION, locked: false}` |

锁接口不接收正文，不改描述、细则或提取消费位置，返回新的 `revision` 与完整 `summary`。`item` 从 1 开始，与该版本的事项列表对应。CLI 不接受会话或项目作为加锁／解锁目标；MCP 的 `id` 仅为任务 ID，不提供 `kind` 参数。

发布待办：读取完整摘要 → 用 `summary set` 保存待办（新事项的 `title_locked=false`，保留已有锁）→ 使用返回的新版本核实事项序号并加锁 → 读取确认 `title_locked=true`。先前已有锁的同一待办无需再次切换。

完成待办：保留锁并写回已核实的完成结果 → 使用写回后的新版本解锁对应标题 → 读取确认 `title_locked=false`。不能将“更新摘要成功”“没有待处理消息”或任务已关联会话当作完成证据。若仍未完成则只更新进展并保留锁；仅从历史提取摘要时不进行这套锁切换。

每次加锁／解锁会推进摘要版本；多个事项使用每一步返回的新版本。冲突时重新读取和定位，不能复用旧序号。给保存、每个锁切换分别使用独立请求编号；同一操作重试保持编号和参数。部分失败时报告“已写入，但尚未锁定”或“已记录完成，但尚未解锁”，再按回执和实际状态处理。

## 结果与重试

CLI 检查退出码和 JSON `ok`、`data/error`；MCP 检查 `isError` 及同一结果对象。同步／提取同时检查失败和待处理信息。成功后重读对象或摘要核实结果。

可能重试的写操作预先提供 `--request-id ID` / MCP `request_id`，同一请求重试沿用相同编号和参数。超时后先用 `ne request show ID` / `request_get {id: ID}` 查询回执，并核对目标；不换编号盲目重建任务。

当前没有会话启动、继续和消息发送接口，也没有通过写入任务或摘要来投递指令的机制。不要编造相关命令或把底层 Tauri 内部接口当作通用 CLI/MCP 能力。
