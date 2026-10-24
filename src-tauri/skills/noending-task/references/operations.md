# NoEnding 任务操作

使用同一个已确认 Home。以下 `ne` 替换为访问入口提供的绝对 `cli` 路径；内置入口已绑定 Home，不追加 `--home`。ID、PATH、TEXT、FILE、REVISION、CURSOR 是待替换参数，按当前 shell 正确传参，不将用户文字拼成可执行代码。

## 定位与关系

| 目的 | CLI | MCP |
| --- | --- | --- |
| 核对数据目录 | `ne status` | `status` |
| 定位项目 | `ne project resolve --path PATH` | `project_resolve {path}`，使用调用方实际绝对目录 |
| 原生身份转 NoEnding 身份 | `ne session resolve --agent AGENT --native-id NATIVE_ID` | `session_resolve {agent, native_id}`，使用调用方已知的原生主会话 ID |
| 查询对象详情 | `ne task/session/project show ID`，三者选一 | `object_get {kind, id}` |
| 项目任务 | `ne task list --project ID --scope all` | `object_list {kind: "task", project, scope: "all"}` |
| 任务关联会话 | `ne session list --task ID --scope all` | `object_list {kind: "session", task, scope: "all"}` |
| 项目候选会话 | `ne session list --project ID --scope all` | `object_list {kind: "session", project, scope: "all"}` |
| 任务关联项目 | `ne task project list ID` | `object_get {kind: "task", id}`，读取关联项目 |

`session resolve` / `session_resolve` 返回 `id`（NoEnding ID）及原生身份、项目和任务元数据，包含已归档记录；后续操作使用该 `id`。未命中返回 `not_found`，不扫描或自动同步；核对 Home，必要时显式同步相应来源。查询身份不要求桌面在线。

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
{"items":[{"id":"已有事项的 ID（新增时省略）","title":"事项标题","description":"当前确认的进展","details":["必要依据或后续"],"title_locked":false}]}
```

CLI 用 `ne summary set task ID --file FILE --revision REVISION`，FILE 是独立 UTF-8 JSON 临时文件，用后删除；MCP 用 `summary_set {kind: "task", id, revision, content: {items: [...]}}`。写入替换完整事项列表，示例不能直接覆盖旧内容。保留仍有效事项及锁定标题、`title_locked=true`；普通 `summary set` / `summary_set` 不能切换标题锁。CLI/MCP 不接受空事项列表。版本冲突时重读并融合，不强行覆盖。

尚未入库的当前讨论可在目标明确时直接整理写入。手动编辑不会推进提取消费位置，不得声称已吸收全部新增消息或已完成模型提取。

## 发布、完成与废弃任务待办

| 目的 | CLI | MCP |
| --- | --- | --- |
| 发布新待办 | `ne task todo publish TASK_ID --file FILE --revision N` | `task_todo_publish {id, revision, content: {title, description?}}` |
| 再次发布已有事项 | 发布命令增加 `--item-id ITEM_ID` | 发布工具增加 `item_id: ITEM_ID` |
| 完成待办 | `ne task todo complete TASK_ID --item-id ITEM_ID --revision N`，可选 `--file FILE` | `task_todo_complete {id, item_id, revision, result?: {description, details}}` |
| 废弃待办 | `ne task todo discard TASK_ID --item-id ITEM_ID --revision N` | `task_todo_discard {id, item_id, revision}` |

发布 FILE 最简为 `{ "title": "修复会话同步" }`，可选增加 `description` 概括目标与必要边界；不传 `details` 或锁字段。新事项描述省略时为空，细则由后端初始化为空数组。发布省略 `item_id` 和 `item` 时追加；指定时修改该事项标题和已提供的描述并锁定，保留已有细则，未提供描述也保留原描述；显式传空描述可清空描述，已锁定标题不能改名。

`details` 记录事项本身已确认的进展、结果、阻碍和验证情况，不是待办子项、执行步骤或勾选清单。推进时通过普通摘要编辑更新细则，例如“已复现问题并定位原因”“修复完成，Windows 验证仍受阻”；保留其他事项和锁，按当前有效进展综合整理，不机械追加流水账，不虚构执行情况，也不按细则条数判断完成。

完成默认只解锁标题，保留原有标题、描述与进展细则。可选提供完成 FILE，例如 `{ "description": "同步异常已修复并验证", "details": ["回归验证通过，已有消息保持完整"] }`；提供时完整替换描述与细则，至少一处非空，需保留仍有效的进展和必要背景。FILE 为 UTF-8 JSON。

废弃直接删除指定事项，包括标题、描述与进展细则，不保留废弃标记或占位。可删除最后一项并清空任务摘要。只在用户明确放弃该待办时调用，不能把受阻、暂缓或未完成自动当成废弃。

完成／废弃只接受已锁定事项，均为处理动作，不新增事项状态。各接口保留其他事项和提取消费位置，不自动匹配相似标题，不启动、通知 Agent，也不归档整个任务。每次操作使用当前版本和稳定事项 ID，在同一事务内完成；成功返回 `item_id`、当前序号 `item`、新 `revision` 与完整 `summary`。序号从 1 开始，只在对应版本有效；冲突时重读并定位，多项操作逐次采用新版本。无须再单独解锁或删除。

写操作可传独立 `--request-id` / `request_id`，同一操作重试保持编号和所有参数，避免重复发布或删除序号移位后的其他事项。已完成或已废弃后，不能用新编号重放旧发布来核对状态。

标题锁不是完成状态。普通历史提取保留现有锁；不要批量完成或废弃锁定的重要结论，也不要把普通未锁定事项视为已完成。明确要求单独调整标题保护时，仍可用 `ne summary lock/unlock task ID --item-id ITEM_ID --revision N` 或 `task_summary_title_lock {id, item_id, revision, locked}`；它只改标题锁，不修改正文、不代表完成或废弃待办，不开放会话／项目锁。

事项定位优先使用稳定 ID：CLI 使用 `--item-id ITEM_ID`，MCP 使用 `item_id`，同时提供当前 `revision`。也支持 `--item NUMBER` / `item` 序号，但只能用于对应摘要版本，且不能与 ID 同传。全量编辑须保留延续事项的 `id`，新增事项省略 `id`，由 NoEnding 生成。ID 不代替并发版本检查；事项被删除或合并后，原 ID 可能不再存在，不猜测替代项。
