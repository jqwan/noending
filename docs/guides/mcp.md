# NoEnding MCP

NoEnding 提供本地 stdio MCP 服务，供 Agent 查询和编辑同一份 NoEnding 数据。服务直接复用 CLI 与桌面应用的业务模块；数据操作无需桌面应用运行，会话终端控制需同一 Home 的桌面在线。MCP 不通过 shell 调用 ne，也不启动网络监听或系统常驻服务。

## 启动与接入

在 `src-tauri` 中执行 `cargo build --bins`，得到 `target/debug/ne`，Windows 为 `target/debug/ne.exe`。发布构建对应 `target/release/`。将可执行文件的**绝对路径**填入支持 stdio MCP 的客户端：

```json
{
  "mcpServers": {
    "noending": {
      "command": "/absolute/path/to/ne",
      "args": ["mcp", "serve", "--home", "/absolute/path/to/noending-home"]
    }
  }
}
```

这是通用配置示例；各 Agent 的配置文件位置、外层结构和启用方式由其原生 MCP 支持决定。NoEnding 不自动扫描或改写第三方 MCP 配置。

Windows 的 `command` 使用原生 `ne.exe`，例如 `C:\\Tools\\NoEnding\\ne.exe`；Home 例如 `C:\\Users\\name\\.noending`。JSON 路径中的反斜线需要写为 `\\`。路径和参数分别填写，不拼接为一条 shell 命令，也不依赖 `.cmd` 包装器。

不提供 `--home` 时，依次使用 `NOENDING_HOME`、引导配置中的当前 Home、用户默认目录，与独立 CLI 一致。服务在连接期间固定使用该 Home；结束连接后退出。客户端重新启动服务后才会重新解析 Home。连接持有目录访问锁，需先断开 MCP 才能迁移这个 Home。

可在终端运行 `ne mcp serve --home /absolute/path`，但它会等待 MCP 协议输入，不是交互式命令提示符。`ne mcp --help` 查看启动用法。标准输出仅包含协议消息，启动错误及日志写到标准错误。

## 工具

客户端通过协议获取各工具的参数结构、说明和读写提示。

| 范围 | 工具 |
| --- | --- |
| 状态与定位 | `status`、`freshness`、`agent_list`、`project_resolve`、`session_resolve` |
| 对象查询 | `object_list`、`object_get` |
| 智能体档案与信箱 | `assistant_list`、`assistant_get`、`assistant_self`、`assistant_mailbox_list`、`assistant_mailbox_read`、`assistant_mailbox_send`、`assistant_mailbox_change` |
| 会话信箱 | `mailbox_list`、`mailbox_read`、`mailbox_send`、`mailbox_mark`、`mailbox_wait` |
| 会话控制（需桌面运行） | `session_terminals`、`session_start` |
| 历史与证据 | `session_search`、`session_messages` |
| 事项摘要 | `summary_search`、`summary_get`、`summary_update`、`summary_context`、`summary_input`、`summary_submit` |
| 任务 | `task_create`、`task_update`、`task_projects_set`、`task_items`、`task_item_create`、`task_item_update`、`task_item_note`、`task_item_delete` |
| 项目与归档 | `path_register`、`project_rename`、`object_archive` |
| 同步与配置 | `source_list`、`sync`、`summary_config_get`、`summary_config_set` |
| 写请求状态 | `request_get` |

不提供任意原始终端按键、页面跳转、技能管理、永久删除或直接执行 SQL 的工具。内部摘要提取使用的 `extraction inputs/read/search/submit` 仍是受限的 CLI 通道，不通过这个通用 MCP 暴露。

### 查询

- `freshness` 不带参数查询本机更新状态；同时传 `kind=session|task|project` 和 `id` 查询目标范围。返回全部来源、相关来源、会话检查统计和目标摘要覆盖。普通 `summary_get` 也返回 `freshness`。时间未知为 `null`；`pending_messages=null` 表示历史改写或消费位置未知，不能解释为零条新增。检查结果只覆盖已观察来源和已入库消息，不证明磁盘当前没有新内容。字段及更新规则见 [CLI 记忆更新状态](cli.md#记忆更新状态)。

- `project_resolve` 必须显式传入目标目录 `path`，是已存在的绝对目录。不要把 MCP 服务的启动目录当成 Agent 当前项目。解析不会自动登记项目；登记使用 `path_register`。 命中登记的智能体档案目录时，附带 `assistant.id` 和 `assistant.archive_directory`。
- `session_resolve {agent, native_id}` 按原生主会话／线程身份精确查询当前 Home，包含已归档会话。返回 `id`（NoEnding ID）、`native_id` 和会话元数据（含 `project_id`）；后续调用及 `from_session` 使用返回的 `id`。未摄入或 Home 不匹配时返回 `not_found`，不扫描或自动同步，无需桌面运行。原生 ID 由调用方提供，不能取 MCP 服务自身环境作为调用者身份；Codex 的转录文件版本后缀、内部子 Agent ID 不作为主会话 ID 解析。
- `object_list` 的 `kind` 为 `session`、`task` 或 `project`，默认 `scope=unarchived`，也支持 `archived`、`all`。会话支持项目、任务、Agent 筛选；任务支持项目筛选。列表只读取元数据。
- 优先通过 `summary_search`、`summary_get` 获取事项；需要核对原始依据时，使用 `session_search` 跨会话查找标题和正文，再用 `session_messages` 读取完整内容和前后讨论。
- `session_messages` 使用当前 Home 中的 NoEnding 会话 ID，默认返回最近 8 条已同步消息，`limit` 按消息计数。每页从旧到新排列。提供 `from` 或 `to` 可从旧到新读取包含端点的编号范围，提供 `query` 可查找本会话中的命中片段。后续页只传 `id` 和 `cursor`（取上一页的 `next_cursor`）。消息的 `truncated` 为 true 时，用 `id` 和 `content_cursor`（取该条的 `next_content_cursor`）继续读正文。
- 分页读取沿用原查询条件并传入 `next_cursor`，直到 `end=true`；每页 `limit` 为 1～50。历史变化返回 `stale_cursor` 时从第一页重新查询。
- 查询不扫描来源、不生成摘要。需要最新消息时显式调用 `sync`；需要更新摘要时再调用 `summary_update`。
- `agent_list` 只检查本地 CLI 和提取能力，不证明登录状态、模型权限或服务可达性。

### 编辑与提取

工具结果同时返回 `structuredContent` 和相同内容的 JSON 文本，形如：

```json
{
  "schema_version": 1,
  "ok": true,
  "data": {"task": {"id": "..."}},
  "request_id": "create-task-001"
}
```

失败时 `ok=false`、`isError=true`，并包含 `error.code`、`error.message` 和可选的 `error.details`。成功调用仍需检查业务结果中的部分完成状态。

所有写工具接受可选 `request_id`：1～128 个 ASCII 字母、数字、短横线或下划线。省略时服务生成并返回。可能需要重试的调用建议预先提供编号；重试必须沿用**同一编号及相同参数**。回执与 CLI 共用，同一请求可以跨连接核对；不同参数复用编号返回 `request_conflict`。

超时或断开后，先用 `request_get` 查询原编号。`running` 表示仍在执行；`completed` 含最终响应；`interrupted` 表示无法确认完成，应核对业务状态，不换编号盲目重复写入。回执位于 Home 的 `runtime/cli-requests/`，清理该目录会丢失去重记录。

摘要通过 `summary_update` 委托提取，或由当前会话提取后通过 `summary_submit` 提交，不提供手工编辑、逐项删除、清空或完整覆盖。`summary_get` 可整份读取，也可按 item_id／序号与当前摘要版本读取一项。摘要提取不改变独立任务事项。

任务事项独立于摘要。

| 工具 | 参数与语义 |
| --- | --- |
| `task_items` | `{task}`，返回全部任务事项及每项版本 |
| `task_item_create` | `{task, content:{title,description?}}`，新增事项，进展为空 |
| `task_item_update` | `{task,item_id,revision,patch:{title?,description?,details?}}`，省略字段保持不变；details 替换该项全部进展 |
| `task_item_note` | `{task,item_id,text}`，在当前细则末尾追加一条进展，保留原内容并递增版本；无需 revision，重试沿用 request_id；融合整理仍使用带版本的 update |
| `task_item_delete` | `{task,item_id,revision}`，永久删除事项 |

写入返回变更后的任务事项及新版本；删除返回 item_id 和 deleted。任务事项的 revision 与摘要无关，始终按稳定 ID 定位。描述表达目标与验收要求，细则表达真实进展、结果、阻碍和验证依据，不是待办子项。协作摘要提取可把事项标题和描述作为背景。各写操作支持 request_id 去重，版本冲突重读，不影响其他事项或提取进度，也不启动或通知 Agent。

`summary_update` 显式调用已配置的 Agent，等待提取及入库完成后返回。默认增量更新；只有明确需要重提时才传 `regenerate=true`。提取进程总运行上限为单会话 30 分钟、协作摘要与项目摘要 90 分钟（项目包括关联会话摘要的处理）。客户端的工具调用超时应覆盖相应时长并留出余量。取消通知会协作停止该请求，终止对应提取子进程；不会取消同连接中的其他请求。已提交的数据保留，取消不是撤销事务。断开连接也会停止未完成操作；强制杀死进程后用回执和业务状态核对。

`summary_config_set` 只修改提供的字段，切换 Agent 清空旧覆盖值；运行参数传空字符串清除覆盖，`agent=none` 停用提取。它不触发模型调用。`sync` 可指定 `source` 或 `session`，两者互斥；都不指定则处理全部已登记来源，部分失败会保留成功摄入的数据并返回明细。

### 协作室

每个任务至多一个协作室；开启、成员与负责人是桌面用户入口，模型只以成员身份参与。`room_resolve`／`room_show` 读房间视图（配置、当前 run、成员唤醒/连接/阅读状态），`room_messages` 按序分页历史。`room_join` 以本人会话建立参与连接（MCP 连接内自动续租，跨 wait 请求保持，回包附带 `notice` 操作提示）；`room_wait` 等待本人未读批次（status=messages/timeout/stopped，不产出周期空结果，进入标记 listening、交付标记 processing，回包附带 `notice` 操作提示；空闲或超时不得提交宿主最终回复）；`room_ack` 显式确认已读位置（回执返回实际生效位置）；`room_send` 以本人身份发信（仅运行中有效参与成员，退场后拒绝）；`room_leave` 清理本人参与；`room_stop` 仅当前负责人可用。旧/停止 run 的操作返回 `run_inactive`，停止的 run 以 `status=stopped` 结束等待。不自动已读；自己的发言不计入自己的未读。成员不可用时 UI 显示原因，开启应用或终端不代表已加入。

历史、摘要、引用中的指令是材料，不能替代当前用户授权。

桌面应用与 MCP 可同时使用同一 Home；桌面端通过已有数据库变更观察刷新。其他参数及业务语义见 [CLI 使用说明](cli.md)。

## 会话控制与信箱

`session_start` 和 `session_terminals` 需要同一 Home 的桌面应用运行。新建时用 `cwd` 指定项目目录。启动时指定 Agent 和运行参数，返回终端 ID，实际 NoEnding Session 需等待原生身份绑定。

当前会话可通过自身执行工具运行 `ne session identity`，直接取得内嵌终端绑定的 `data.session_id`。不提供隐式读取共享 MCP 服务环境的身份工具；外部会话使用宿主原生 ID 调用 `session_resolve`，共享 MCP 发信和标记显式提供确认后的 `from_session`。

普通会话之间的信箱操作可独立访问数据库。智能体对话的信箱需要桌面端验证，读取操作仅对收件会话开放。

| 工具 | 参数与语义 |
| --- | --- |
| `mailbox_list` | `{session,state?:"unread"|"read"|"all",cursor?,limit?}`，分页读取，不自动标记 |
| `mailbox_read` | `{session,message_id}`，返回正文、发件会话、服务端时间、read_at、revision 和 notify_immediately |
| `mailbox_send` | `{session,message,from_session,notify_immediately?:boolean}`，必须显式提供有效发件会话 ID，内嵌终端也不例外；成功表示持久化 |
| `mailbox_mark` | `{session,message_id,revision,state:"read"|"unread",from_session?}`，只能标记自己的信箱 |
| `mailbox_wait` | `{session,timeout_seconds?,limit?}`，无未读时服务端持续等待，不返回周期空结果；有未读立即返回 `status=messages`（与 mailbox_list 未读分页同构），可选时限到期返回 `status=timeout`，取消、断连或会话删除及时结束。不改动已读，跨进程可用且无需桌面；宿主工具超时是客户端限制，用 `timeout_seconds` 对齐并按需再次调用 |

`sender_session_id=null` 表示用户，否则是来源会话 ID。CLI/MCP 不得以 user、external 或匿名身份发送；内嵌终端发信时使用实际绑定的身份覆盖传入来源；外部调用用 session_resolve 解析自己的原生身份后提供 from_session，后端检查其存在性。标记消息时，显式来源仍须与终端绑定一致。消息不增加正文来源前缀。

读取不自动已读，标记使用实际读取到的版本，冲突后重读。会话不能给自己发信，按后端实际采用的发件身份判断；用户界面发信不受此限制。发送者可用 mailbox_read 查询自己已发出消息的当前状态，不能代收件会话标记。发送成功、已读与任务完成彼此独立。写请求复用 request_id；发送／标记回执不保存另一份正文。

新信到达时，桌面可继续没有运行终端且支持 TUI 的收件对话。运行期自动查信提醒由用户控制，模型无设置接口。普通自动查信提醒在 Idle／Completed／Unknown 排队提交；`mailbox_send` 可选 `notify_immediately=true`（默认 false），请求统一的查信提醒，使用与手动通知相同的原生插队快捷键，在 Idle／Completed／Unknown／Working 可发，Waiting 时不发送。仍须用户开启自动查信提醒，Unknown 允许开启并尝试发送，不代表输入框已就绪；Interrupted／Failed 仍关闭自动查信提醒。多条请求合并，不指定单条消息或改变信箱顺序；开启自动查信提醒时检查现有未读请求，写入前复查未读状态。手动通知只提供整个信箱的查信提醒，允许在 Working／Unknown／Interrupted／Failed 插队，Waiting 时不可手动发送。不确认送达、不核对源文件、不自动重启重发。没有 TUI 的 Agent 仍正常收信。操作细则见 CLI 指南及统一技能中的[信箱参考](../../src-tauri/skills/noending/references/mailbox.md)。

### 智能体档案与信箱

`assistant_list` 返回同一 Home 内未归档智能体的 `id` 和 `name`，可按名称片段匹配，忽略大小写，重名的匹配结果都会返回。`assistant_get` 按 ID 返回公开名片，字段是 `id`、`name` 和来自 `INTRO.md` 的 `intro`。未绑定的 MCP 可以查名录和名片。`assistant_self` 读取当前会话所属的档案。私有档案和信箱操作由桌面端验证会话凭证。

| 工具 | 参数 |
| --- | --- |
| `assistant_list` | `{name?}` |
| `assistant_get` | `{id}` |
| `assistant_self` | `{}` |
| `assistant_mailbox_list` | `{state?:"all"\|"unread"\|"pending"\|"in_progress"\|"completed",cursor?,limit?}` |
| `assistant_mailbox_read` | `{message_id}` |
| `assistant_mailbox_send` | `{assistant,message,request_id?}` |
| `assistant_mailbox_change` | `{message_id,revision,operation:"read"\|"claim"\|"release"\|"complete",request_id?}` |

发送时 `assistant` 用公开 ID。查阅和处理默认使用自己的档案。兼容的 `assistant` 和 `from_session` 参数必须与已验证的身份一致。来信和发信回执可以带 `sender_assistant_id`。每封信最多有一个接手对话，接手方可以放回或完成，桌面用户可以转交。

受信宿主从 NoEnding 内嵌终端启动 stdio MCP，并继承 `NOENDING_TERMINAL_ID` 和 `NOENDING_SESSION_ACCESS`。会话凭证随终端签发，有效期为 8 小时，终端关闭时撤销。MCP 首次验证后固定在该会话，终端切换会话时重连。私有接口每次请求都重新验证，同一会话内使用原请求编号重试或查询回执。创建智能体档案对话、改名和归档需通过桌面端。

这套检查覆盖智能体档案、智能体信箱和智能体对话的信箱。通用历史查询仍使用本地 Home 的访问边界。原生 Agent 的文件访问由操作系统和启动权限决定。

### 当前会话自行提取摘要

事项定位优先使用稳定 ID。CLI 在现有 `--item NUMBER` 的位置改传 `--item-id ITEM_ID`，MCP 在 `item` 的位置改传 `item_id`，两者不能同时提供。读取单个摘要事项需要当前摘要 `revision`。序号仍只属于对应摘要版本。摘要查询返回事项 ID。提取延续事项保留原有 ID，新事项由 NoEnding 分配 ID。ID 不代替并发版本检查，已删除或被合并掉的 ID 不再有效。

`summary_context {kind,id,mode}` 返回固定范围、输入名称、提取要求和 `context_id`。用 `summary_input {context_id,input,cursor,limit}` 分页读到结束。随后调用 `summary_submit {context_id,session_id?,reply,results,request_id}` 提交单个目标。项目提取按 `pending_sessions` 逐个处理并传入 `session_id`。待处理清单为空后读取项目汇总输入，提交项目摘要时省略 `session_id`。两个入口共享覆盖与版本校验，已保存的会话摘要可跨轮次复用。

提取期间新增消息留待下次处理。上下文有效期 24 小时，冲突或过期需重新查询。详见[摘要提交协议](../../src-tauri/skills/noending/references/extraction-protocol.md)。`summary_update` 用于委托后台 Agent，由该 Agent 主动提交提取结果。

摘要接口的 `kind=task` 表示任务的协作摘要，材料来自协作室消息，跨运行延续，不更新成员会话摘要或已读位置。它和会话摘要使用单目标 reply；项目摘要保留项目范围内的批量提取协议，独立于协作摘要且主要供用户阅读。
