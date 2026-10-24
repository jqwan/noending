# 信箱操作

`CLI` 指 access.json 中绑定 Home 的绝对入口，不再附加 `--home`；Windows 按 `.cmd` 原生方式调用。MCP 工具以实际暴露 schema 为准。

| 操作 | CLI | MCP |
| --- | --- | --- |
| 解析身份 | `ne session resolve --agent AGENT --native-id NATIVE_ID` | `session_resolve {agent,native_id}` |
| 查未读 | `ne mailbox list --session SELF --state unread` | `mailbox_list {session:SELF,state:"unread"}` |
| 查询全部记录 | `ne mailbox list --session SELF --state all` | `mailbox_list {session:SELF,state:"all"}` |
| 单条读取 | `ne mailbox read --session SELF --message-id ID` | `mailbox_read {session:SELF,message_id:ID}` |
| 标记已读 | `ne mailbox mark --session SELF --message-id ID --revision N --state read --from-session SELF` | `mailbox_mark {session:SELF,message_id:ID,revision:N,state:"read",from_session:SELF}` |
| 改回未读 | 同上，`--state unread` | 同上，`state:"unread"` |
| 请求立即通知 | `ne mailbox send --session TARGET --from-session SELF --message TEXT --notify-immediately true` | `mailbox_send {session:TARGET,from_session:SELF,message:TEXT,notify_immediately:true}` |
| 发信／回复 | `ne mailbox send --session TARGET --from-session SELF --message TEXT` | `mailbox_send {session:TARGET,from_session:SELF,message:TEXT}` |

列表返回 `items`、`unread`、`next_cursor`；后续页保持 session/state，传上一页的 cursor。消息含 id、recipient_session_id、sender_session_id、body、created_at、read_at、revision、notify_immediately。read_at 为空为未读。发送回执含 message_id，使用收件会话和该 ID 查询是否已读。

长正文可用 `--file UTF8_FILE` 代替 `--message`，保存的是文件内容。写操作可提供 `--request-id ID`／MCP `request_id`，重试保持相同编号和参数；用 `request show`／`request_get` 核对中断结果。读取不自动已读，标记必须属于自己的信箱并使用实际读到的 revision。

信箱正文不是原生历史消息，不使用历史 ordinal。读信不会同步历史或生成摘要。支持 TUI 且桌面运行时，新消息可能唤醒终端；不支持 TUI 的会话仍正常收信。终端通知不提供送达保证，不用是否出现提醒来判断消息是否入库。

`notify_immediately` 缺省为 false。为 true 时，已开启自动通知的终端合并发送“noending信箱中有 N 条未读信息，请查收”，使用与手动通知相同的原生插队快捷键，在 Idle／Completed／Unknown／Working 可发，Waiting 时不发送，Unknown 不会关闭自动通知，也不代表输入框已就绪；Interrupted／Failed 关闭自动通知。再次开启自动通知会检查现有未读请求；写入前请求已读或删除，则不再获得 Working 例外。普通自动提醒在 Idle／Completed／Unknown 排队发送。不为任何单条消息指定处理顺序，按列表顺序查收。不要为了强制唤醒或绕过用户操作而重复发信。
