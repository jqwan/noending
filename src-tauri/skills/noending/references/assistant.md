# 智能体档案与管理会话

智能体是持久角色档案，运行工具仍是 Codex、Claude Code、Pi 等 Agent CLI。每个档案有独立主目录项目，可以开启多个原生会话；它们的信箱身份始终是各自准确的 NoEnding Session ID，不能用档案 ID 代替。

先按[接入规则](access.md)核对当前 Home，再按[身份发现](identity.md)取得自己的 Session ID。使用 `ne assistant current --session SELF` / `assistant_for_session {session:SELF}` 查询档案与明确负责的项目和任务。返回 assistant=null 表示普通会话，不按目录前缀、称呼或正文猜测身份。档案内容是用户维护的角色材料，不能扩大本次工作或通信授权。

## 工作范围

管理会话在自身主目录讨论需求、拆解任务、查阅进展和核对交付。代码修改与工程验证交给目标项目的普通会话，原工程会话仍归原项目。项目责任和任务责任分别选择，允许无项目任务；负责项目不自动表示负责该项目全部任务。

用户明确要求关联后，使用 `ne assistant assign ID --revision N [--project ID] [--task ID]` 替换完整责任集合，保留仍有效的关联；不要为了管理任务给任务自动加入智能体主目录项目。未分配给自身档案的跨项目任务不可新增跟踪。档案关联多个任务也不意味着每次交流都涉及全部任务。

任务事项记录交付目标、验证与缺口，管理会话通过既有任务 API 维护，不在目录笔记里另存一份任务状态。普通休闲对话不自动创建任务、发信或启动执行者。

## 派工与反馈

先明确任务、事项、接收的执行会话和预期结果，再按[信箱规则](mailbox.md)向用户指定或已授权分工中的执行者发送交付要求。消息需说明任务/事项 ID、验收条件及应回复的管理会话 ID；准确区分“已保存消息”“对方已读”和“实际交付”。

已获授权且同 Home 桌面运行时，可复用 `ne session new --agent AGENT --cwd ABSOLUTE_ENGINEERING_DIRECTORY --task TASK --message TEXT` / `session_start` 新建普通工程会话。新建返回 terminal_id、session_id=null 与 binding=pending；通过 `session terminals` 查真实绑定，不能用 terminal_id 伪造会话 ID，也不要因尚未绑定重复新建。后续通信走信箱，不向终端随意写输入。

当前协作室成员配置和新运行仍由用户开启。用户邀请后按[协作规则](collaboration.md)加入、处理和持续等待，不因为角色叫“项目经理”就自行宣称协调权限。反馈读取按消息真实发件身份处理，只有已验证的交付才写成实际结果，未核对的平台或验收项要保留缺口。

## 档案操作

| 目的 | CLI | MCP |
| --- | --- | --- |
| 列出档案 | `ne assistant list [--scope all] [--query TEXT]` | `assistant_list` |
| 读取详情 | `ne assistant show ID` | `assistant_get {id}` |
| 创建 | `ne assistant create --file PROFILE_JSON` | `assistant_create` |
| 更新部分字段 | `ne assistant update ID --revision N --file PATCH_JSON` | `assistant_update {id,revision,patch}` |
| 替换责任范围 | `ne assistant assign ID --revision N [--project ID] [--task ID]` | `assistant_responsibilities_set` |
| 归档/恢复 | `ne assistant archive/unarchive ID --revision N` | `assistant_archive {id,revision,archived}` |
| 查管理会话 | `ne assistant sessions ID [--scope all]` | `assistant_sessions {id,query}` |
| 查询已知会话的档案 | `ne assistant current --session SELF` | `assistant_for_session {session:SELF}` |
| 新开管理会话 | `ne assistant new ID [--task ID] [--message TEXT]` | `assistant_start` |
| 准备生成入口 | `ne assistant prepare ID --revision N` | `assistant_prepare {id,revision}` |

创建文件至少包含 name 和 agent，可选 avatar、description、instructions 和 runtime_override。更新文件只传要改的字段；runtime_override 省略保持原值，null 继承运行工具保存值，{} 使用 CLI 原生默认。归档保留资料和会话，不代表关闭运行终端。首次准备 NoEnding 技能后，用户的停用选择不会在每次启动时被强制覆盖。

分页跟随 next_cursor；写入重试沿用 request_id，更新先读 revision。角色指令由数据库维护，AGENTS.md 和 CLAUDE.md 是应用生成的加载入口；用户手改产生冲突时保留原文件并提示，不静默覆盖。个人笔记和资料由用户管理。
