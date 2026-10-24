# NoEnding CLI

数据命令独立打开 NoEnding 数据库，复用桌面应用的业务模块，无需启动桌面应用；会话终端控制需同一 Home 的桌面应用正在运行。命令等待操作完成后返回并退出；不启动常驻服务，也不自动扫描会话、技能或项目。`ne` 是短命令，`noending` 保留完整名称，子命令不使用缩写。输出为 JSON，日志不写入标准输出。

Agent 也可以使用 `ne mcp serve [--home PATH]` 启动 stdio MCP 服务，直接调用同一业务能力。该服务保持连接期间运行，标准输出专供 MCP 协议；配置及工具见 [MCP 使用说明](mcp.md)。

## 安装

源码运行时先在 `src-tauri` 执行 `cargo build --bins`，得到桌面程序 `target/debug/noending` 和控制台程序 `target/debug/ne`（Windows 加 `.exe`）。桌面应用照常使用 `pnpm tauri dev` 启动。

可以直接运行控制台程序，也可以安装两个命令到已有的 PATH 目录：

```sh
./target/debug/ne cli install --dir ~/.local/bin
ne cli status --dir ~/.local/bin
ne cli update --dir ~/.local/bin
ne cli uninstall --dir ~/.local/bin
```

安装包可使用应用的可执行文件运行同一安装命令，例如 macOS：

```sh
/Applications/NoEnding.app/Contents/MacOS/noending cli install --dir ~/.local/bin
```

Windows 使用 `ne.exe` 或 `noending.exe cli install --dir <目录>`，生成 `ne.cmd` 和 `noending.cmd`。将所选目录加入 PATH；这些命令不修改 shell 配置。install 不覆盖已有文件，update 和 uninstall 只操作有 NoEnding 管理标记的普通文件，拒绝覆盖其他命令或符号链接。status 检查两个入口是否存在、是否受管理、是否指向当前程序。移动应用后，用新位置的程序执行 cli update 修正入口。

## 内置技能

桌面应用启动时在技能库分别提供 `noending-memory`、`noending-task` 和 `noending-mailbox`，默认不为任何 Agent 或项目启用。在项目详情的「配置技能」或技能详情中启用，也可配置用户级启用；之后新建或重新启动对应 Agent 会话以加载技能。

记忆技能负责按会话、项目、任务或本机范围回忆与维护上下文；协作技能负责创建与调整任务、项目关联、已有会话的任务跟踪及进展汇总。三者可独立启停、卸载；任务摘要仍属于记忆范围。协作接口支持通过运行中的桌面应用新建／继续终端；信箱技能负责查收、标记与发送消息，数据操作无需桌面运行。

内置技能的 `references/access.json` 提供固定到当前 Home 的 CLI 绝对路径，入口随技能分发，不修改全局 PATH，也不自动配置 MCP。该入口已带 `--home`，调用时不重复追加；独立安装的普通 `ne` 命令仍可按本文方式选择 Home。已有 MCP 连接应核对 `status.home` 与技能 Home 一致。

应用更新只维护内置定义、操作参考及访问入口，保留其他支持文件；项目／用户级停用和删除源文件的卸载不会在重启时被恢复。存储位置变更后保留现有技能源和加载链接，更新它们访问的 Home。同名、未经应用管理的目录会保留，不自动覆盖。

通过 NoEnding 新建或继续的终端 Agent 会话，默认使用各自的完全访问模式：Codex 使用 `--dangerously-bypass-approvals-and-sandbox`；Claude Code 使用 `--dangerously-skip-permissions`，并在支持沙箱的非 Windows 平台通过本次启动的 `--settings` 关闭沙箱；Antigravity 使用 `--dangerously-skip-permissions --sandbox=false`。Pi 原生按当前系统账户权限执行工具，无需额外权限参数。这些启动参数不依赖助手项目或记忆技能是否启用，不修改 Agent 的持久配置，也不改变后台摘要提取的权限策略。

参数依据：[Codex CLI](https://developers.openai.com/codex/cli/reference)、[Claude Code CLI](https://code.claude.com/docs/en/cli-reference) 与[沙箱说明](https://code.claude.com/docs/en/sandboxing)、[Antigravity CLI 权限](https://www.antigravity.google/docs/permissions?tab=cli)、[Pi 安全模型](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/security.md)。

## 查询

```sh
ne --help
ne status
ne freshness
ne freshness --session <id>
ne freshness --task <id>
ne freshness --project <id>
ne agent list
ne project resolve
ne project resolve --path /absolute/project/subdirectory
ne session list --project <id> --scope all
ne session list --title "摘要" --path /absolute/path
ne task list --title "任务名称"
ne project list --path /absolute/path
ne search --query "关键词" --project <id>
ne session resolve --agent codex --native-id <原生线程id>
ne session show <id>
ne session summary <id>
ne session summary <id> --item 2 --revision 3
ne session messages <id> --limit 8
ne session search <id> --query "关键词"
ne task list
ne task show <id>
ne project list --scope unarchived
ne project show <id>
ne project summary <id>
```

各命令支持 `--help`。列表支持 `--cursor` 和 `--limit`，默认每页 20 项、最多 50 项，按 ID 排序；会话和任务默认查询未归档项，也可选择 `archived` 或 `all`。

三个列表的 --title 按标题或项目名称作不区分 ASCII 大小写的子串匹配，`%` 和 `_` 按原文匹配。--path 为绝对工作路径，匹配会话工作目录、任务关联目录或项目任一目录；不递归包含子目录。

`ne search` 跨会话搜索标题、关联背景和当前历史消息，复用 FTS5 及无匹配时的字面子串回退。支持 --project、--task、--agent 和 --scope（默认 unarchived）。返回 session_id，消息命中还包含 message_id 对应的 ref_id 和当前 sequence，可用 session messages --from/--to 读取上下文。分页按相关度及稳定 ID 排序，使用 next_cursor 时保持过滤条件。搜索结果随数据库变化，不是固定快照；阅读固定消息历史仍使用下面的消息分页。

消息始终通过当前消息投影读取，不返回被历史改写淘汰的消息。`sequence` 是当前会话内的顺序，时间字段只作参考。`message_id` 是稳定的消息标识；长消息按 Unicode 字符分为连续的 `part`，直到 `last_part=true`。每页默认 8 块、最多 50 块，需完整拼接，工具显示截断时减小 limit 重读。

使用返回的 `next_cursor` 获取下一页，并保持原来的查询条件。一次分页固定消息代次和截止位置，不混入其后新增的消息；历史改写返回 `stale_cursor`，需从第一页重新查询。搜索按大小写敏感的子串匹配完整消息，支持跨分块匹配。根据命中的 sequence，使用 `messages --from <顺序> --to <顺序>` 展开前后讨论。

## 定位当前项目

`ne project resolve` 使用命令进程的当前目录，`--path` 可指定其他已存在的绝对目录。依次匹配当前目录和最近的已登记父目录；相同物理目录的路径别名也可识别，不修改数据库里的路径标识。遇到独立 Git 仓库边界时不继续归入外层项目；未登记的 worktree 可通过已有 Git common dir 身份找到项目，不按项目名称或远程 URL 猜测。

返回 `found`、`query_path`、`project`、`matched_path`、`matched_by` 和 `reason`。`matched_by` 为 `exact_path`、`physical_path`、`ancestor_path` 或 `git_family`。项目包含归档状态，已归档项目也可定位。未匹配时 `found=false`，包括未登记、未知仓库边界或物理路径映射到多个不同项目等情况；不存在的目录返回参数错误。查询不注册目录、创建项目、发现并摄入会话或改写归属，需要注册时显式执行 `ne path register`。助手目录默认定位到助手项目，操作其他项目时应明确指定路径。

## Agent 可用性与提取配置

```sh
ne agent list
ne summary config show
ne summary config set --agent codex --model gpt-6-luna --effort high
ne summary config set --agent pi --provider lmstudio --model local-model
ne summary config set --model ""
ne summary config set --agent none
```

`agent list` 单独查询当前机器，返回各 Agent 的 `supports_cli`、`cli_available`、可执行文件路径、可读取的版本、`supports_extraction` 和可覆盖的运行字段。`supports_cli` 表示 NoEnding 提供可调用的 CLI 入口；有些适配器仅摄入历史，不提供这个入口。`cli_available` 仅表示本机找到可执行文件，不证明已登录、模型可用或远程请求会成功；查询只做本地版本探测，不调用模型，不改写检测缓存或提取配置。

`summary config show/set` 复用设置页的提取配置。`set` 只修改提供的字段；切换 Agent 时先清除旧 Agent 的运行参数，再应用本次参数，同一 Agent 的别名不视为切换。模型、provider 和 effort 传空字符串恢复 Agent 默认值。`--agent none` 停用提取；不能同时设置非空运行参数。当前提取支持 Codex、Claude Code 和 Pi，参数字段遵守各自能力约束。允许预先保存尚未安装的 Agent 配置，不自动选择其他 Agent；实际提取仍会重新检查 CLI 并报告调用结果。保存配置本身不触发同步或提取。

`session resolve --agent AGENT --native-id NATIVE_ID` 按 Agent 与原生主会话／线程 ID 精确查询当前 Home，包含已归档会话。返回 `id`（NoEnding 会话 ID）、`native_id`、`agent`、标题、项目 `project_id`、跟踪任务 `task_ids` 及会话时间等元数据；后续查询、写入及 `--from-session` 使用返回的 `id`。Agent 名称与 `agent list` 一致，接受已有别名，例如 `claude`。

此查询无需桌面在线，不扫描、不启动终端，也不自动同步；尚未摄入或 Home 不匹配时返回 `not_found`。应由调用方明确传入自己的原生身份，不读取 MCP 服务进程的会话环境来猜测调用者，也不按目录、标题或最新活动推断。Codex 使用线程 ID，文件名中回退版本的后缀 ID 不是独立会话身份；内部子 Agent 的 ID 不自动映射为主会话。

## 编辑与归档

```sh
ne task create --title "任务名称" --description "说明" --project <项目id>
ne task update <任务id> --title "新标题"
ne task update <任务id> --description "新说明"
ne task attach <任务id> --session <会话id>
ne task detach <任务id> --session <会话id>
ne task archive <任务id>
ne task unarchive <任务id>
ne session archive <会话id>
ne session unarchive <会话id>
ne project rename <项目id> --name "新名称"
ne project archive <项目id>
ne project unarchive <项目id>
ne path register --path /absolute/existing/directory
ne task project list <任务id>
ne task project set <任务id> --project <项目id1> --project <项目id2>
```

任务创建可重复传入 `--project` 关联多个已有项目。一个会话可跟踪多个任务，attach 只增加该任务的跟踪，不替换其他任务；新增跟踪限于会话当前项目的未归档任务，detach 只取消指定任务的跟踪。

task update 只更改提供的字段，保留归档状态及关联。至少提供 title 或 description，可同时提供；标题不能为空，`--description ""` 清空说明。任务及项目改名同步更新搜索索引，项目名称会标记为用户自定义，不被后续自动命名覆盖。

archive/unarchive 复用界面的归档规则，保留摘要、历史消息和目录关联。归档任务不能新建会话，归档会话不能通过应用继续。归档任务不会连带归档其会话。会话重复执行相同状态的转换会被拒绝；脚本重试使用同一个 request_id 获取原结果。

path register 要求目录已存在且为绝对路径，不创建磁盘目录，也不扫描摄入目录下的会话。项目按目录和 Git/worktree 规则派生。task project set 替换任务的完整项目列表，空列表解除所有项目关联；不会移动目录或会话，也不会清除现有任务跟踪。任务的 --path 查询匹配其关联项目当前的所有目录。

桌面应用运行时会观察这些写操作的数据库提交并刷新。CLI 不提供原始消息编辑、原生会话标题改写、项目身份修改或永久删除。

## 会话终端控制

这些命令需要同一 Home 的桌面应用运行，只控制 NoEnding 内嵌终端：

```sh
ne session new --agent codex --cwd /absolute/project --message "开始工作" --task TASK_ID
ne session new --agent pi --cwd /absolute/project --file prompt.txt
ne session terminals --session SESSION_ID
```

`--task` 可重复；可用 `--model`、`--provider`、`--effort` 覆盖运行参数。省略消息只打开 TUI。新建先返回终端 ID、`session_id=null` 和 `binding=pending`；原生会话被摄入后通过 terminals 查询实际绑定，不猜测 ID。一个会话最多一个有效的 NoEnding 运行终端。

## 会话信箱

所有会话都能收信，包括没有终端 TUI 的 Agent。信箱持久化，数据命令无需桌面运行：

```sh
ne session resolve --agent codex --native-id NATIVE_ID
ne mailbox send --session RECIPIENT --from-session SELF --file message.txt --request-id unique-id
# 可选请求立即通知；省略或 false 时遵循普通自动通知规则
ne mailbox send --session RECIPIENT --from-session SELF --message "请查收新的信息" --notify-immediately true
ne mailbox list --session SELF --state unread --limit 50
ne mailbox read --session SELF --message-id MESSAGE_ID
ne mailbox mark --session SELF --message-id MESSAGE_ID --revision N --state read --from-session SELF
```

`list` 可用 `unread|read|all`，返回 `items`、`unread` 和 `next_cursor`；后续页保持筛选并传 `--cursor`。读取不会自动已读，标记必须提供真正读到的 revision，冲突时重读。改回未读使用 `--state unread`。已读只代表查阅，不代表完成任务。

发件人只有用户和会话 ID。用户消息由界面发送；CLI/MCP 必须解析出有效来源会话，不能使用 user、external 或匿名来源。内嵌终端继承 `NOENDING_TERMINAL_ID` 时按实际绑定核对；外部 Agent 用原生 ID 解析后提供 `--from-session`。上下文与显式来源冲突会拒绝，不按目录猜测。会话 ID 用于本地关联，不是认证凭证。

发送返回消息 ID、收件会话和服务端时间，只证明正文已经保存。长正文直接存库，不改成临时正文文件；请求重试复用相同 request-id 和参数，不因为没有回复而重复发送。消息含 `sender_session_id`（为空表示用户）、`body`、`created_at`、`read_at`、`revision` 和 `notify_immediately`。向来源会话回复使用它的 sender_session_id，不解析正文前缀。

桌面运行期间，新消息可唤醒可恢复的 TUI 会话，首条输入仅为“noending信箱中有 N 条未读信息，请查收”；应用启动不为历史未读补唤醒。运行中的终端由用户控制自动通知，开关不持久化，不能由模型设置。普通自动通知在 Idle／Completed／Unknown 时使用原生排队快捷键。发件会话可用 `--notify-immediately true` 请求立即通知：仍须用户开启自动通知，在 Idle／Completed／Unknown／Working 可发送，Waiting 时不发送；提醒使用与手动通知相同的原生插队快捷键，内容仍为统一的未读数量，多条请求与普通查收提醒合并，不指定单条消息，不改变消息顺序。开启自动通知时会检查已有未读消息及立即通知请求，写入前复查未读状态；已读或删除的请求不再允许 Working 例外。手动查收提醒同样只发未读数量，允许在 Working／Unknown／Interrupted／Failed 插队，Waiting 时不可手动提醒。Unknown 允许开启并尝试自动通知，状态保持未知，不代表输入框已就绪；Failed／Interrupted 会关闭自动通知。用户须保持聊天输入框干净可用。提醒不等待送达确认，也不自动重启终端重发。

右侧栏只显示未读，模型标记已读后自动移除；会话详情只读展示全部消息。自动通知关闭时用户可以编辑双方未读消息或原地二次确认永久删除；编辑保留 ID、来源和发送时间，不保存编辑历史。信箱随收件会话永久删除，删除来源会话不会抹掉其他会话已收到的消息。

收发和标记的 Agent 操作规则见独立的 `noending-mailbox` 技能。技能需在目标 Agent 作用域启用；短提醒本身不能保证未接入 CLI/MCP 的 Agent 自动查收。

## 摘要

摘要以 `summary.items` 列表返回，每个事项包含 `id`、`title`、`description`、`details` 和必填的 `title_locked`。界面可锁定或解锁标题；锁定后 Agent 仍可更新描述和细则，但不能通过改名、删除或合并使锁定标题消失；保留标题时可融合相关描述和细则。CLI 和 MCP 仅开放任务摘要的显式加锁／解锁；会话和项目摘要不开放该入口。未锁定事项可自由调整、合并、拆分、增删和排序。版本号仍属于整份摘要，每项还有稳定的 `id`；已有事项编辑、改名或重排时保留 ID，合并保留一个延续 ID，拆分仅一个沿用原 ID；新增事项省略 ID，由程序分配。

```sh
ne summary search --query "关键词" --project <id> --scope all
ne summary search --query "关键词" --kind task --limit 8
ne session summary <id> --item 2 --revision 3
ne task summary <id> --item 1 --revision 4
ne project summary <id> --item 1 --revision 2
```

`summary search` 按事项匹配标题、描述和细则，匹配为字面子串（ASCII 不区分大小写），返回命中事项及所属 `kind`、`id`、对象 `title`、`revision` 和从 1 开始的 `item_number`。支持 `--kind session|task|project`、`--project`、`--scope unarchived|archived|all`（默认 unarchived）和 `--cursor`，默认每页 20 项、最多 50 项。检索不读取原始消息，顶部查找范围也不受影响。

按序号读取时必须同时提供 `--item` 和检索结果中的 `--revision`，避免摘要重组后读取错位内容。摘要版本变化返回 `stale_result`；分页范围或摘要版本变化返回 `stale_cursor`，需要重新检索。事项序号仅在对应摘要版本内有效。

桌面详情页支持逐项编辑摘要：点击事项旁的编辑按钮修改标题、描述和细则，标题栏的「＋」添加事项。编辑状态可以删除未锁定事项，也可删除最后一项清空摘要；锁定标题须先解锁才能改名或删除。手动编辑保留提取进度，编辑期间当前页面的模型更新按钮暂停，版本冲突会保留草稿。

`summary set` 文件使用 UTF-8 JSON，格式为 `{"items":[{"title":"事项标题","description":"简短描述","details":["细则"],"title_locked":false}]}`，一次替换完整列表；须保留原有锁定标题及其 `title_locked=true`，不能新增、移除或转移标题锁。至少包含一个事项，标题必须是非空单行文字，描述允许为空字符串，细则允许为空数组；每条细则必须非空。旧 Markdown 正文不再接受。默认提取提示词要求会话最多 15 个事项，任务和项目最多 10 个事项，并建议标题不超过 20 字符、描述不超过 60 字符、每条细则不超过 80 字符、整个事项不超过 300 字符；这些数量和篇幅要求用于指导模型整理，后端不据此拒绝或截断提取及手动编辑结果。

```sh
ne summary update session <id>
ne summary regenerate task <id>
ne summary update project <id>
ne summary set session <id> --file summary.json --revision <当前版本>
ne summary lock task <id> --item 1 --revision <当前版本>
ne summary unlock task <id> --item 1 --revision <当前版本>
```

`summary lock task` / `summary unlock task` 显式切换一个任务事项的标题锁，必须提供当前 `--revision` 和从 1 开始的 `--item`，返回新版本及完整摘要。它不修改描述、细则或提取消费位置；普通 `summary set` 仍不得改变锁状态。多项操作逐次采用新版本，版本冲突时重新读取并定位，不能复用旧序号。

发布、完成和废弃任务待办使用专用接口，分别在事务内执行发布锁定、完成解锁保留或废弃删除，不再分别调用 `summary set` 和锁接口：

```sh
ne task todo publish TASK_ID --file todo.json --revision 0
ne task todo publish TASK_ID --item 1 --file todo.json --revision 1
ne task todo complete TASK_ID --item 1 --revision 2
# 如需同时回写结果，可选增加 --file result.json
# 废弃另一个待办时，使用它当前的序号和版本
ne task todo discard TASK_ID --item NUMBER --revision N
```

发布文件只需 `{"title":"修复会话同步"}`，可选增加 `description` 描述目标与必要边界，不接受 `details` 或锁字段。新事项描述省略时为空，细则初始化为空数组。省略 `--item` 时追加；指定时修改该事项标题与已提供的描述并锁定，保留已有细则和未提供的描述；显式传空字符串可清空描述，已锁定标题不能改名。

待办事项的 `details` 记录本事项已确认的进展、结果、阻碍和验证情况，不是待办子项或执行步骤清单。推进时通过普通摘要编辑维护细则，保留其他事项与锁；不虚构进展，不按细则条数判断完成。

完成操作默认只解锁标题，保留原有标题、描述和进展细则，使其成为普通摘要事项。可选传 `--file result.json` 回写结果，例如 `{"description":"同步异常已修复并验证","details":["回归验证通过，已有消息保持完整"]}`；提供时完整替换描述和细则，结果至少一处非空，所需旧背景和有效进展应一并保留。

`task todo discard` 直接移除指定事项的标题、描述和细则，不保留废弃标记或占位，也可删除最后一项并清空任务摘要。完成和废弃均要求目标标题已锁定；这些是处理动作，不新增事项状态，不归档任务，不触发 Agent 或提取。其他事项、提取消费位置和任务归档状态保持原样。

返回处理对象的 `item`、新 `revision` 与完整 `summary`。版本冲突或校验失败不会部分写入；通过 `--request-id` 去重，同一请求重试保持编号和全部参数，避免重复删除序号移位后的其他事项。每次操作后的序号都可能变化，冲突时重读，不复用旧序号。

普通历史提取保留已有锁，不主动发布、完成或废弃待办。完成需核实实际结果；废弃需明确放弃对应工作，受阻、暂缓或未完成都不等于废弃。标题锁不是完成状态，不把所有锁定结论视为待办，也不把普通未锁定事项视为已完成。单独锁定／解锁仍用于显式调整标题保护。

更新和重新生成等待配置的 Agent 调用及后端校验、入库全部完成后，直接返回最终结果。失败时 `ok=false` 并返回非零退出码；部分完成状态仍需检查结果字段。`update` 沿用旧摘要，`regenerate` 忽略目标旧摘要，遵守现有提取和并发规则。同一会话参与的独立和批量提取共用跨进程锁，桌面与 CLI 不能重叠提取。Ctrl+C 会取消当前命令并结束它启动的 headless Agent 进程树；同步在安全边界停止，已提交数据保留。强制杀进程无法保证执行清理，未确认完成的写请求需要核对业务状态后再操作。

人工编辑使用 UTF-8 JSON 文件，必须提供查询得到的当前摘要版本；版本不一致时拒绝覆盖。编辑只改内容和版本，不把待处理消息标记为已提取。

## 记忆更新状态

`ne freshness` 读取全部已登记来源和本机已入库会话的检查状态；`--session`、`--task`、`--project` 三选一，查询对应范围及摘要覆盖。查询不扫描文件、不调用模型，包含已归档会话。项目返回所有来源的扫描状态，因为新会话可能来自尚无该项目记录的 Agent；任务和会话返回已知关联会话所属来源。未登记来源的会话仍可查询单会话状态。

- `all_sources` 是最近一次全部来源同步的观察；`sources[].check` 是各来源扫描状态；`session_check` 只在单会话查询中提供。单会话同步不更新来源或全部来源时间，单来源同步不更新全部来源时间。
- `started_at` / `finished_at` 记录最近已结束检查；`outcome` 为 `success`、`partial`、`failed`、`missing`、`cancelled` 或未知。`checked_at` 是最近成功检查的起始时间，采用保守观察边界；成功但无新消息也推进，失败保留之前的成功时间。它不保证之后源文件没有继续变化。未安装 Agent 的默认路径可显示 `missing`，全部来源因此可为 `partial`，不代表已经摄入的数据丢失。
- `sessions` 汇总范围内会话数量、曾成功检查数、从未成功检查数、最近检查失败／来源缺失数，以及成功检查时间的最早／最晚值；这些计数可交叉，曾成功检查的会话也可能最近失败。查询不验证当前磁盘状态。
- `summary` 与普通摘要读取返回的 `freshness` 含义相同。`refreshed_at` 是最近成功提取并提交的时间（含保留摘要但确认材料已处理的跳过结果）；`updated_at` 仍表示编辑时间，手动编辑和标题锁定不会推进 `refreshed_at` 或消费消息。
- `pending_messages` 是已入库、尚未被目标摘要消费的 user/assistant 消息数，按会话去重，不是长消息分页块数或工具事件数。任务／项目以自身消费位置计算，单独刷新子会话摘要不会将这个计数归零。新关联会话的未覆盖消息也计入，因此它不等同于按时间统计“刚刚新产生”的消息。
- `summary_changed_sessions` 表示会话摘要版本变化、尚未汇入目标；`scope_changed` 表示关联或元数据输入变化。`history_rewritten_sessions` 表示已消费历史发生改写；`unknown_coverage_sessions` 表示消费位置无法确定。后两者非零时 `pending_messages=null`，不冒充零条新增；结合 `pending` 决定是否更新或重新提取。

版本 1 数据会在同一事务内兼容升级，保留现有数据。无法还原的同步／提取时间为 `null`；只有可以通过消费版本核实的位置才会补齐。零条待处理只描述已入库材料，尚未检查的源文件可能仍有新内容。

## 会话同步

查询只读取已入库的数据。需要读取源文件中的最新消息时，显式调用：

```sh
ne source list
ne sync
ne sync --source <来源id>
ne sync --session <会话id>
```

`sync` 增量同步全部已注册来源，`--source` 只同步指定来源，`--session` 只刷新指定会话，后二者互斥。命令等待同步完成，返回 `discovered`、`messages` 和 `failures`；全部来源有部分失败时返回 `sync_partial_failure`，统计放在 `error.details` 中。同步不触发摘要提取。桌面同时运行时会观察数据库提交并刷新显示。

Codex 以转录头声明的线程 ID 识别会话；回退产生的文件版本仍归属同一会话，显示当前有效转录文件路径。同步会按历史引用的截止位置读取前序消息，再接入当前文件；真正具有独立线程 ID 的分叉保留为独立会话。普通和压缩转录都适用。无法确认当前版本或历史引用不完整时报告同步失败，保留已入库内容。

## 返回与数据目录

成功：`{"schema_version":1,"ok":true,"data":...}`，退出码 0。

失败：`{"schema_version":1,"ok":false,"error":{"code":"...","message":"..."}}`，退出码非零。error 可包含 details。常见错误包括 `invalid_arguments`、`not_found`、`locked`、`scope_mismatch`、`stale_cursor`、`stale_result`、`cancelled`、`request_running` 和 `request_interrupted`。

业务写操作自动分配 request_id，成功时返回该字段，失败时放在 error.details。脚本可预先指定 --request-id（1～128 个字母、数字、短横线或下划线）：

```sh
ne task create --title "任务名称" --request-id create-task-1
ne request show create-task-1
```

同一数据目录下，以相同编号及完全相同参数重试会返回原结果，不再次执行；编号用于其他操作时返回 `request_conflict`。失败结果也保留，准备重新执行时使用新编号。记录保存在 `runtime/cli-requests/`，跨 CLI 进程保留，不随命令退出而清空。`request show` 返回 `running`、`completed` 或 `interrupted`。进程异常退出而未写完回执时，重试返回 `request_interrupted`，需要核对业务状态；不自动重复可能已经提交的操作。清理运行目录会失去对应的去重记录。

默认按 `NOENDING_HOME`、应用保存的当前存储位置、默认存储目录依次定位数据，`--home <目录>` 可明确指定 Home；待迁移位置由桌面启动处理，CLI 不执行目录搬迁。桌面、CLI 和 MCP 共用数据库身份、基线、格式版本及开发结构检查入口；不兼容时保留数据并返回错误。格式版本独立于应用版本，只在承诺兼容的发布中递增；本地开发结构调整按 AGENTS.md 的草案规则处理。桌面或 CLI 使用数据目录期间会持有共享锁，目录迁移必须等待这些进程释放访问。`--connection <文件>` 仅用于提取查询，不能用于普通业务命令。

摘要提取为 Agent 提供完整可执行文件路径及一次性连接，只开放 `extraction inputs/read/search`，不能通过该连接调用通用编辑命令；不提供 SQL、原始消息编辑或 `extraction submit`。提取结果仍由后端根据最终回复校验并保存。

事项定位优先使用稳定 ID：CLI 在现有 `--item NUMBER` 的位置改传 `--item-id ITEM_ID`；MCP 在 `item` 的位置改传 `item_id`，两者不能同时提供。读取单个事项、标题锁及待办操作都需要当前 `revision`。序号仍只属于对应摘要版本。查询和待办操作返回事项 ID；摘要全量编辑须保留延续事项的 `id`，新增事项可省略 `id`，由 NoEnding 生成。ID 不代替并发版本检查，已删除或被合并掉的 ID 不再有效。
