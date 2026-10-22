# NoEnding CLI

CLI 独立打开 NoEnding 数据库，复用桌面应用的业务模块，无需启动桌面应用。命令等待操作完成后返回并退出；不启动常驻服务，也不自动扫描会话、技能或项目。`ne` 是短命令，`noending` 保留完整名称，子命令不使用缩写。输出为 JSON，日志不写入标准输出。

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

## 查询

```sh
ne --help
ne status
ne agent list
ne project resolve
ne project resolve --path /absolute/project/subdirectory
ne session list --project <id> --scope all
ne session list --title "摘要" --path /absolute/path
ne task list --title "任务名称"
ne project list --path /absolute/path
ne search --query "关键词" --project <id>
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

桌面应用运行时会观察这些写操作的数据库提交并刷新。CLI 不提供原始消息编辑、原生会话标题改写、项目身份修改或永久删除。内置助手的 Codex 会话仅额外开放 NoEnding 的数据、运行、日志和访问锁目录，以便独立 CLI 工作，不开放其他项目工作目录。

## 摘要

摘要以 `summary.items` 列表返回，每个事项包含 `title`、`description`、`details` 和必填的 `title_locked`。界面可锁定或解锁标题；锁定后 Agent 仍可更新描述和细则，但不能通过改名、删除或合并使锁定标题消失；保留标题时可融合相关描述和细则。CLI 和 MCP 不能改变锁定状态。未锁定事项可自由调整、合并、拆分、增删和排序。版本号仍属于整份摘要，事项没有固定 ID。

```sh
ne summary search --query "关键词" --project <id> --scope all
ne summary search --query "关键词" --kind task --limit 8
ne session summary <id> --item 2 --revision 3
ne task summary <id> --item 1 --revision 4
ne project summary <id> --item 1 --revision 2
```

`summary search` 按事项匹配标题、描述和细则，匹配为字面子串（ASCII 不区分大小写），返回命中事项及所属 `kind`、`id`、对象 `title`、`revision` 和从 1 开始的 `item_number`。支持 `--kind session|task|project`、`--project`、`--scope unarchived|archived|all`（默认 unarchived）和 `--cursor`，默认每页 20 项、最多 50 项。检索不读取原始消息，顶部查找范围也不受影响。

按序号读取时必须同时提供 `--item` 和检索结果中的 `--revision`，避免摘要重组后读取错位内容。摘要版本变化返回 `stale_result`；分页范围或摘要版本变化返回 `stale_cursor`，需要重新检索。事项序号仅在对应摘要版本内有效。

`summary set` 文件使用 UTF-8 JSON，格式为 `{"items":[{"title":"事项标题","description":"简短描述","details":["细则"],"title_locked":false}]}`，一次替换完整列表；须保留原有锁定标题及其 `title_locked=true`，不能新增、移除或转移标题锁。至少包含一个事项，标题必须是非空单行文字，描述允许为空字符串，细则允许为空数组；每条细则必须非空。旧 Markdown 正文不再接受。默认提取提示词要求会话最多 15 个事项，任务和项目最多 10 个事项，并建议标题不超过 20 字符、描述不超过 60 字符、每条细则不超过 80 字符、整个事项不超过 300 字符；这些数量和篇幅要求用于指导模型整理，后端不据此拒绝或截断提取及手动编辑结果。

```sh
ne summary update session <id>
ne summary regenerate task <id>
ne summary update project <id>
ne summary set session <id> --file summary.json --revision <当前版本>
```

更新和重新生成等待配置的 Agent 调用及后端校验、入库全部完成后，直接返回最终结果。失败时 `ok=false` 并返回非零退出码；部分完成状态仍需检查结果字段。`update` 沿用旧摘要，`regenerate` 忽略目标旧摘要，遵守现有提取和并发规则。同一会话参与的独立和批量提取共用跨进程锁，桌面与 CLI 不能重叠提取。Ctrl+C 会取消当前命令并结束它启动的 headless Agent 进程树；同步在安全边界停止，已提交数据保留。强制杀进程无法保证执行清理，未确认完成的写请求需要核对业务状态后再操作。

人工编辑使用 UTF-8 JSON 文件，必须提供查询得到的当前摘要版本；版本不一致时拒绝覆盖。编辑只改内容和版本，不把待处理消息标记为已提取。

## 会话同步

查询只读取已入库的数据。需要读取源文件中的最新消息时，显式调用：

```sh
ne source list
ne sync
ne sync --source <来源id>
ne sync --session <会话id>
```

`sync` 增量同步全部已注册来源，`--source` 只同步指定来源，`--session` 只刷新指定会话，后二者互斥。命令等待同步完成，返回 `discovered`、`messages` 和 `failures`；全部来源有部分失败时返回 `sync_partial_failure`，统计放在 `error.details` 中。同步不触发摘要提取。桌面同时运行时会观察数据库提交并刷新显示。

## 返回与数据目录

成功：`{"schema_version":1,"ok":true,"data":...}`，退出码 0。

失败：`{"schema_version":1,"ok":false,"error":{"code":"...","message":"..."}}`，退出码非零。error 可包含 details。常见错误包括 `invalid_arguments`、`not_found`、`locked`、`scope_mismatch`、`stale_cursor`、`stale_result`、`cancelled`、`request_running` 和 `request_interrupted`。

业务写操作自动分配 request_id，成功时返回该字段，失败时放在 error.details。脚本可预先指定 --request-id（1～128 个字母、数字、短横线或下划线）：

```sh
ne task create --title "任务名称" --request-id create-task-1
ne request show create-task-1
```

同一数据目录下，以相同编号及完全相同参数重试会返回原结果，不再次执行；编号用于其他操作时返回 `request_conflict`。失败结果也保留，准备重新执行时使用新编号。记录保存在 `runtime/cli-requests/`，跨 CLI 进程保留，不随命令退出而清空。`request show` 返回 `running`、`completed` 或 `interrupted`。进程异常退出而未写完回执时，重试返回 `request_interrupted`，需要核对业务状态；不自动重复可能已经提交的操作。清理运行目录会失去对应的去重记录。

默认按 `NOENDING_HOME`、应用保存的当前存储位置、默认存储目录依次定位数据，`--home <目录>` 可明确指定 Home；待迁移位置由桌面启动处理，CLI 不执行目录搬迁。桌面、CLI 和 MCP 共用数据库身份、版本及结构检查入口；不兼容时保留数据并返回错误。桌面或 CLI 使用数据目录期间会持有共享锁，目录迁移必须等待这些进程释放访问。`--connection <文件>` 仅用于提取查询，不能用于普通业务命令。

摘要提取为 Agent 提供完整可执行文件路径及一次性连接，只开放 `extraction inputs/read/search`，不能通过该连接调用通用编辑命令；不提供 SQL、原始消息编辑或 `extraction submit`。提取结果仍由后端根据最终回复校验并保存。
