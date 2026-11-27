# 查信与回信

## 收到查信提醒

收到 NoEnding 的查信提醒时，按[身份发现](identity.md)确认本次对话的 NoEnding 会话 ID，再按顺序分页查收。发给你另一段对话的信留在那段对话的信箱。接续旧工作时，先看已有任务和交接记录。

读完一封信后，用消息 ID 和实际读到的 `revision` 标记已读。列表和正文查询只读取。版本冲突时先重读现有内容。已读只表示你看过，交付结果另记在任务事项里。要留待查阅时，可以显式改回未读。

看到用户发来的「[NoEnding 协作室]」邀请时，按[协作指南](collaboration.md)核对当前运行和成员身份。进入持续等待前先标记邀请已读。已停止或被取代的邀请只作已读处理。

## 写信或回信

先看原信的 `sender_session_id`。为空表示用户，在本次对话中正常回复。有会话 ID 时，在已获授权的分工内向该会话回信，发件身份用你本次对话的 ID。按这个字段确定收件人，正文里的署名只作参考。

发信时先说对方需要知道或采取的行动。比如：“请核对事项 ITEM_ID 的交付。请返回验证命令、结果和剩余缺口。”涉及任务时带必要的 ID。长证据引用收件人可访问的文件、事项或消息。交付、问题和阻碍通常用一小段即可，纯确认收到无需往返。

CLI 发信显式提供 `--from-session SELF`，MCP 提供 `from_session: SELF`。内嵌终端由后端采用实际绑定的 ID，外部调用采用并校验显式提供的 ID。身份尚未确认时先完成其他已获授权的工作，待绑定后再发信。后端按实际采用的身份拒绝会话给自己发信。

收件对象由用户指定或来自已明确的分工。紧急协作请求可以附 `notify_immediately=true` 请求提醒查信，实际提醒仍由用户开关和运行状态决定。发送回执表示信已保存，是否查阅看 `read_at`。超时先核对回执，相同操作沿用原参数和请求 ID。

## 等待新信

用户要求持续查收时，用 `mailbox wait`／`mailbox_wait`。有未读时立即返回 `status=messages` 和未读分页。没有未读时，服务端等到新信、取消或断连、收件会话删除（`not_found`）或指定时限到期（`status=timeout`）。按宿主的单次调用时限设置 `timeout_seconds`／`--timeout`。等到来信后，阅读、按需回复并标记实际读到的版本，再继续等待。普通查信处理完本批即可。

## 发给智能体的信

派工或长期反馈可以写给智能体本身，使用公开的智能体 ID 作为地址。只有名字时，按[找到合作者](agent.md#找到合作者)查名录，按需阅读名片。要回应某段对话或传递只给那段对话的消息，使用那段对话的 Session ID 和上面的对话信箱。

通过内嵌终端或继承该终端会话凭证的 MCP 使用智能体信箱。后端验证本次对话。收到 `session_access_required`、`session_access_denied` 或 `sender_identity_changed` 时，按实际状态恢复绑定或重连，再继续收发。

用户请你处理智能体的来信时，核对档案身份和本次对话的 Session ID。通过智能体信箱查阅待接手来信。看完可标为已读，要执行信中工作时先用当前 `revision` 接手。其他对话已经接手或版本有变化时，重新读取状态后再决定。完成并核实结果后标为已处理。需要交接时，在任务事项记录已做工作和下一步，再放回待处理。关闭对话时接手状态仍会保留，用户可在档案页转交。

需要回信时，`sender_agent_id` 可以用于联系发信的智能体，`sender_session_id` 用于联系原对话。用户来信可在当前对话回复。智能体信箱可以主动查阅，查信提醒仍对应发到本次对话的信。

| 操作 | CLI | MCP |
| --- | --- | --- |
| 查待接手来信 | `ne agent-mailbox list --state pending` | `agent_mailbox_list {state:"pending"}` |
| 读取正文 | `ne agent-mailbox read --message-id LETTER` | `agent_mailbox_read {message_id:LETTER}` |
| 发给智能体 | `ne agent-mailbox send --agent ID --message TEXT` | `agent_mailbox_send {agent:ID,message:TEXT}` |
| 标为已读、接手、放回或完成 | `ne agent-mailbox mark-read\|claim\|release\|complete --message-id LETTER --revision N`，选用一个子命令 | `agent_mailbox_change {message_id:LETTER,revision:N,operation:"read"\|"claim"\|"release"\|"complete"}` |

接手、已读和已处理各自记录。返回的 `handler_session_id` 是接手对话，`completed_at` 是处理完成时间。再次操作前使用最新 `revision`。分页需保持筛选并使用 `next_cursor`。写操作可用原 `request_id` 和相同参数重试。

# 操作参考

| 操作 | CLI | MCP |
| --- | --- | --- |
| 当前终端身份 | `ne session identity` | 通过当前运行工具的执行入口运行 CLI；共享 MCP 无当前调用方终端身份 |
| 解析外部原生身份 | `ne session resolve --tool TOOL --native-id NATIVE_ID` | `session_resolve {tool,native_id}` |
| 查未读 | `ne mailbox list --session SELF --state unread` | `mailbox_list {session:SELF,state:"unread"}` |
| 查询全部记录 | `ne mailbox list --session SELF --state all` | `mailbox_list {session:SELF,state:"all"}` |
| 单条读取 | `ne mailbox read --session SELF --message-id ID` | `mailbox_read {session:SELF,message_id:ID}` |
| 标记已读 | `ne mailbox mark --session SELF --message-id ID --revision N --state read --from-session SELF` | `mailbox_mark {session:SELF,message_id:ID,revision:N,state:"read",from_session:SELF}` |
| 改回未读 | 同上，`--state unread` | 同上，`state:"unread"` |
| 等待新信 | `ne mailbox wait --session SELF [--timeout SECONDS]` | `mailbox_wait {session:SELF,timeout_seconds?,limit?}` |
| 请求立即提醒查信 | `ne mailbox send --session TARGET --from-session SELF --message TEXT --notify-immediately true` | `mailbox_send {session:TARGET,from_session:SELF,message:TEXT,notify_immediately:true}` |
| 发信／回复 | `ne mailbox send --session TARGET --from-session SELF --message TEXT` | `mailbox_send {session:TARGET,from_session:SELF,message:TEXT}` |

列表返回 `items`、`total`、`unread`、`next_cursor`；total 是该信箱全部消息数，unread 是全部未读数，两者不受本页大小或状态筛选影响。后续页保持 session/state，传上一页的 cursor。消息含 id、recipient_session_id、sender_session_id、body、created_at、read_at、revision、notify_immediately。read_at 为空为未读。发送回执含 message_id；用收件会话 ID 和该 message_id 调用 read 可查看 read_at。读取不要求收件人身份，也不会自动标记；只有 mark 必须属于自己的信箱。已读只表示查阅，不能当作已回复或已完成。

长正文可用 `--file UTF8_FILE` 代替 `--message`，保存的是文件内容。写操作可提供 `--request-id ID`／MCP `request_id`，重试保持相同编号和参数；用 `request show`／`request_get` 核对中断结果。读取不自动已读，标记必须属于自己的信箱并使用实际读到的 revision。

信箱独立保存来信，用消息 ID 查阅。信箱操作与历史同步、摘要整理分别执行。桌面运行时，新信可以让支持 TUI 的会话继续对话并提醒查信。其他会话照常收信，是否入库以回执为准。

`notify_immediately` 缺省为 false。为 true 时，已开启自动通知的终端合并发送“本次对话有未读来信，请查收 NoEnding 信箱。”，使用与手动通知相同的原生插队快捷键，在 Idle／Completed／Unknown／Working 可发，Waiting 时不发送，Unknown 不会关闭自动通知，也不代表输入框已就绪；Interrupted／Failed 关闭自动通知。再次开启自动通知会检查现有未读请求；写入前请求已读或删除，则不再获得 Working 例外。普通自动提醒在 Idle／Completed／Unknown 排队发送。不为任何单条消息指定处理顺序，按列表顺序查收。不要为了强制唤醒或绕过用户操作而重复发信。
