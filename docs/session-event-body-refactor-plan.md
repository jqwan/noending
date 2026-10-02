# 会话事件、正文摄取与成员详情统一重构执行方案

日期：2026-10-02

状态：已确认执行方案。2026-10-02 用户授权先提交现有工作区，再分阶段实施。实施进度与验证追加在文末。

## 1. 目标与本次边界

将 NoEnding 的会话保存划分为三种职责：

1. 事件事实：全面摄取可识别的执行、交互、工具、协作、上下文与生命周期事实，供统计和追溯。
2. 正文内容：按分类与用户设置保存有阅读价值的内容，供浏览、搜索和 Context 提取。
3. 读取投影：明确哪个成员、哪个分支、哪个修订的正文当前有效；现有 Context 暂时继续使用根成员的可见对话。

NoEnding Session 仍是逻辑会话。根成员、子成员和辅助成员均是 SessionMember，使用统一的数据格式，拥有各自的详情与正文阅读入口。新增成员详情不创建另一种独立 Session，不改变 Workstream 所有权。

用户本轮已要求重新考虑旧有取舍。本方案不把此前对成本、压缩次数、缓存字段或格式版本的决定当成约束；是否展示一项统计，依据本方案中的数据可靠性和产品价值决定。

本次交付包括：统一事件、可配置正文摄取、八种适配器、成员页面、统计换源、Context 的最小读取适配、搜索适配、重摄和验证。以下不进入首轮交付：

- 全部 Agent 历史来源扩张，例如 Codex 已删除 rollout 的 SQLite 历史恢复。
- 宣称能够完整还原所有 Agent 的每次 API 请求。
- 根据过程记录推断任务成功率、产物质量或人工纠错率。
- 跨所有逻辑会话的模糊去重和统一货币估算。
- 图片/音频/备份文件的大规模复制、完整日志归档、远程资源下载。
- 新增成员级独立 Context、Workstream 所有权、回收站或通用 Resume 能力。
- 多成员 Context 提取策略、专用 Context 输入版本、输入快照与新的提交校验机制。这些与后续 Context 提取重构一起设计，不作为本轮前置工作。

“事件全面摄取”是归一化事实覆盖，不是把所有 JSON 行原样复制进数据库。流式分片组装为事实；镜像合并；纯配置更新保存为配置事实或成员状态；不透明密文只保存必要的来源标识，不作为正文。

## 2. 现状与影响范围

### 2.1 已确认的代码约束

| 区域 | 当前行为 | 必须调整的内容 |
| --- | --- | --- |
| `domain/models.rs` | SessionMember 已有 root/child/side；SessionMessage 仅 user/assistant 且限定 root | 增加事件、正文分类、来源、关联、正文策略和覆盖状态类型 |
| `adapters/mod.rs` | ParsedLine 最多一条正文，活动通过 MemberObservation 聚合，另有 UsageEvent | 改为一条源记录可生成多个事件与多个正文候选 |
| 八个适配器 | child/side 一般只发出统计观察 | 所有成员产出相同事件与正文候选，按实际来源分类 |
| `storage/schema.rs` | 独立数量快照、usage_events、仅根正文与 Session 级投影 | 统一事件、成员正文、关联、成员投影、正文策略覆盖状态 |
| `storage/mod.rs` | 写入检查 root-only；用量认领与提交不完全在同一事务 | 新统一提交，认领、事实、正文、投影、游标原子提交 |
| `storage/context_repo.rs` | 根正文投影和派生 turn_final；Session 级消息边界 | 增加根可见对话读取适配，保留现有边界和提交检查 |
| `context/mod.rs` | 摘要 CAS 依赖 generation、序号与正文前缀 | 本轮保持提取范围与 CAS，不增加多成员输入版本 |
| `sync/extractor.rs` | PromptSession 使用二元角色的 SessionMessage | 从新正文结构生成现有根对话 DTO，不重写提取流程 |
| `search/mod.rs` | 仅索引根正文；按 Session 投影与回收站过滤 | 索引已保存且当前有效的成员正文，命中可定位成员 |
| commands/API/types | SessionDetail、消息分页和导航标记均为 Session 范围 | 增加 MemberDetail、事件和正文查询；校验所属 Session |
| 前端路由和会话页面 | 逻辑会话详情 + 根正文整屏阅读 | 新增成员详情和成员正文入口，复用阅读组件 |
| Settings | 有来源、后台补摄等设置，没有正文分类策略 | 增加正文摄取开关、历史补摄与覆盖状态 |

设计参考：

- `docs/agent-session-source-breakdown.md`：字段事实和源格式证据。
- `docs/agent-activity-usage-unified-ledger.md`：统一事件方向；调用最大值去重和活动 MAX 等规则不直接采用。
- `docs/agent-session-ingestion-review.md`：历史摄取审计；其中缺陷必须与当前代码重新核对，不能一概视为未修复。
- `docs/usage-events-refactor-2026-10-01.md`、`docs/usage-source-audit-2026-10-01.md`：历史用量核查基线。

### 2.2 不改变的领域边界

- Session 是逻辑会话，SessionMember 是内部执行成员。
- fork 在有原生证据时建立新的逻辑 Session；child/side 属于原逻辑 Session。
- 项目、WorkspacePath、Owner Workstream、逻辑会话生命周期仍由现有模块管理。
- Context 显式更新仍只通过当前服务调用模型；摄取、补摄、浏览和搜索不会自动调用模型。
- 适配器解释原生字段，上层不自行猜测 Agent 的目录或格式。
- Agent 源文件始终只读；临时回归样本、NoEnding 数据库和测试输出独立保存。

## 3. 正文摄取策略

### 3.1 推荐的默认分类

所有 root/child/side 共用下表。判断依据是语义，不是源 role；父成员交给子成员的 user 消息是委派正文，不是真人输入。

| 正文分类 | 默认 | 用户可切换 | 默认保存内容 | 后续 Context 提取用途 |
| --- | --- | --- | --- | --- |
| 真人输入 | 开 | 是 | 实际要求、补充、回答 | 意图与约束 |
| 代理可见文本 | 开 | 是 | 过程说明、可见结果、最终答复 | 决策、结论、未完成工作 |
| 协作任务与结果 | 开 | 是 | 委派要求、明确的报告与成员间有效消息 | 分工、返回结果 |
| 压缩/分支摘要 | 开 | 是 | 源提供的可读摘要和覆盖引用 | 历史不足时补充，不与已覆盖原文重复加入 |
| 人工决定与明确错误 | 开 | 是 | 审批问题、决定、错误说明 | 授权边界与失败原因 |
| 工具参数 | 关 | 是 | 实际命令、查询、路径、补丁等参数 | 用户开启后可用作操作证据 |
| 工具结果 | 关 | 是 | 源中返回给模型的输出、diff、验证与失败结果 | 用户开启后按相关性和预算选取 |
| 注入上下文 | 关 | 是 | 实际注入的指令、记忆、文件片段、系统/开发者内容、工具定义 | 理解请求输入；默认不把整套提示词放进任务摘要 |
| 可读推理与推理摘要 | 关 | 是 | 源确实提供的明文或可读摘要 | 可浏览；任务 Context 默认排除 |

默认模式有意不保存大量工具流量、重复注入和推理。默认模式足以读取主要对话、成员任务与报告，但不宣称具有完整执行证据或完整模型输入。详细的工具证据需要开启工具结果。

上表 Context 列描述内容为后续提取重构提供的价值，不代表本轮扩大 Context 输入。本轮现有 Context 只读取根成员当前分支的真人输入与代理可见文本。

图像/文件等附件：随已保存正文保留必要的名称、类型、原始引用与可用状态，不默认复制二进制内容。仅附件消息不能因为文本为空而丢失。

全文代码备份、完整溢出日志、密文、签名、重复侧车和流式临时分片不设为普通正文开关；先保留引用或组装结果。本轮不扩张为附件归档系统。

分类发生重叠时拆分内容，不保存重复全文：例如工具结果事件同时有错误，工具结果关闭时只保存源提供的错误说明，不能以“错误”为名复制整段输出。协作报告中的工具日志同理。

### 3.2 设置界面

入口：设置 → 数据与高级 → 会话正文摄取。

首版提供“基础正文（默认）”“仅统计”“自定义”三种状态。选择仅统计会关闭全部正文分类，但仍摄取事件、结构、用量和内容可用性元信息。用户修改任一分类后显示自定义。

正文设置全局作用于当前 NoEnding 数据库中的所有已启用来源与所有成员。首版不做每 Agent、每项目、每成员覆盖，避免形成复杂继承规则。

设置保存为有版本的 JSON；建议字段：

```json
{
  "version": 1,
  "revision": 1,
  "categories": {
    "human_input": true,
    "assistant_text": true,
    "coordination": true,
    "context_summary": true,
    "control_and_error": true,
    "tool_input": false,
    "tool_output": false,
    "context_injection": false,
    "reasoning": false
  }
}
```

设置影响存储边界，不影响事件解析。只在 UI 隐藏内容不算关闭摄取；正文关闭时，也不能把正文放进事件 payload、raw_ref、调试日志、FTS 或关联表。

### 3.3 设置变化与历史数据

| 操作 | 行为 |
| --- | --- |
| 开启分类 | 从下一次任务开始保存；界面提示历史尚未补齐 |
| 补摄历史 | 用户选择来源或会话后，后台重读可用源，只补选中正文和关联；事件统计不能增加 |
| 关闭分类 | 停止后续保存该类正文，已保存内容仍保留 |
| 清理已保存分类 | 独立入口，先显示分类、影响范围和已保存规模；执行时删除正文与相关索引/链接，保留事件和统计 |
| 源文件缺失 | 已保存内容保留；补摄显示源不可用，不伪造成功 |
| 流式记录尚未结束 | 标为部分内容；后续可用更新仅在当前策略允许时保存 |

不自动全量回读所有来源，不因为关闭开关而静默删除历史正文。

补摄任务捕获策略 revision，按成员串行或互斥提交，防止与普通增量摄取竞争。同一任务不能中途混用新旧开关。用户改变策略后取消/重排相关正文任务；已提交批次按当时策略记录。

### 3.4 覆盖状态

页面区分：已保存、部分保存、未开启、未补齐、源不提供、源已丢失、被清理。

事件保留轻量 body manifest：候选正文的稳定键、分类、来源位置和源完整性，不含文本。这使“无正文”和“正文没保存”能够区分。

成员/分类覆盖表只保存最后完成的重读范围、source generation、策略 revision、任务状态与缺口原因；已保存量和缺失量优先从 manifest 与正文推导，不重新维护一套累计活动统计。

无法确认源覆盖的 Agent 只报告已观察范围，不标记全部历史完整。已关闭类别的旧正文仍可浏览；若其源内容继续变化但不再保存，应标记正文版本落后或部分保存。

## 4. 数据契约与表结构

### 4.1 核心结构

| 结构 | 职责 |
| --- | --- |
| `sessions` | 逻辑会话及既有归属和生命周期 |
| `session_members` | root/child/side、来源身份、父关系、成员展示元信息 |
| `session_events` | 归一化事件与可统计事实 |
| `session_messages` | 按策略保存的正文单元及修订 |
| `session_event_message_links` | 主归属之外的输入、输出、证据、摘要等关联 |
| `session_message_projection` | 成员当前有效分支与正文修订顺序 |
| `session_member_cursors` | 每来源的字节/记录前沿、文件身份、模型解析状态 |
| `session_ingest_state` | 暂时保留现有根对话的 generation/消息边界，供现有 Context 使用 |
| `member_body_coverage` | 分类补摄进度与可用性，不存累计统计 |
| 事件汇总视图 | 成员、逻辑会话和统计维度的数量/用量/结果汇总 |

`session_member_stats`、`usage_events` 及旧 StatsDelta/StatsSnapshot 最终删除。切换开发中允许暂留未启用的新旧类型，但发布结构不双写统计，不保留老格式读取分支。

### 4.2 SessionEvent

共用字段：

| 字段 | 规则 |
| --- | --- |
| id | NoEnding 主键，重读和内容补摄不改 ID |
| member_id | 实际观察成员，FK；通过成员读取 session_id/agent |
| event_key | 成员内归一化事实键，NOT NULL，唯一 |
| event_type | 闭合类型集；未知源类型产生轻量诊断，不塞进任意类型 |
| occurred_at | UTC 时间，未知为 NULL |
| source_order | 来源顺序；跨成员只作阅读排序，不作严格因果证明 |
| turn_key/request_key | 经过作用域限定的原生/可靠重建身份；未知为 NULL |
| parent_event_id | 已解析的因果事件；可晚到后补，禁止跨错误逻辑会话链接 |
| target_member_id | 协作目标；未解析时保留原生标识，不造成员 |
| origin | human/model/tool/runtime/peer/unknown |
| purpose | task_execution/context_maintenance/title_generation/verification/search/internal/unknown |
| status | pending/running/completed/failed/cancelled/unknown |
| execution_origin_key | 有证据的原始执行身份，用于识别继承/镜像；禁止拿普通内容哈希冒充 |
| provenance_kind | native/inherited/mirror/unknown，说明观察与原始执行的关系 |
| source_refs | 文件、代、记录位置、表/主键、块路径及修订证据的定位，不含正文 |
| body_manifest | 可保存正文的身份、分类与来源完整性，不含正文 |
| payload | 经类型校验的小结构，仅保留有用途的原生细节 |
| parser_version | 解析规则版本 |

经常查询的模型、provider、用量、请求数与质量标记使用显式列。工具名、结果状态等先使用受约束 payload；真实查询需要时提升为列或生成索引，不提前给所有事件铺大量稀疏字段。

模型用量列：input_tokens（归一化总输入）、cache_read_tokens、cache_write_tokens、output_tokens（总输出）、reasoning_tokens、requests、usage_granularity、usage_completeness、request_count_basis、model、provider、model_evidence。

非模型事件不贡献模型请求；未知请求数量不得默认 1。源提供真实一次请求时为 1，汇总记录使用源给出的数量。未知 Token 为 NULL，已证实的零为 0。推理是输出子集时不加到总量。

起止、耗时、首 Token 延迟、结束原因、错误类型、重试关联进入类型化 payload；高频统计确定后再提升必要字段。使用原生耗时优先，计算耗时必须保存依据。

源提供的积分/货币/估算计量可作为带单位和来源的扩展属性，不进行货币合并，不自动接入价格拉取链路。

### 4.3 事件词表和计数对象

| 家族 | 首轮支持的语义事实 | 计数对象 |
| --- | --- | --- |
| message | 真人输入、逻辑代理消息、协同消息、明确诊断 | 逻辑消息身份，不是内容块/正文行数 |
| model | 单次请求、仅汇总可用时的 usage_summary | 实际请求或源汇总数量，不是 usage 报告条数 |
| tool | call、result | 调用数只计唯一 call；结果用于状态与耗时 |
| collaboration | 成员创建、委派、投递、返回 | 成员、任务、通信分别计数 |
| context | injection、compaction、prune、restore | 独立上下文动作；摘要不伪装成人类输入 |
| control | approval、interrupt、pause/resume | 人工控制对象与决定 |
| artifact | 确认的文件/产物变化、提交、验证结果 | 有执行结果证据的产物，不根据回复文字猜 |
| lifecycle | turn、session 生命周期 | 原生执行轮和动作；结束不等于任务成功 |
| configuration | 模型/模式/权限等配置改变 | 背景事件，通常不做总览数字 |

同一调用状态从进行中更新为完成时，更新请求/调用事实；不会新增一条“同调用请求”。调用结果可另有结果事件与正文，但两者共享调用身份。多个不同重试必须有不同执行身份。

每个适配器提交能力矩阵：原生支持、可靠重建、部分支持、不支持。未知状态不能当失败；用户取消单独统计。模型 tool_use 结束属于正常请求结束，不是失败或用户任务完成。

### 4.4 正文与修订

字段：id、member_id、primary_event_id、message_key、message_group_key、revision、kind、role、author_kind、phase、content、source_refs、content_basis、completeness、source_visibility、content_hash。

- primary_event_id 必填；数据库/提交检查正文和事件成员归属。
- 同一原生消息可拆成不同语义正文单元；同一单元更新形成新修订，旧修订不参与当前阅读/搜索。
- content 是有序文本、JSON、代码和附件引用块，纯文本作为搜索/提取投影，不另建第二份正文事实。
- role 是原生模型通道；author_kind 才决定是否真人。两者不能合并。
- phase 优先使用原生 final/commentary；没有时为 unknown。旧“下一条不是 assistant 即最终答复”只可作为显示启发，不进入最终答复统计。
- 原生模型/provider 统一由事件提供；正文 DTO 通过明确请求关联读取，不再从配置猜测生成来源。
- 正文候选未被保存时只有 manifest，不创建空正文占位行；附件消息有结构化内容，不算空行。

### 4.5 关联表

字段：event_id、message_id、relation、ordinal、block_path、basis、coverage。

relation 支持 request_input/request_output/tool_input/tool_output/summary/evidence/same_turn。basis 为 native_id/request_snapshot/runtime_rule/turn_association；弱关联不升级成实际请求输入。主归属从 primary_event_id 读取，不在关联表重复保存。

索引至少包括：events(member_id,event_key) UNIQUE、events(member_id,occurred_at)、request/turn 键、messages(primary_event_id)、messages(member_id,message_key,revision) UNIQUE、links(event_id,relation)、links(message_id)、成员投影(member_id,ordinal)。关联唯一约束中可选路径使用非空默认值，避免 SQLite NULL 导致幂等失效。

正文删除时关联自动清理，事件不被删除；成员永久删除时其事件、正文、覆盖与投影一致清理。跨成员关联必须先解除，不能连带删除其他成员事实。

## 5. 摄取与提交流程

### 5.1 新适配器输出契约

替换“单消息 + 聚合观察 + 用量”的主通路：

```text
MemberIngestBatch
  source_snapshot / source_generation / cursor_frontier
  normalized_events[]
  body_candidates[]
  event_body_links[]
  projection_operations[]
  provenance_state
  coverage / diagnostics
  complete_snapshot
```

body_candidates 是临时解析结果或可按源位置读取的候选，不能未经正文策略进入数据库。关闭分类时仍要读取源并解析身份、状态、用量和内容类别，但可以跳过昂贵的正文组装。

策略在任务开始时固定，提交前检查其 revision；使用旧策略的任务被取消/重排时不能继续写入新禁用类别。导出日志只打印数量、来源与错误码，不打印完整候选正文。

### 5.2 单成员事务

1. 核对逻辑会话状态、成员归属、来源快照与策略 revision。
2. 规范并合并事件身份、镜像和必要的用量归属。
3. upsert 事件，保留稳定 ID；按原生修订规则更新半成品，拒绝无证据覆盖完整值。
4. 策略过滤正文候选；幂等插入内容修订。
5. 建立可确认的关联，延后解析引用放入待解析元信息。
6. 更新成员分支投影、正文覆盖与 FTS；仅根可见对话变化时，按现有规则更新 Session 的根对话边界。
7. 最后推进来源游标与模型解析状态。
8. 一起提交；失败全部回滚。

首轮不在 parse closure 中写数据库。现 ingest_usage_claims 的认领逻辑移动到此事务；最终是否复用该表名，在统一身份契约冻结时决定。墓碑/删除语义先有回归，再删除旧认领机制。

来源不完整时，不清空旧投影、不宣称历史补齐、不越过不完整帧。SQLite 读取使用 mode=ro 和一致的读事务；活 WAL 库不通过 immutable=1 隐藏 WAL。仅对已冻结副本使用不可变模式。

### 5.3 重读、重写与重复

- 全量重读、增量重读、正文补摄都以稳定键收敛；补摄不更换事件 ID，不增加请求或活动。
- 同一事实的 response_item/item_completed 或多份 usage 归并为一个事实，多位置保留 source_refs。
- 内容相同的两次用户输入、两次工具调用、两次重试都保留，不能按文本去重。
- 文件替换、原地改写、分支变更、dsh 换代由适配器提交明确替代关系；不能把 generation 变化当作新增执行。
- 原生压缩导致历史正文不再出现，不自动否定过去的真实模型请求；正文当前投影与执行事实有效性分开。
- 没有可靠换代身份时记录覆盖不确定与诊断，禁止同时把两代镜像都当新增执行；P0 样例必须给每家确定可验证的规则。
- 已确认继承/镜像不计作子成员的新执行；事件与正文仍可用于查看来源历史。
- 首轮不拿累计 Token 元组或短消息 ID 做全局跨 Session 唯一键。跨逻辑会话的实际执行去重另需原生 fork/继承证据。

## 6. 八种适配器工作包

每家都交付：来源权限与快照规则、事件身份、轮/请求/工具关联、事件能力表、正文分类、更新/重放规则、脱敏 fixture、全量/增量/补摄一致性测试。

### 6.1 Codex

- 新用量优先 token_usage_record 的 response_id 和 usage；旧 token_count 回退，核验覆盖对应，不能双记。
- turn_context、thread settings 保留模型状态与证据；区别声明配置与实际响应证明。
- response_item 与 item_completed 选择权威记录或按原生身份归并，不同时计消息/工具。
- task_started/task_complete/turn_aborted 提供轮状态、耗时和 TTFT。
- commentary/final_answer 保留原生阶段；用户机器信封依据字段/结构拆解，不能按 # 或 < 前缀一律丢弃。
- 工具参数/结果、可读 reasoning summary、协作正文、压缩摘要和输入附件分别受正文策略控制。
- 无具体请求-正文连接证据时只标 same_turn，不制造完整请求输入。
- 测试：两种 usage 重叠、原生 ID 缺失、继承副本、纯工具请求、Markdown 真人输入、缺失源文件。

### 6.2 Claude Code

- 分离源 uuid、逻辑 message.id、promptId、工具调用 ID。
- 主转录重复用量与辅转录递增用量分别归一化；续片请求数为 0 的贡献不能被存储强制为 1。
- 所有成员输出正文候选；isSidechain 表达来源/关系，不把所有子消息计为协同。
- attachment.rendered 作为注入正文；边界、摘要、队友投递分别归类。
- 实际 provider 未提供时保持未知，不能以原厂名充当调用渠道。
- 工具参数、结果、可读推理按策略保存；queue-operation 与实际投递去重，排队不等于已送入模型。
- 测试：多工具同请求、流式截断续写、主/辅差异、fork 历史、摘要标记、注入与真人同信封。

### 6.3 Pi

- entry id/parentId 维护树；活动 leaf 和 compaction 的 firstKeptEntryId/retainedTail 控制当前分支。
- responseId 有则使用；缺失时采用明确的成员内事实身份，不推断跨文件全局相同。
- message 的 user/assistant/toolResult/bashExecution 分别归类；一个 assistant 内多个 toolCall 不取 MAX。
- compaction/branch_summary 的摘要与 usage 独立摄取；custom_message 与 custom 的模型可见性区分。
- stopReason、errorMessage、isError、excludeFromContext 保留。
- 测试：多分支、压缩保留尾部、纯工具与空响应、失败/取消、同内容重复、图片附件。

### 6.4 Qoder

- uuid、message.id、promptId、requestSetId、请求 anchor 和工具 ID 分清粒度。
- humanInput/origin 认真人；user/tool_result、任务通知、注入和 compact summary 单独分类。
- Token 占位零记录为不可用；源积分保持独立计量，不伪装 Token/货币。
- model code 原样保留；显示映射不改变模型事实，不能依靠过期内置映射宣称实际型号。
- 工具状态与输出来自 content/toolUseResult，避免两份结果重复；active-leaf 驱动投影。
- 测试：同消息多块、仅一行 usage、工具回灌、源 Token 不可用、active-leaf、子成员身份。

### 6.5 WorkBuddy

- 所有成员使用相同格式；根/子身份沿用当前已核验目录与字段规则。
- providerData.usage 为权威，message.usage/rawUsage 只回退/核验；保留 source requests。
- providerData 中 messageId/conversationRequestId 只在核验其语义与唯一性后作关联。
- user_query 与注入信封拆分；压缩/meta/summary 用机器标记优先。
- function_call 与 function_call_result 通过 callId 连接；reasoning 读取 rawContent，不读空 content。
- auto 作为未解析模型选择，不从会话偏好猜实际模型；未知 provider 保持空。
- 测试：三份 usage、关闭正文仍有用量、子正文、reasoning rawContent、错误、工具结果溢出引用。

### 6.6 dsh

- canonical 代选取、seq 作用域、sourceEventSeqs、surfaceOp 替代关系先确定。
- source.kind 区分真人、注入、relay；turn/step/attempt 区分执行身份。
- 请求 header 保留配置、system 与工具定义的候选；不能只凭 header 推断完整输入。
- assistant message、summary、attempt 内流式 usage、独立检索调用按实际结构核验，不把 attempt 标记直接计为请求。
- subagent catalog、协作报告、审批、工具错误和压缩/裁剪事件接入统一词表。
- 测试：v0/v2/v4、zstd 不完整尾帧、replace、裁剪范围、relay、重试身份与非线性覆盖。

### 6.7 ZCode

- model_usage 作为请求事实；turn_usage 只作校验或覆盖明确的回退。
- 用 assistant_message_id/parent_user_message_id/turn_id 建立原生关联；取消/失败可没有回复正文。
- 模型/provider 使用请求记录；保留 query_source、variant、attempt、retry 和错误归因。
- message/part 在流式期间原地改写，按主键 upsert 与完成标记形成正文修订。
- part.text 是正文权威，不重复摄取 metadata.inputIntent.text。
- semantics 控制真人/提醒/通知/摘要；tool part 的 input/output 与状态分离。
- 测试：大/小写 ID 字段、原地更新、未完成快照、失败无正文、原生 join、汇总校验、task_type。

### 6.8 Antigravity

- gen_metadata 的 ModelUsageStats 为用量权威，steps 镜像不双记；半成品不能抢占完整请求身份。
- response_id、last_step_index、step 身份区分；一个生成与多个 steps 的关联标记具体证据。
- 以已核验 protobuf 字段取得模型/provider、输出、思考、输入、缓存和状态，不以字段数字猜语义。
- brain 规范转录与 steps 互补，按 step/消息身份归并；full/transcript/chunks 不重复保存。
- 工具、错误、压缩与父子关系从各自权威载体读取；不同库快照不完整时允许迟到补关联。
- 测试：零用量半成品、重复 responseId、body 仅在 brain、纯工具步骤、跨库迟到与源缺失。

## 7. 逻辑会话与成员详情页面

### 7.1 页面模型

| 页面 | 展示范围 | 可执行操作 |
| --- | --- | --- |
| 逻辑会话详情 | 全体成员汇总、主对话预览、成员结构、逻辑会话 Context、Owner | 既有 Resume/Owner/回收站/Context 更新 |
| 根成员详情 | 根成员自身事件、正文、来源、覆盖、统计 | 来源定位、正文阅读、历史补摄 |
| 子成员详情 | 子成员自身事件、正文、父/根链接、下级成员、统计 | 来源定位、正文阅读、历史补摄 |
| 辅助成员详情 | 辅助成员自身事件、正文、所属关系、统计 | 来源定位、正文阅读、历史补摄 |

成员页面不继承逻辑会话操作按钮。只有适配器明确支持时，未来才讨论成员 Resume；首版不展示可用但无法工作的按钮。

### 7.2 路由与组件

新增 Route：

```ts
{ view: "session_member"; sessionId: string; memberId: string;
  entry?: "conversation" | "events"; messageId?: string; eventId?: string }
```

保留现有 session/conversation 入口作为主对话快捷入口，内部解析 root member；这只是调用入口复用，不是旧数据库兼容分支。

- SessionDetailView 保留逻辑会话视角，成员列表改为可点击；不在同一列表混排所有成员正文。
- 新 SessionMemberDetailView 复用统计、来源和覆盖组件，显示面包屑“逻辑会话 → 根/父成员 → 当前成员”。
- 复用 SessionConversationView 的滚动与分页实现，抽出 MemberConversationReader，参数为 sessionId/memberId。
- 正文行按类别显示来源与角色，工具/注入/推理折叠，支持“查看相关事件”。
- 事件阅读器使用分页和类型过滤；支持定位已保存正文，不把所有 payload 一次拉到浏览器。
- 只有事件、没有正文时仍有可用详情页；提示具体原因和设置/补摄入口。
- Search 和统计排行跳转携带 memberId/messageId，不能把子正文跳回根阅读页。
- 分页键包含投影 generation 与 ordinal；投影变化时重置过期分页，保留可重新定位的消息键。

### 7.3 分支、生命周期与缺失源

- root/child/side 都读各自当前有效分支，旧修订另作来源审计。
- 子/辅不提升为独立逻辑 Session；独立 fork 仍按来源证据创建逻辑 Session。
- 逻辑会话进回收站后成员沿用其访问/搜索过滤；不能经成员路由绕开生命周期规则。
- 源缺失时可读本地已保存正文与事件，补摄不可用；不删现有数据。
- 未能归属的子源继续进摄入诊断，不用孤立子源伪造逻辑会话。根出现后重试归属和摄取。

## 8. API 与查询改造

### 8.1 API 合同

新增/调整命令：

- get_session_detail(sessionId)：逻辑会话摘要、全体成员与 aggregateStats。
- get_session_member_detail(sessionId,memberId)：memberStats、关系、来源、正文覆盖、能力。
- get_member_messages(sessionId,memberId,before/after,limit,categoryFilter,generation)。
- get_member_message_marks：真人输入或委派输入的导航标记，不把注入当用户刻度。
- get_member_events：类型/时间/状态过滤与分页。
- get_message_events：主事件和相关事件，附关联依据。
- get_event_messages：已保存正文、候选但未保存的分类状态，不返回未授权存储的文本。
- get/set_content_ingestion_policy。
- backfill_member_bodies/backfill_source_bodies：仅选中类别与范围，返回任务 ID、进度和不可补齐原因。
- preview/purge_saved_bodies：分类和范围明确，事件不变；若清理影响根可见对话，则按现有 generation 规则处理 Context 边界。

所有 sessionId/memberId 对均做归属校验；memberId 不能仅靠调用者声称属于某 Session。有限分页，禁止一次返回全工具日志或全部成员正文。

类型共享定义在 domain/models.rs、src/types.ts；命令注册更新 lib.rs/commands。前端 api.ts 不通过任意 JSON 绕过类型。

### 8.2 统计口径

| 统计 | 规则 |
| --- | --- |
| 用户输入 | 真正 human 事件；委派、注入、摘要不计 |
| 用户轮 | 明确的 turn 对象；推断/不可用标识保留 |
| 模型请求 | 唯一请求或源报告汇总，不对流式续片加 1 |
| 工具调用 | 唯一 call；result、镜像不重复 |
| 辅成员 | 成员结构，不按协作消息数推导 |
| 委派/通信 | 任务与投递分别计；不按 child 消息数量推导 |
| 失败率 | 已知 completed/failed 请求或工具作为可解释分母，cancelled/unknown 分别显示 |
| 时长 | 源边界可靠时计算；会话跨度和执行时长区分 |
| 缓存命中 | SUM(cache_read)/SUM(input)，不是各行百分比平均；覆盖不足要标明 |
| Token | 同覆盖范围只计权威明细或回退汇总；推理不重复加到输出 |
| 按模型 | 只统计有明确模型关联的执行；弱关联活动保留“未归属”，不能猜模型 |

数量、用量、时间各按自身覆盖选择来源，不能用一个通用 MAX 或“Token 最大源行”代表所有事实。

正常统计不受正文开关、历史补摄或正文清理影响。成员统计和逻辑会话汇总先在每成员汇总层关联，避免事件/正文/链接多对多 JOIN 放大。

总览首轮建议显示逻辑会话、用户轮、模型请求、工具调用、执行时间、总 Token、缓存命中。失败/重试、协作、压缩/上下文、产物与人工控制作为详细分析；来源覆盖不足的项显示未知或部分。

首次全量对账记录所有预期差异：旧统计把注入计真人、请求默认 1、漏读请求明细等修正可能使数字变化；每个差异必须对应源事实，而不是要求新旧错误值相同。

## 9. Context 与搜索

### 9.1 本轮只适配现有根对话读取

后续 Context 提取逻辑将单独重构，本轮不实现 ContextInputBuilder、多成员输入选择、专用 input_revision、快照存储或新的 CAS。

新增一个根可见对话读取适配器，从成员当前投影中选择根成员的真人输入与代理可见文本，转换为现有提取器使用的 user/assistant DTO。保留逻辑消息分组、阅读顺序和现有 generation/消息边界，不把工具、注入、摘要、推理或子/辅正文强行塞进二元角色。

这个适配器读取新格式的正文，属于现有提取服务的正常读取合同，不是旧数据库兼容分支。现有根对话分页入口可复用同一读取模型。

最低验收：

- 新存储接入后，现有根对话 Context 提取可以继续工作。
- 根可见对话的改写、补摄、清理使用现有投影 generation 与提交检查，不另造一套输入版本。
- 子/辅正文的摄取、补摄和清理不推进本轮根对话 Context 的 pending 状态；成员详情仍正常可读。
- 仅事件更新、可选工具/注入/推理正文变化也不影响现有提取范围。
- 仅统计模式下保留已有 Context，新增更新提示无可用正文，不从事件数量制造摘要。
- 已有 Context 和人工 context_items 不被摄取自动重写；摄取与补摄不调用 AI。

### 9.2 为后续提取重构保留基础

本轮只提供稳定的事件/正文 ID、修订、成员归属、当前分支、内容类别、来源和有证据的关联。后续可据此构建多成员 Context 输入，无须重新定义摄取格式。

以下决策延后：子/辅内容选取、父报告与子报告去重、工具证据预算、摘要历史覆盖、多成员新鲜度、输入快照及新增/改写期间的提交策略。不把其实现或测试作为本轮发布阻塞条件。

### 9.3 搜索

- 只索引已保存、当前有效的正文修订；事件原始载荷不进入全文索引。
- 默认搜索真人、代理文本、协作报告、摘要与控制/错误正文。
- 工具、注入、推理可作为扩展搜索分类，不因“已开启摄取”自动污染默认结果。
- SearchHit 带 sessionId/memberId/messageId/category；命中落到对应成员并高亮正文。
- 回收站、旧投影、旧修订、已清理正文同时从 FTS 与 LIKE 回退排除。
- 索引删除、重建与正文事务保持一致；查询额外做当前投影/生命周期防护。

## 10. 分阶段实施与验收

这是一个跨领域重构，不建议同时在八个适配器和所有页面上直接替换。采用准备工作包 + 可验证的纵向链路，再批量扩展。每阶段有独立验收；发布切换必须等完整链路就绪。

### P0：冻结合同、基线与恢复方案

交付：本方案定稿、事件词表、正文开关、统计公式、能力矩阵、脱敏 fixture 和审计清单。

任务：

- 核对当前 HEAD，区分历史文档缺陷和已修复实现；保留现有未提交工作。
- 记录八种来源的成员结构、事件数、请求/Token、已有正文、增量行为和默认模式存储规模。
- 为身份、流式、镜像、分支、换代、用户输入混合信封等建立最小回归样例。
- 列出所有 SessionMessage/Stats/usage 查询调用者及测试，锁定改造清单。
- 明确本地库中哪些数据可从源再生，哪些是人工维护、不能删库恢复的资料。
- 设计完整备份与恢复验收，不在用户工作库上开发或审计。

验收：每家有可运行样例；字段与覆盖有确定合同；没有以“暂时全塞 raw JSON”代替分类。

### P1：领域类型与公共归一化器

依赖 P0。交付：SessionEvent、BodyCandidate、BodyPolicy、关联、能力与覆盖类型；正文分类器和模型用量归一化规则。

- 确定 event/message/request/tool 身份作用域和单元测试。
- 实现一源多事件/多正文候选，兼顾文本、JSON 与附件引用。
- 分开作者与 role、用途与成员关系、请求状态与任务结果。
- 定义流式/upsert/最终值合并规则及数据不确定性。

验收：工具数量不取调用组 MAX；请求默认值不造 1；未知与零区分；关闭正文不改变事件输出。

### P2：存储、原子提交与统计底座

依赖 P1。交付：目标 schema、统一提交、成员正文投影、关联查询、事件汇总、覆盖状态。

- 数据格式采用仓库 AGENTS.md 规则，在最终格式切换时提升 DATABASE_FORMAT_VERSION；当前为 1，发布号依实施时实际 HEAD 决定。
- 只改 schema.rs，不增加 migration，不保留旧库兼容读取。
- 把认领和游标移到统一事务，支持迟到关联与稳定 upsert。
- 取消正文 root-only 限制，按成员维护当前分支和修订。
- 写数据库级 FK/幂等/回滚/清理/生命周期测试。

验收：正文能找到主事件；反向关联可查；事务故障不前移游标；重读和补摄不会增加统计。

中间 schema 仅用于隔离开发库。此阶段不能让未适配客户端直接打开常用库，也不把“内部测试新表”变成发布版双写统计。

### P3：先完成一条纵向链路

依赖 P2。选 WorkBuddy 完成事件 → 默认正文 → root/child → 成员查询 → 临时阅读验证，随后以 ZCode 验证原生请求关联与可变 SQLite。

- WorkBuddy 验证独立工具 call/result、根子正文、三份用量、rawContent 与正文开关。
- ZCode 验证请求-消息原生 join、更新状态、请求与汇总二选一。
- 比较相同 fixture 在所有正文策略下的事件和统计。

验收：默认模式、仅统计、开启工具正文、历史补摄均可端到端；不用八家全部改完才发现公共模型错误。

### P4：其余适配器及完整来源对账

依赖 P3。按 Claude → Codex → Pi → Qoder → dsh → Antigravity 的顺序落地；各工作包按第 6 节验收。

- 每家先事件与默认正文，再可选正文、关联和覆盖。
- 不强制八家支持相同延迟/结果字段；不支持即能力缺失。
- 真实来源审计只读，写入临时 NoEnding 库；记录预期数字差异。

验收：八家均有基础完整链路；全量/增量/重读/补摄对账通过；复杂身份样例不再依赖不准确通用去重。

### P5：正文设置、任务与清理

依赖 P3，可在 P4 部分完成后接入测试；全来源开放前必须等 P4。

- 接入 policy 设置持久化与界面。
- 按来源/会话/成员/分类补摄，任务进度与取消，普通摄取互斥。
- 保存实际覆盖和不可补齐原因。
- 清理预览与执行，保留事件，清理链接/索引；根可见对话受影响时更新现有边界。
- 测试关闭后未写入 payload/log/FTS，且不会自动清除历史正文。

验收：设置行为与第 3.3 节一致；任务重启可安全重跑；源缺失和政策变化不会被当作完成。

### P6：成员 API、页面与阅读器

依赖 P3/P5；发布前需八家通过。

- 逻辑会话详情继续展示 aggregateStats 和成员入口。
- 新成员详情、事件过滤、正文阅读、双向关联定位。
- 复用分页阅读与来源组件，处理 generation 更新和定位失败。
- Settings、统计/搜索跳转、成员面包屑和能力状态接入。

验收：root/child/side 都能独立查看；错误 member/session 组合被拒绝；成员没有伪 Resume/Owner 操作；事件-only 页面可用。

### P7：搜索、统计换源与现有 Context 最小适配

依赖 P4/P6。

- 根可见对话读取适配器接入现有 Context，保留根序号/generation 和既有 CAS。
- SearchHit 与索引支持成员，保持回收站/投影防护。
- 全维度统计切到事件汇总，增加质量/覆盖标识。
- 明确哪些统计仅在有源能力时显示，执行时间不混同会话跨度。

验收：现有根对话 Context 可用；根对话变化遵守已有前沿检查；子/辅与可选正文不混入现有输入；所有已保存正文可按规定搜索并定位。本轮不验收多成员 Context 的新鲜度机制。

### P8：删除旧链路、格式切换与发布验证

依赖 P0–P7 全部完成。

- 删除 MemberObservation 累计写入、Stats*、session_member_stats、usage_events 和旧 root-only 假设。
- 删除仅为开发保留的旧统计转换桥、双路径存储和过期注释；保留根对话读取适配器与有意义的旧行为回归，更新断言口径。
- 进行格式提升、备份/重建/恢复演练，不自动删除用户库。
- 全部必需检查、macOS/Windows CI、真实只读审计和性能检查。
- 更新 README、相关设计稿引用与字段能力说明，旧文档标为历史或被本方案替代。

验收：发布结构只有一套统计事实来源；没有遗留静默重建、旧格式兼容或未完成的八家分支。

建议按阶段和适配器拆为可审查变更组，不将其视为一次“小改字段”。工作量主要集中 P2/P4/P7。先完成 P0 和 P3 的真实纵向验证，再估算后续时长；当前不提供未经验证的人日承诺。

## 11. 测试矩阵

### 11.1 存储与身份

- 相同事实多次全读/增量读：ID、数量、用量稳定。
- 两次内容相同的真人输入/工具调用：仍是两次。
- 多工具同请求：工具数正确，请求只一次。
- 请求重试/取消/失败：身份分开，用量按源保留。
- 半成品随后完整：更新事实，不被 INSERT OR IGNORE 卡住。
- 正文修订：旧版本仍有来源，新版本成为当前投影。
- 关联晚到：能补，跨错误 Session 不能补。
- 文件替换/同尺寸改写/截断/换代/分支：无重复执行与幽灵正文。
- 注入、摘要、relay 和工具返回：不计真人输入。
- 模拟任意提交步骤失败：事实、正文、游标、认领一致回滚。
- 缺失源、回收站、永久删除：保留/过滤/清理语义正确。

### 11.2 正文策略

每个 Agent 至少验证：默认、仅统计、工具参数/结果开启、注入开启、推理开启；补充测试全部分类开/关、任务中途改策略和源不可用。

要求：事件事实与统计不随策略改变；所有正文可找到主事件；正文关闭时数据库与搜索/日志不残留新增全文。开启历史补摄不增加统计；重复补摄不新增正文修订。

重跑被关闭分类，不清空此前保存的正文；用户清理后不在未开启分类的后台重摄中复活。允许用户再次开启并明确补摄恢复。

### 11.3 Context/搜索/页面

- 根与子同内容、同时间但来源不同，详情和证据可区分。
- 成员当前分支改变，旧正文不进入搜索；根分支变化按已有规则影响 Context。
- 子报告、工具结果、注入和推理补摄不进入本轮 Context，也不推进根对话边界。
- 根可见对话补摄/修订/清理遵守现有 generation 与 CAS 检查，避免破坏已有提取行为。
- 仅统计模式无正文，页面与已有 Context 不崩溃，不自动调用 AI。
- 默认搜索不受推理/工具海量正文污染，扩展搜索可查。
- 双向分页、过滤、命中跳转、generation 切换正确。
- memberId/sessionId 不匹配和回收站访问规则有后端回归。

### 11.4 审计与性能

扩展 `src-tauri/examples/usage_source_audit.rs` 或新增统一审计 example：

- 只读源；临时库测试默认/全正文/仅统计/补摄。
- 每成员事件身份、请求、Token、工具、正文分类、关联与缺口对账。
- 记录源字段不支持与预期修正，不把旧统计当唯一真值。
- 对比重读前后汇总与 ID；抽样每种正文的来源和相关事件。
- 记录数据库大小、默认正文占用、事件 payload 大小、每家摄取耗时与分页响应。

性能目标先在 P0 固定真实基线与预算。禁止整库/整成员正文无界返回；沿用当前阅读页约 60 条分页。可选大文本有明确大小限制、truncated 标识和源引用；不能静默截断后标成完整。

### 11.5 必需工程检查

每个发布候选完成：

```bash
cd src-tauri
cargo fmt --check
cargo check --all-targets
cargo test --all-targets

cd ..
pnpm install --frozen-lockfile
pnpm test
pnpm build
```

涉及文件身份、路径、SQLite WAL、压缩和来源读取时，在最终 HEAD 验证 macOS 与 Windows CI。没有 Windows 真实 Agent 源样本的项明确记录能力缺口，不能把 macOS 通过等同 Windows 源格式已验证。

## 12. 数据格式、用户数据与恢复

按照当前仓库约定，schema.rs 是唯一格式来源，破坏性变化提高 DATABASE_FORMAT_VERSION 并重建，不编写老格式迁移分支。本次编写方案不改版本；实际切换在 P8。

现有数据库不仅有可再摄取的会话缓存，还含人工维护的项目/工作流归属、Context、修订、设置等。不能承诺“删库重摄就能恢复全部数据”。

P0 必须产出两份清单：

1. 可再生：来源中仍存在的成员、事件、正文、派生索引。
2. 不可再生：用户维护的归属、Context/修订、工作流内容、设置、回收站意图，以及源已删除但只存于旧库的内容。

切换前通过 SQLite 一致备份接口或关闭写入后的完整快照备份旧库，包含 WAL 状态，不能仅复制正在写入的主 .db 文件。

恢复最低要求：旧程序和旧库备份可一起回退；旧库备份不被新程序覆盖。若需要自动带入人工资料，先交付在旧程序侧导出的格式无关工作区资料包，并在新格式导入；按稳定 Session/成员身份映射，旧正文序号/Context 前沿不能直接照搬。源已缺失的正文若要保留，也必须进入显式导出/导入范围。

上述资料包是明确的备份导入合同，不是新程序临时读取任意旧表的兼容分支。在恢复闭环完成前，不对常用库执行破坏性重建。格式不匹配时给出可执行的恢复路径，不静默删除。

## 13. 最终完成标准

以下全部满足才算重构完成：

- 八种 Agent 的 root/child/side 都使用统一事件与正文合同。
- 全部可识别事实按类型保存；流式、镜像、继承和聚合不重复计量。
- 正文默认分类生效，可选分类只在用户开启后保存。
- 正文必有主事件，相关请求/工具关联有依据；缺失关联明确标未知。
- 默认/仅统计/自定义、补摄/关闭/清理行为一致且有覆盖提示。
- 逻辑会话与各类成员页面清晰，成员不会变成错误的独立 Session。
- 搜索支持所有成员的有效已保存内容；现有 Context 通过读取适配继续处理根可见对话，多成员提取留待后续重构。
- 数量和用量只来自统一事件，未知数据不伪装成零或精确请求数。
- 旧统计链路和 root-only 正文限制删除，无发布版双写或老格式兼容。
- 用户数据恢复演练、必需测试、真实源只读审计、性能与平台验证完成。

## 14. 本次方案提交的验证记录

本节只记录编写方案时工作区的基线检查，不代表上述重构已经实现。此次新增文件仅为本方案，保留原有未提交代码。

| 检查 | 本次结果 |
| --- | --- |
| cargo fmt --check | 通过 |
| cargo check --all-targets | 通过 |
| cargo test --all-targets | 通过；既有 ignored 测试按原配置跳过 |
| pnpm install --frozen-lockfile | 通过，lockfile 未变；有既有 esbuild build-script 提示 |
| pnpm test | 22 个测试文件、143 项测试通过 |
| pnpm build | 通过；有既有大于 500 kB 的产物提示 |
| 文档结构 | 代码围栏成对、P0–P8 各一处、参考文档存在、无尾空格或编码替代字符 |

方案编写阶段未运行平台 CI，未改源读取或业务代码，未重建数据库，也未执行历史补摄。本目录当前被仓库忽略；按用户提交要求显式纳入此方案文件，不改变其余忽略规则。
