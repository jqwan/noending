# 查询与结果协议

将启动消息提供的 CLI 前缀记为 `<cli>`，其中已经包含 `--connection <路径> extraction`，使用完整可执行文件路径，不依赖 PATH 或短命令别名。此连接只允许本次提取查询，不提供通用 CLI 的偏好编辑、任务组织或摘要操作。输出统一为 JSON（schema_version=1），成功 `ok=true`，失败 `ok=false` 并返回非零退出码；error 包含 code 和 message。连接失效、消息历史变化或摘要版本变化时按失败处理，不换成通用连接绕过校验。

- `<cli> inputs`：列出本次可查询的名称及记录数。
- `<cli> read --input <名称> --cursor 0 --limit 8`：分页读取原始记录。records 中每项包含 cursor 和 record；next_cursor 为下一页位置，null 表示结束，end=true 表示到达末尾。limit 为 1～50，输出过长时主动减小。
- `<cli> search --input <名称> --query <文字> --cursor 0 --limit 8`：按大小写敏感的子串搜索完整原始消息，跨 part 的文字也能匹配；返回匹配消息的组成记录及其 cursor。消息记录含 message_id、sequence、timestamp、role、part、last_part 和 content。用 read 展开前后讨论。搜索不计为完整阅读。

输入清单中的 input_file / preference_input_file 是查询标识：取文件名作为 `--input`，这些输入不在磁盘上，而是按需查询数据库。结果路径是实际文件路径。只写指定结果文件。最终回复必须是下列状态格式的单个 JSON 对象，不写状态文件，不带 Markdown 代码围栏或其他文字。后端确认调用正常结束后，校验必需材料的分页读取进度、结果文件和并发状态，再统一入库。

单会话本次消息没有值得更新摘要的信息时正常跳过，由后端保留旧摘要。任务/项目只有在没有可摘要的实质内容且没有可沿用的有效摘要时正常跳过；保留旧摘要由后端处理。

摘要结果只包含 `{"summary":"完整 Markdown 摘要"}`，正文非空；失败或正常跳过不写对应摘要结果。偏好结果独立为 `{"changes":[]}`，无变更也写空数组。结果应为 UTF-8 普通文件，不能使用符号链接。

偏好 change：
- 新增：`{"action":"add","scope":"user","content":"简明准则"}`。
- 修改：`{"action":"update","scope":"user","id":"已有 ID","content":"完整新内容"}`。
- 删除：`{"action":"delete","scope":"user","id":"已有 ID"}`。
- project 范围同时提供清单中的 project_id；user 范围不提供 project_id。不得改变条目范围、修改或删除 locked=true 条目、输出 locked 字段或自行生成新 ID。同一已有条目最多变更一次。

提交入库时以后端读取的当前锁定状态为准。用户在提取期间编辑或新增其他偏好不会使本次变更失效；目标条目已删除、已锁定或范围不合法时拒绝偏好变更，有效摘要仍可保存。

最终回复状态 JSON 严格区分单会话和批量：

单会话成功：`{"status":"completed","preferences":{"status":"completed"}}`。
单会话正常跳过：`{"status":"skipped","reason":"no_summary_content","preferences":{"status":"completed"}}`。
任务/项目成功：`{"status":"completed","skipped_session_ids":[],"preferences":{"status":"completed"}}`。
任务/项目部分完成：`{"status":"partial","failed_sessions":[{"session_id":"待更新会话 ID","error":"具体原因"}],"skipped_session_ids":[],"preferences":{"status":"completed"}}`。
任务/项目正常跳过：`{"status":"skipped","reason":"no_summary_content","skipped_session_ids":[],"preferences":{"status":"completed"}}`。
整个摘要失败：`{"status":"failed","error":"具体原因"}`。

偏好失败时将上述 preferences 替换为 `{"status":"failed","error":"具体原因"}`，不写偏好结果；有效摘要仍由后端保存。批量跳过和失败列表只包含 need_update=true 的会话，不列直接复用的会话、不列未知或重复 ID。
