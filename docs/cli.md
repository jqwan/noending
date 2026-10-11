# NoEnding CLI

CLI 连接正在运行的桌面应用，复用后端服务；CLI 不打开数据库。`ne` 是短命令，`noending` 保留完整名称，子命令不使用缩写。输出为 JSON，日志不写入标准输出。

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
ne session list --project <id> --scope all
ne session list --title "摘要" --path /absolute/path
ne task list --title "任务名称"
ne project list --path /absolute/path
ne search --query "关键词" --project <id>
ne session show <id>
ne session summary <id>
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

## 编辑与归档

```sh
ne preference list --scope user
ne preference list --scope project --project <id>
ne preference add --scope user --content "说明保持简明"
ne preference update <id> --content "先说明结论，再说明原因"
ne preference delete <id>
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

偏好不能改变所属范围，CLI 不提供锁定状态修改；修改或删除时以后端当前锁定状态为准。用户仍可在界面管理所有条目。任务创建可重复传入 `--project` 关联多个已有项目。一个会话可跟踪多个任务，attach 只增加该任务的跟踪，不替换其他任务；新增跟踪限于会话当前项目的未归档任务，detach 只取消指定任务的跟踪。

task update 只更改提供的字段，保留归档状态及关联。至少提供 title 或 description，可同时提供；标题不能为空，`--description ""` 清空说明。任务及项目改名同步更新搜索索引，项目名称会标记为用户自定义，不被后续自动命名覆盖。

archive/unarchive 复用界面的归档规则，保留摘要、历史消息和目录关联。归档任务不能新建会话，归档会话不能通过应用继续。归档任务不会连带归档其会话。会话重复执行相同状态的转换会被拒绝；脚本重试使用同一个 request_id 获取原结果。

path register 要求目录已存在且为绝对路径，不创建磁盘目录，也不扫描摄入目录下的会话。项目按目录和 Git/worktree 规则派生。task project set 替换任务的完整项目列表，空列表解除所有项目关联；不会移动目录或会话，也不会清除现有任务跟踪。任务的 --path 查询匹配其关联项目当前的所有目录。

这些写操作完成后会通知应用刷新。CLI 不提供原始消息编辑、原生会话标题改写、项目身份修改或永久删除。

## 摘要

```sh
ne summary update session <id>
ne summary regenerate task <id>
ne summary update project <id>
ne summary update session <id> --wait --timeout 600
ne job show <job_id>
ne job wait <job_id> --timeout 600
ne job list
ne summary set session <id> --file summary.md --revision <当前版本>
```

更新或重新生成立即返回 job_id，由后端调用配置的 Agent。作业完成后，result 返回后端确认的更新结果，包括部分失败或偏好错误；启动成功不代表已入库。作业记录只在本次应用运行中保留。`update` 沿用旧摘要，`regenerate` 忽略目标旧摘要，遵守现有提取和并发规则。

--wait 或 job wait 在客户端等待，完成时返回作业及后端确认的 result；作业失败时 ok=false、退出码非零，error.details 包含作业信息。默认等待 600 秒，可用 --timeout 设置 1～86400 秒。等待超时返回 wait_timeout 并保留作业 ID，不取消作业，可继续用 job show/wait 查询。部分完成与偏好错误仍需检查 result。

人工编辑使用 UTF-8 Markdown 文件，必须提供查询得到的当前摘要版本；版本不一致时拒绝覆盖。编辑只改正文和版本，不把待处理消息标记为已提取。

## 返回与连接

成功：`{"schema_version":1,"ok":true,"data":...}`，退出码 0。

失败：`{"schema_version":1,"ok":false,"error":{"code":"...","message":"..."}}`，退出码非零。error 可包含 details。常见错误包括 `invalid_arguments`、`not_found`、`locked`、`scope_mismatch`、`stale_cursor`、`stale_result`、`app_unavailable` 和 `request_timeout`。

业务写操作自动分配 request_id，成功时返回该字段，失败时放在 error.details。脚本可预先指定 --request-id（1～128 个字母、数字、短横线或下划线）：

```sh
ne task create --title "任务名称" --request-id create-task-1
ne request show create-task-1
```

同一应用运行期间，以相同编号及完全相同参数重试会返回原结果，不再次执行；编号用于其他操作时返回 request_conflict。失败结果也保留，更改参数或准备重新执行时使用新编号。request show 返回 running 或 completed 及原始 response；超时后先查询该记录和业务状态。记录仅在本次应用运行中保留，重启后不能依靠旧编号去重；最多保留 1000 条或约 64 MiB 的结果，达到上限时拒绝新写请求，不淘汰旧记录后重新执行。

默认按 `NOENDING_HOME`、应用保存的存储位置、默认存储目录依次定位连接。`--connection <文件>` 可指定连接。摘要提取为 Agent 提供完整可执行文件路径及一次性连接，只开放 `extraction inputs/read/search`，不能通过该连接调用通用编辑命令；不提供 SQL、原始消息编辑或 `extraction submit`。提取结果仍由后端根据最终回复校验并保存。
