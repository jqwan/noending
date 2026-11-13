# 摘要提交协议

自主提取和后台提取使用相同的提交内容。CLI 从 UTF-8 JSON 文件读取，MCP 直接传字段。后端核对目标范围、完整分页记录、历史代次、摘要版本和事项 ID，通过后保存并返回回执。

## 后台专用连接

使用启动消息给出的完整命令前缀，包含 `--connection PATH extraction`。此连接开放本次提取范围内的以下操作。

- `inputs` 返回目标、模式、输入名称、记录数和项目的 `pending_sessions`。
- `read --input NAME --cursor 0 --limit 8` 分页读取。每项包含 `cursor` 和 `record`，跟随 `next_cursor` 直到 `end=true`。
- `search --input NAME --query TEXT --cursor 0 --limit 8` 按大小写敏感的子串检索完整消息。搜索结果用于定位，完整阅读通过 `read`。
- `submit --file FILE` 提交一个结果或报告一个失败。以返回的 `data.status` 及版本为准，进程退出只表示调用结束。

输入名称是查询标识。长消息的 `part` 按顺序拼接到 `last_part=true`。工具输出截断时，从同一游标减少 `limit` 重新读取。

## 提交单个目标

```json
{
  "reply": {"status": "completed"},
  "results": {
    "session-会话ID": {
      "summary": {
        "items": [{"title": "已确认结论", "description": "简短说明", "details": []}]
      }
    }
  }
}
```

项目提取过程中，提交某个会话时额外提供顶层 `session_id`，`results` 只包含该会话。提交项目汇总时省略 `session_id`，`results` 只包含 `project-ID`。单独提取会话或协作摘要时同样省略 `session_id`，使用对应的 `session-ID` 或 `task-ID`。

- `{"status":"completed"}` 保存 `results` 中的完整摘要。
- `{"status":"skipped","reason":"no_summary_content"}` 确认本次材料已完整阅读，且无可更新内容。此时 `results={}`。
- `{"status":"failed","error":"具体阶段和简短原因"}` 报告失败，此时 `results={}`。项目中的会话失败时带上该会话 ID，继续处理其他会话。原因只写诊断所需的信息，避免引用消息正文、命令输出或凭据。待处理会话全部成功或正常跳过后，才可读取和提交项目汇总。

提交回执中的 `updated` 或 `skipped` 表示本次覆盖位置已保存，`no_change` 表示当前范围无需保存，`failed` 表示已报告的会话仍待处理。同一上下文中的相同成功提交可重取回执。冲突或过期时重新查询范围，保留已经入库的会话摘要。

摘要事项最多为会话 15 项、协作及项目各 10 项。标题使用非空单行文字，描述为字符串，细则为非空文字组成的数组，没有细则时使用空数组。每次提交完整列表。延续事项沿用 `item_identities` 中的 ID，新增事项省略 ID。改名或重排保留 ID，合并和拆分各保留一个延续 ID。提取只更新摘要。
