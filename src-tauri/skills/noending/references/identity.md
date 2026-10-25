# 定位项目与当前会话

当前项目使用调用方实际绝对工作目录，通过 `ne project resolve --path PATH` / `project_resolve {path}` 定位；不要使用 MCP 服务自身的启动目录，不按项目名称猜测。未登记时保留未登记事实，不为查询自动注册。

## 当前会话身份

先核对访问入口的 Home，需要自身身份时按以下顺序获取：

1. 通过当前 Agent 的执行工具运行 `ne session identity`，使用返回的 `data.session_id`。无需自行读取环境变量或筛选终端列表。
2. 返回 `terminal_context_missing`，或只有 MCP 而无执行工具时，使用宿主明确提供的当前原生会话 ID，调用 `ne session resolve --agent AGENT --native-id NATIVE_ID` / `session_resolve {agent,native_id}`，使用返回的 `id`。该查询只做精确转换，不扫描、不自动同步；未命中先核对 Home。
3. 仍无法定位时，请用户提供“自己在 NoEnding 中对应的会话 ID”。使用 `ne session show ID` / `object_get {kind:"session",id:ID}` 核对记录存在后即可使用，无需再查原生 ID；不要将用户指定的收件会话 ID 当成自己。

`sender_unbound` 表示内嵌终端尚未绑定，等待摄入后重试；`desktop_unavailable` 表示同一 Home 的桌面控制不可用，`sender_unavailable` 表示来源终端已失效。这些错误不能通过手工指定其他会话来绕过。

`session identity` 必须由当前 Agent 自身执行，共享 MCP 服务的环境不能代表调用会话。宿主身份也必须明确属于当前调用方，例如 Codex 的 `CODEX_THREAD_ID`，或 zcode 当前上下文中的 `session_<id>`；不假定所有宿主都提供同样的信息。

身份仍未知时，只暂停发信、标记自身信箱、关联当前会话等依赖自身身份的操作，其他已授权工作继续。不按目录、标题、最近活动或对话相似度猜测，不扫描历史日志推断身份。
