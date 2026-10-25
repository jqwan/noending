# 接入与写入规则

先读本技能目录中的 [access.json](access.json)，使用其中固定到当前 NoEnding Home 的绝对 CLI 路径。入口已带 Home，不重复追加 `--home`，不把技能 bin 当作项目目录，不修改 PATH。macOS 使用可执行脚本，Windows 按 `.cmd` 原生方式调用；参数按当前 shell 正确传递，正文不能拼成可执行代码。

独立分发且 access.json 的入口为空时，用已安装的 `ne` / `noending`，通过 `status` 核对 Home；也可用同一 Home 的 NoEnding MCP。MCP 的 Home 在连接期间固定，不能使用服务进程的目录或环境冒充客户端。应用移动或 Home 变更后，重新启动应用更新受管理入口。

数据查询、任务、信箱、同步与提取接口无需桌面持续运行；终端启动及终端身份查询需要同一 Home 的桌面应用。没有可用入口时说明依赖缺失，不直接打开 SQLite。`ne --help` 与各命令 `--help` 可核对实际参数，MCP 以当前暴露 schema 为准。

后续参考中的 ne 均替换为已确认入口，ID、TEXT、FILE、CURSOR 等为占位符。普通回忆通常按 scope=all 包含归档记录；列表及搜索跟随 next_cursor 分页并保持筛选条件，不从第一页推断全量。

CLI 返回 `{schema_version:1,ok:true,data:...}`；失败为 `{schema_version:1,ok:false,error:{code,message,...}}`，退出码非零。MCP 使用相同业务回包。读取不会隐含写入。

写操作可带 `--request-id` / MCP `request_id`，同一操作重试保持编号和全部参数，用 `request show ID` / `request_get {id}` 查询回执。请求超时先核对回执及业务状态，不换编号重复创建、追加或发信；失败后修正参数属于新操作。需要 revision 的修改先读当前版本，仅提交需改变的字段；冲突时重读融合，不覆盖其他会话的修改。
