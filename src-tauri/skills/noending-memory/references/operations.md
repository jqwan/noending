# NoEnding 记忆操作

使用已确认的同一 Home。访问入口提供 `cli` 时，下列示例的 `ne` 均替换为该绝对路径；该入口已绑定 Home，不追加 `--home`。Windows 使用 `.cmd` 的原生脚本调用方式。下列 CLI 示例中的 ID、PATH、TEXT、REVISION、FILE、CURSOR 是待替换参数；含空格的路径或正文按当前 shell 正确传参，避免把检索内容拼成可执行代码。已有 MCP 时可直接使用表中的工具，不需要再启动 `ne mcp serve`。

## 定位与范围查询

| 目的 | CLI | MCP |
| --- | --- | --- |
| 确认 Home 与能力 | `ne status` | `status` |
| 定位项目 | `ne project resolve --path PATH` | `project_resolve`，`path` 为实际项目绝对目录 |
| 原生身份转 NoEnding 身份 | `ne session resolve --agent AGENT --native-id NATIVE_ID` | `session_resolve {agent, native_id}`，使用调用方已知的原生主会话 ID |
| 查询单个对象 | `ne session/task/project show ID`，三者选一 | `object_get`，`kind` 和 `id` |
| 项目会话 | `ne session list --project ID --scope all` | `object_list`，`kind=session`、`project`、`scope=all` |
| 项目关联任务 | `ne task list --project ID --scope all` | `object_list`，`kind=task`、`project`、`scope=all` |
| 任务关联会话 | `ne session list --task ID --scope all` | `object_list`，`kind=session`、`task`、`scope=all` |
| 已登记来源 | `ne source list` | `source_list` |
| 目标更新状态 | `ne freshness --session ID`，或 `--task ID` / `--project ID` | `freshness`，同时提供 `kind` 和 `id` |
| 本机更新状态 | `ne freshness` | `freshness`，不提供 `kind` 和 `id` |

会话对象的 `task_ids` 是其跟踪任务。`session resolve` / `session_resolve` 返回 `id`（NoEnding ID）、`native_id` 及会话元数据，包含已归档记录；未命中返回 `not_found`，不自动同步。原生身份须来自调用方，不使用 MCP 服务自身的环境；不把 Codex 文件版本后缀或子 Agent ID 作为主会话身份。项目定位失败返回 `found=false`，不需要为检索而调用 `path register`。

列表、搜索使用 `--cursor CURSOR` / MCP `cursor` 延续 `next_cursor`，保持其他参数一致。列表默认每页 20 项、最多 50 项；只有需要覆盖整个集合时才遍历全部页，不从第一页推断全量数量或完整结论。

## 摘要与证据

| 目的 | CLI | MCP |
| --- | --- | --- |
| 完整摘要与覆盖状态 | `ne session/task/project summary ID`，三者选一 | `summary_get`，`kind` 和 `id` |
| 摘要事项搜索 | `ne summary search --query TEXT --project ID --scope all`；本机范围省略 `--project` | `summary_search`，`query`、可选 `project`、`scope=all` |
| 按事项读取 | `ne session/task/project summary ID --item-id ITEM_ID --revision REVISION` | `summary_get`，同时提供 `item_id` 和 `revision` |
| 项目／任务历史搜索 | `ne search --query TEXT --project ID --scope all`，任务范围改为 `--task ID` | `session_search`，`query`、`project` 或 `task`、`scope=all` |
| 当前已确认会话内搜索 | `ne session search ID --query TEXT` | `session_messages`，`id` 和 `query` |
| 阅读完整消息及上下文 | `ne session messages ID --from ORDINAL --to ORDINAL --limit 8` | `session_messages`，`id`、`from`、`to`、`limit` |

`summary_search` 是事项的字面检索，不是语义检索。项目筛选也可能返回关联任务的摘要，该任务可能覆盖其他项目；核实命中的 `kind` 与归属。严格只查项目会话事项时指定 `kind=session`。任务摘要搜索没有 `task` 参数：先读目标任务摘要，按需列出关联会话并读取其摘要；不要把同项目的全部会话当成任务会话。当前会话摘要直接按其 ID 读取，避免全库搜索后猜测身份。

`session_search` 返回定位信息，完整证据用 `session_messages` 阅读。消息 `sequence` / `from` / `to` 指会话当前投影中从 1 开始的 ordinal；多段 `part` 属于同一条消息。`message_id` 是消息标识，不能拿来充当 ordinal。搜索使用不同匹配机制，无匹配时可换用原话、别称或更短关键词；不要因一次关键词未命中就否定事实。

通用对象查询没有 `source_path`、`source_kind`、原始文件读取入口。源文件层需要当前调用环境或用户提供可核实的路径和来源信息，并使用可用的文件读取能力。JSONL、压缩文件和来源数据库不能混用读取方法；不识别格式时说明限制，不修改原文件，不打开 NoEnding SQLite 寻找路径。MCP 服务报告的 Home 也不能直接当作 Agent 源会话目录。

## 同步与提取

- `ne sync --session ID` / MCP `sync {session: ID}`：只检查已知会话。
- `ne sync --source ID` / MCP `sync {source: ID}`：扫描一个已登记来源，可发现新会话。
- `ne sync` / MCP `sync {}`：扫描全部已登记来源。项目或任务没有独立的同步参数；按需选择来源，本机全部范围才采用全部来源同步。
- `ne summary update session|task|project ID` / MCP `summary_update {kind, id}`：调用已配置 Agent，增量整理并等待入库。它不自动同步源文件。
- 明确需要重新提取时，使用 `ne summary regenerate session|task|project ID` / MCP `summary_update {kind, id, regenerate: true}`。重新生成仍保留锁定标题。

更新项目／任务摘要会处理其范围内需要更新的会话；成功刷新子会话后，其他关联任务／项目仍可能待更新，不要为消除这些状态而顺带刷新无关对象。`refreshed_at` 也可能来自部分完成或保留摘要的跳过结果，应结合 `pending`、失败会话和覆盖状态判断。默认来源目录缺失不等于历史数据丢失；不为了消除 `missing` 状态安装 Agent 或创建空目录。

## 编辑和结果确认

写入格式为完整事项列表：

```json
{"items":[{"id":"已有事项的 ID（新增时省略）","title":"事项标题","description":"当前确认的结论","details":["必要细则"],"title_locked":false}]}
```

此示例仅说明格式，不能作为覆盖旧摘要的默认内容。保留已有锁定事项的标题及 `title_locked=true`；普通 `summary set` / `summary_set` 不能切换标题锁。CLI/MCP 要求非空事项列表，不能通过写入空列表清空摘要。

- CLI：将完整 UTF-8 JSON 写入独立临时 FILE，执行 `ne summary set session|task|project ID --file FILE --revision REVISION`，作用域三选一。使用后删除自己的临时文件。
- MCP：`summary_set` 传入 `kind`、`id`、`revision` 和 `content` 对象。
- `revision` 不一致时重新读取、融合，不能强行覆盖。成功后重新读取目标摘要核实结果；手动写入不会更新 `refreshed_at` 或处理位置。
- CLI 看退出码和 JSON `ok`、`data/error`；MCP 看 `isError` 及同一结果对象。同步／提取还需查看部分完成、失败或剩余待处理状态。
- 需要重试的写操作预先提供 `--request-id ID` / MCP `request_id`；相同请求重试沿用同一编号和相同参数。超时后用 `ne request show ID` / MCP `request_get {id: ID}` 查询回执，结合目标状态核对，不换编号盲目重复执行。

任务待办的发布、完成与废弃由 `noending-task` 指导：CLI 为 `ne task todo publish/complete/discard`，MCP 为 `task_todo_publish` / `task_todo_complete` / `task_todo_discard`，发布只需标题和可选描述，新事项细则为空，已有进展细则保持不变；完成默认保留内容并解锁，可选回写已核实结果；废弃直接删除指定事项，包含最后一项，不保留标记。后端事务执行这些动作，不新增事项状态。不要再拆成保存摘要和切换锁两步。单纯回忆、历史提取或普通摘要修正不主动发布、完成或废弃待办，也不把所有锁定事项都当作待办。独立的任务标题保护仍可通过 `summary lock/unlock task` 或 `task_summary_title_lock` 显式调整，不代表完成状态；会话、项目不开放这些写接口。

没有提取 Agent 时，可按已有材料回答或在用户要求下直接编辑摘要；不要自动安装 Agent、切换模型或改写提取配置。终端启动和消息发送由协作 skill 指导，要求桌面在线；记忆 skill 不依赖这些能力。通用 MCP 不提供永久删除工具。

事项定位优先使用稳定 ID：CLI 使用 `--item-id ITEM_ID`，MCP 使用 `item_id`，同时提供当前 `revision`。也支持 `--item NUMBER` / `item` 序号，但只能用于对应摘要版本，且不能与 ID 同传。全量编辑须保留延续事项的 `id`，新增事项省略 `id`，由 NoEnding 生成。ID 不代替并发版本检查；事项被删除或合并后，原 ID 可能不再存在，不猜测替代项。
