# 查询与结果协议

将启动消息提供的 CLI 前缀记为 `<cli>`，其中已经包含 `--connection <路径> extraction`，使用完整可执行文件路径，不依赖 PATH 或短命令别名。此连接只允许本次提取查询，不提供通用 CLI 的任务组织或摘要操作。输出统一为 JSON（schema_version=1），成功 `ok=true`，失败 `ok=false` 并返回非零退出码；error 包含 code 和 message。连接失效、消息历史变化或摘要版本变化时按失败处理，不换成通用连接绕过校验。

- `<cli> inputs`：列出本次可查询的名称及记录数。
- `<cli> read --input <名称> --cursor 0 --limit 8`：分页读取原始记录。records 中每项包含 cursor 和 record；next_cursor 为下一页位置，null 表示结束，end=true 表示到达末尾。limit 为 1～50，输出过长时主动减小。
- `<cli> search --input <名称> --query <文字> --cursor 0 --limit 8`：按大小写敏感的子串搜索完整原始消息，跨 part 的文字也能匹配；返回匹配消息的组成记录及其 cursor。消息记录含 message_id、sequence、timestamp、role、part、last_part 和 content。用 read 展开前后讨论。搜索不计为完整阅读。

输入清单中的 input_file / history_input_file 是查询标识：取文件名作为 `--input`，这些输入不在磁盘上，而是按需查询数据库。结果路径是实际文件路径。只写指定结果文件。最终回复必须是下列状态格式的单个 JSON 对象，不写状态文件，不带 Markdown 代码围栏或其他文字。后端确认调用正常结束后，校验必需材料的分页读取进度、结果文件和并发状态，再统一入库。

单会话本次消息没有值得更新摘要的信息时正常跳过，由后端保留旧摘要。任务/项目只有在没有可摘要的实质内容且没有可沿用的有效摘要时正常跳过；保留旧摘要由后端处理。

摘要结果只包含 `{"summary":{"items":[{"title":"事项标题","description":"简短描述","details":["具体细则"]}]}}`，items 至少一个事项，会话摘要最多 15 个事项，任务和项目摘要最多 10 个事项（批量中的会话摘要也遵循 15 项要求）。写入前检查事项数量，接近上限时先合并真正相关的事项，清理重复、失效和次要内容，不将无关事项合并，不机械截断内容。title 为非空单行文字，description 为字符串，details 为字符串数组且每条细则非空；没有细则时使用空数组。未锁定事项可自由修改、合并、拆分、增删和排序，不输出事项 ID。输入的 locked_titles 列出当前目标受保护的标题（包括重新生成时）：每个标题必须原样保留，重复标题保留对应次数，不得通过改名、删除或合并使这些标题消失；其 description 和 details 仍可更新、融入相关信息，顺序可调整。锁定标题的保留优先于项数目标；不输出 title_locked 或其他锁定状态字段，后端按标题恢复用户的锁定设置。每次提交完整列表；失败或正常跳过不写对应摘要结果。结果应为 UTF-8 普通文件，不能使用符号链接。

最终回复状态 JSON 严格区分单会话和批量：

单会话成功：`{"status":"completed"}`。
单会话正常跳过：`{"status":"skipped","reason":"no_summary_content"}`。
任务/项目成功：`{"status":"completed","skipped_session_ids":[]}`。
任务/项目部分完成：`{"status":"partial","failed_sessions":[{"session_id":"待更新会话 ID","error":"具体原因"}],"skipped_session_ids":[]}`。
任务/项目正常跳过：`{"status":"skipped","reason":"no_summary_content","skipped_session_ids":[]}`。
整个摘要失败：`{"status":"failed","error":"具体原因"}`。

批量跳过和失败列表只包含 need_update=true 的会话，不列直接复用的会话、不列未知或重复 ID。
