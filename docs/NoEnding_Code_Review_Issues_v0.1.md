# NoEnding Code Review Issues v0.1

> 基于 NoEnding v0.3 产品/技术方案与当前仓库实现的代码 Review 整理。

## 文档目的

将当前最影响 NoEnding 核心假设——“长期 Context 能跨 Session 演进且保持可信”——的问题，整理为可直接落入 GitHub 的工程 Issue。每个 Issue 都包含问题背景、影响、建议解决方案、实现要点与验收标准。

## 推荐开发顺序

```text
#1 Event Identity / Cursor
        ↓
#2 SourceReference
        ↓
#3 Sync Transaction
        ↓
#4 Authority / Audit
        ↓
#5 LaunchIntent / Binding
        ↓
#6 Resume Delta
        ↓
#7 Cross-platform
        ↓
#8 Storage / Domain
```

建议将 #1~#5 作为一个独立的 **Context Integrity Milestone**。

---

## Issue 1 · [P0] 重新设计 Session Event Identity 与 Cursor，安全处理 truncate / rewrite / compact

**目标：** 把 Agent 源文件中的位置与 NoEnding 内部稳定事件身份彻底分离，确保历史 append-only、可追溯、可重放。

### 问题背景

- 当前 Codex / Claude / Pi Adapter 基本使用 JSONL 当前行号作为 SessionEvent.sequence。
- session_events 以 (session_id, sequence) 作为事件身份，并存在 INSERT OR REPLACE 式写入。
- 当 Agent Session 文件发生 truncate、rewrite、compact、rotate 或 file replacement 时，源行号可能被复用，从而覆盖 NoEnding 已经保存的历史事件。
- 这直接违背 NoEnding 的长期 Context 可信性原则：Agent 原始 Session 即使被 compact、rewrite 或删除，NoEnding 已摄入的历史仍必须保留。

### 影响

- 历史事件可能被新内容覆盖。
- ContextItem 的 SourceReference 可能指向已经被替换的事件。
- Search、Resume Delta、Audit 都会在错误历史之上继续运行。
- 一旦出现数据污染，很难再从 Agent 原始文件恢复。

### 建议解决方案

#### 1. SessionEvent 改为应用内稳定 ID

- 新增稳定 event id，作为真正主键。
- sequence 改为 NoEnding 自己分配的单调递增序号，不再等于 JSONL 行号。
- 增加 source_event_id、source_generation、source_position/source_offset 等字段。

#### 2. SessionCursor 保存 Source 状态

- 记录 source_file_identity、generation、byte_offset、last_source_event_id、last_seen_size、mtime。
- source_file_identity 可由 canonical path + inode/file id/fingerprint 等组合得到。

#### 3. Source mutation detection

- append：same source identity 且 current_size >= last_seen_size，从 byte_offset 继续读。
- truncate：current_size < last_seen_size，创建新的 source generation 并重新扫描。
- replacement：source file identity 发生变化，创建新 generation。
- rewrite：identity 不变但此前区域 fingerprint 发生变化时，视为 rewrite。

#### 4. Source Event 去重

- 优先使用 Agent 原生 source event id。
- 没有稳定原生 ID 时，以 normalized payload + timestamp + event type + source metadata 生成 hash 作为 fallback identity。

#### 5. 禁止覆盖历史

- Session Event 不再使用 INSERT OR REPLACE 覆盖正文历史。
- 新 source identity -> INSERT；已见 source identity -> SKIP。
- 只有明确的 metadata enrichment 才允许 UPDATE。

#### 6. 数据库迁移

- 给 session_events 增加 id、source_event_id、source_generation、source_position/source_offset、source_identity_hash。
- 主键从 (session_id, sequence) 迁移到 id。
- 对稳定 Source Identity 建唯一索引，具体键可按 Agent 能力进一步细化。

### 验收标准

- [ ] JSONL append 后只新增事件，不产生重复事件。
- [ ] truncate 到 0 后重新写入内容不会覆盖旧历史。
- [ ] compact/rewrite 后旧事件仍能从 Session history 与 Search 中查询。
- [ ] file replacement 能识别为新的 source generation。
- [ ] 相同 Source Event 被重新扫描时不会重复插入。
- [ ] session_events 不再通过 INSERT OR REPLACE 覆盖历史。
- [ ] NoEnding Event sequence 不再依赖 Source 行号。
- [ ] Codex / Claude / Pi 各增加至少一个 truncate/rewrite fixture test。

### 主要涉及文件

- `src-tauri/src/adapters/codex.rs`
- `src-tauri/src/adapters/claude.rs`
- `src-tauri/src/adapters/pi.rs`
- `src-tauri/src/ingestion/mod.rs`
- `src-tauri/src/storage/mod.rs`

---

## Issue 2 · [P0] 修复 CLI Context Extractor 的 SourceReference 映射错误

**目标：** Prompt 中的临时 #1/#2 引用必须映射到真实 SessionEvent，而不能被误当成真实 sequence。

### 问题背景

- Context Extractor 构建 Prompt 时，会把参与分析的事件临时编号为 #1、#2、#3。
- 模型返回 source_refs=["#1"] 后，当前 parser 会直接把 #1 当成 Session Event sequence 1。
- 但 #1 仅表示“本次 Prompt 的第一个事件”，真实事件 sequence 可能是 101、105 或其他值。

### 影响

- ContextItem 的证据链指向错误事件。
- Audit、冲突分析、后续 Context 修订都会建立在错误 SourceReference 之上。
- 现有测试若只使用 sequence 1、2，会掩盖该问题。

### 建议解决方案

#### 1. 构建 Prompt 时同时构建 Reference Map

- 为每个临时 short_ref 保存 event_id、sequence、raw_ref。
- 例如：#1 -> event_id=e-xxx / sequence=101，#2 -> event_id=e-yyy / sequence=105。

#### 2. 解析模型响应时只通过 Reference Map 解析

- 模型仍只看到 #1/#2。
- parser 使用 short_ref -> PromptEventRef -> SessionEvent.id -> SourceReference。
- 不再直接 parse("#1") -> 1。

#### 3. SourceReference 优先指向稳定 Event ID

- 长期建议从 session:<session-id>#<sequence> 迁移到类似 session-event:<event-id>。
- sequence 仅用于展示排序，不再承担 Identity 职责。

#### 4. 拒绝伪造或不存在的引用

- 模型返回 #999 而 Prompt 仅包含 #1~#12 时，应 reject/ignore 并记录 diagnostics。

### 验收标准

- [ ] 真实 sequence=101 时，Prompt #1 能正确定位到该事件。
- [ ] 一个 mutation 可以关联多个 SourceReference。
- [ ] 重复 short_ref 不产生重复 SourceReference。
- [ ] 不存在的 #999 不会生成虚假引用。
- [ ] SourceReference 可以反向定位到完整 SessionEvent。
- [ ] 测试覆盖非 1 起始、非连续 sequence。

### 主要涉及文件

- `src-tauri/src/sync/extractor.rs`
- `src-tauri/src/domain/models.rs`
- `src-tauri/src/storage/mod.rs`

---

## Issue 3 · [P0] 将 SyncJob 改造成原子事务，保证 Context / Audit / Cursor 一致性

**目标：** 一次 Sync 要么全部成功，要么完全不生效，避免部分 Context 写入后 cursor 未推进导致污染与重复。

### 问题背景

- 当前一次 Sync 会依次应用多个 mutation、写入 SyncRun，最后推进 cursor，但整个过程没有形成单一数据库事务。
- 中途失败时，可能出现部分 Context 已写入、Cursor 未推进的状态。
- 下一次 Sync 会重新处理同一 delta，可能导致重复 Revision、重复 Resolve/Supersede 等副作用。
- 另外启动 reconcile worker 可能在 AppState manage 之前访问 state，存在初始化 race。

### 影响

- Context 长期状态出现难以解释的半成功。
- Retry 可能生成重复或矛盾 Revision。
- Cursor 与实际 Context 状态不一致。
- 启动时 background reconcile 存在潜在 panic/race。

### 建议解决方案

#### 1. 引入 Storage Transaction API

- 一次 Sync 的所有数据库写操作使用同一 transaction connection。
- 任何一步失败必须整体 ROLLBACK。

#### 2. 事务范围

- persist normalized events（如属于本阶段）。
- apply context mutations。
- create ContextItem revisions。
- create/update conflicts。
- persist binding changes。
- insert SyncRun。
- advance processed cursor。

#### 3. 区分 read_cursor 与 processed_cursor

- read_cursor 表示 Source 已安全进入 NoEnding Event Store。
- processed_cursor 表示这些 Event 已完成 Context Sync。
- 可区分“事件已读但 LLM/merge 失败”和“事件尚未摄入”。

#### 4. SyncRun 增加幂等信息

- 增加 source_generation、from_cursor、to_cursor、delta_fingerprint。
- 可使用 sync_run_id + source refs + mutation fingerprint 作为 retry 去重依据。

#### 5. 修复启动 reconcile race

- 先 app.manage(AppState)，再启动 reconciliation worker。

### 验收标准

- [ ] 任意第 N 个 mutation 注入错误后，ContextItems / Revisions / Conflict / processed cursor 均不产生部分修改。
- [ ] SyncRun 与 processed cursor 在同一个事务中提交。
- [ ] Retry 相同 delta 不生成重复 Revision。
- [ ] read_cursor 与 processed_cursor 概念与存储均明确。
- [ ] AppState 在 reconcile worker 启动前完成初始化。
- [ ] 增加 transaction rollback 与 crash/retry integration test。

### 主要涉及文件

- `src-tauri/src/sync/mod.rs`
- `src-tauri/src/storage/mod.rs`
- `src-tauri/src/lib.rs`

---

## Issue 4 · [P0] 统一 Context Authority / Audit Policy，禁止 Agent 静默覆盖用户 Context

**目标：** 所有 Context 修改都必须经过统一 Authority Policy，并留下可追溯 Revision/Audit。

### 问题背景

- Add 路径已经对 user_explicit / user_edit 做了部分保护，但 Supersede、Resolve、set_item_status、delete 等路径仍可能绕过这套规则。
- heuristic extractor 可能把从 user_message 提取的信息标记成 agent_statement，混淆了“谁表达事实”和“谁执行抽取”。
- 设计原则要求用户明确输入/编辑的 Context 不能被 Agent 推断静默覆盖。

### 影响

- 用户明确决策可能被后续 Agent 推断无提示覆盖。
- 不同 mutation path 的规则不一致，长期会造成不可预测的 Context 演进。
- Status 改变若没有 Revision，无法解释某条 Context 为什么消失或变成 resolved。

### 建议解决方案

#### 1. 引入统一 MutationPolicy

- 所有 mutation proposal 都先经过 Authority Policy 和 invariant validation。
- 建议决策：Allow / CreateConflict / RequireUserConfirmation / Reject。

#### 2. 明确 Authority 语义

- 保留 user_explicit、user_edit、system_observed、agent_statement、agent_inferred。
- 不要把 Authority 简化成单一数字优先级，但必须明确用户权威内容不可被低权威 Agent 信息静默覆盖。

#### 3. 分离 authority / created_by / source_type

- 例如用户说“数据库继续用 SQLite”，由 Assistant 抽取时：authority=user_explicit，created_by=extractor/assistant，source_type=session_event。

#### 4. 所有状态变化都形成 Revision/Audit

- resolve、supersede、archive、reactivate、delete/tombstone、user edit 均记录 previous/new value、previous/new status、actor、source refs、sync_run_id、timestamp、reason。

#### 5. 冲突保留为 ContextConflict

- 高 Authority Context 与低 Authority 新信息冲突时创建 Conflict，而不是静默 supersede。

### 验收标准

- [ ] 所有 Context mutation 均经过统一 MutationPolicy。
- [ ] user_edit 不能被 agent Supersede 或 Resolve。
- [ ] user_explicit 不会被 agent_inferred 静默替换。
- [ ] user message 提取结果能保持 user authority。
- [ ] authority 与 created_by 分离。
- [ ] status change / delete / tombstone 均产生 Revision/Audit。
- [ ] Authority 冲突能创建 Conflict。
- [ ] 增加 Authority Policy matrix tests。

### 主要涉及文件

- `src-tauri/src/sync/merge.rs`
- `src-tauri/src/sync/extractor.rs`
- `src-tauri/src/commands.rs`
- `src-tauri/src/domain/models.rs`

---

## Issue 5 · [P0] 引入 LaunchIntent，保证 New Session 选择的 Workstream 能稳定建立 Binding

**目标：** 持久化“这次启动选了哪些 Workstream”，并在外部 Agent Session 被发现后可靠建立 SessionWorkstreamBinding。

### 问题背景

- 产品模型中 Session ↔ Workstream 是多对多；用户 New Session 时选择的 Workstream 应成为明确 Binding。
- 启动外部 Agent CLI 的瞬间通常还不知道最终 Agent Session ID。
- 当前实现采用“先 launch、后 discovery”，但缺少持久化对象连接 launch intent 与随后发现的 Session。
- 因此用户显式选择可能丢失，而后续 AI auto-classification 不能替代用户明确选择。

### 影响

- 从 Workstream A 启动的新 Session 可能最终仍是 unbound。
- Resume 时可能找不到 Workstream Context。
- 用户显式行为与系统自动分类的权威级别混淆。

### 建议解决方案

#### 1. 新增 LaunchIntent 领域对象

- 字段建议：id、launch_type(new/resume)、agent、selected_workstream_ids、cwd、context_bundle_id、process_id、launched_at、matched_session_id、status。
- status 建议：Pending / Matched / Ambiguous / Expired / Failed。

#### 2. New Session 流程持久化

- Create LaunchIntent -> Build Context Bundle -> Launch Agent -> Discover external Session -> Correlate -> Create Bindings -> Mark matched。

#### 3. Session Matching

- 组合 agent type、cwd、launch timestamp、session created_at、first activity、process id、parent process id 等证据。
- 只有一个高置信候选时自动匹配；多个接近候选进入 Ambiguous，不静默猜测。

#### 4. Binding 记录来源与置信度

- explicit_launch_selection: confidence=1.0。
- automatic_classification: 低于显式选择，不能覆盖 explicit binding。

#### 5. Crash Recovery

- LaunchIntent 必须持久化；应用重启后 Reconcile 能将 pending LaunchIntent 与近期 Agent Session 重新匹配。

### 验收标准

- [ ] 从 Workstream A 启动 New Session 后自动建立 A Binding。
- [ ] 选择 A+B 后建立两个 Binding。
- [ ] 未选择 Workstream 时允许 0 Binding。
- [ ] explicit launch selection 的 source/confidence 被持久化。
- [ ] 自动分类不会覆盖 explicit Binding。
- [ ] 多个候选 Session 时能进入 Ambiguous。
- [ ] NoEnding crash 后可恢复 pending LaunchIntent。
- [ ] Resume specific Session 不重新猜 Binding。
- [ ] 增加 Launch -> Discovery -> Binding integration test。

### 主要涉及文件

- `src-tauri/src/launcher/mod.rs`
- `src-tauri/src/sync/mod.rs`
- `src-tauri/src/storage/mod.rs`
- `src-tauri/src/domain/models.rs`

---

## Issue 6 · [P1] 实现基于 Revision 的 Resume Context Delta 与多 Workstream Context Aggregation

**目标：** Resume 应注入“自上次活动以来发生了什么变化”，多 Workstream 应先聚合语义再渲染，而不是简单拼接。

### 问题背景

- 当前 Context Builder 主要依赖 last_used_at 查找变化，但 Resume Bundle 同时仍包含完整 Core/Extended Context。
- 这使 Resume 更接近 Full Context + Partial Delta，而不是真正的 Context Delta。
- 多 Workstream 当前主要逐个 append，缺少 Shared Task、Primary/Related、跨 Workstream Conflict 等聚合。
- Frontend Preview Resume 也需要具体 session_id 才能知道该 Session 上次看过什么。

### 影响

- Resume Prompt 变长，重复信息多。
- Agent 难以识别真正发生变化的决策、约束与冲突。
- 多 Workstream 组合时会重复约束/决策，且主次关系不清。

### 建议解决方案

#### 1. 用 ContextSnapshot / Revision Set 替代时间作为语义版本

- Binding 或 ContextDelivery 保存 last_delivered_snapshot_id / delivered_revision_set。
- 只有 Context 成功交付给 Session 后才更新 snapshot。

#### 2. Resume Delta 基于 Revision 差异

- 计算 current active revisions - last delivered revision set。
- 同时捕获 new / updated / resolved / superseded items，以及 new/resolved conflicts。

#### 3. 推荐 Resume Bundle 结构

- Current Task Reminder：最小 Core Reminder。
- Changed Since Your Last Activity。
- Resolved / Superseded。
- New Conflicts。
- Relevant Artifacts。

#### 4. 多 Workstream 先 Aggregation 再 Rendering

- 构建 AggregatedContext：shared_task、primary_workstream、related_workstreams、constraints、decisions、open_questions、conflicts、relevant_items。
- 先 deduplicate / rank / detect relationships / detect conflicts，再渲染 Markdown。

#### 5. 调整 Preview API

- Resume mode 下 session_id 必须存在，例如 preview_bundle(session_id, workstream_ids, mode)。

### 验收标准

- [ ] Resume Delta 不再主要依赖 last_used_at。
- [ ] Binding/Delivery 保存实际 Context revision snapshot。
- [ ] 成功 delivery 后才更新 last-delivered snapshot。
- [ ] 没有变化时 Resume 不重复完整 Context。
- [ ] 修改一个 Decision 后 Resume 只出现相关变化。
- [ ] resolved / superseded item 能作为 Delta 出现。
- [ ] new Conflict 能作为 Delta 出现。
- [ ] Preview Resume 必须携带 Session ID。
- [ ] 多 Workstream Bundle 去重重复 Constraints，并区分 Primary / Related。
- [ ] 跨 Workstream Conflict 单独呈现。

### 主要涉及文件

- `src-tauri/src/context/mod.rs`
- `src/features/launcher/LauncherModal.tsx`
- `src/api.ts`

---

## Issue 7 · [P1] 修复跨平台 Launcher / Context Injection，并建立 macOS + Windows CI

**目标：** Adapter 只描述结构化命令，不生成 Shell Syntax；macOS/Windows 双平台都必须通过持续编译与参数传递测试。

### 问题背景

- 当前 Context 注入辅助逻辑会生成类似 $(cat '/path/to/context.md') 的 shell substitution，再交给 Platform Launcher 进行 quoting。
- 在 macOS 下如果整体被单引号包裹，$() 不会展开，Agent 可能收到字面字符串。
- Windows 分支还存在参数 quoting、路径带空格、PowerShell 拼接等风险，并且缺少 CI 对 Windows cfg 分支持续编译。
- Executable resolver 的 version probe 使用 Command::output()，没有真正 hard timeout。

### 影响

- Agent 可能根本没有收到 Context 内容。
- 带特殊字符或空格的 Prompt/路径可能启动失败。
- Windows 专属代码问题长期隐藏到发布阶段。
- 异常 CLI 可阻塞应用启动。

### 建议解决方案

#### 1. Adapter 不生成 Shell Script

- AgentCommand 保持结构化：program、args、cwd、injection。
- 建议 InjectionSpec：InitialPrompt(String) / FollowUp(String) / ContextFile(PathBuf)。
- 需要传 Prompt 时由 Rust 直接 fs::read_to_string，传真实字符串，不生成 $(cat ...)。

#### 2. Platform 负责安全 Process 启动

- 尽量不经过 Shell；必须经过 PowerShell 时实施严格 argument escaping。
- 覆盖 path with spaces、引号、$、&、;、backtick、newline、Unicode。

#### 3. 建立双平台 CI

- GitHub Actions matrix 至少包含 macos-latest 与 windows-latest。
- 执行 cargo check、cargo test、frontend install/build；Tauri 完整 build 可根据 CI 成本选择。

#### 4. Executable probe 增加 hard timeout

- 例如 1~3 秒，超时视为 probe failed，不阻塞 App startup。

### 验收标准

- [ ] Adapter 不再生成 $(cat ...) 或 PowerShell/Shell script fragment。
- [ ] macOS Agent 实际收到 Context 文件内容。
- [ ] Windows executable path 带空格可启动。
- [ ] Prompt 包含引号、$、&、;、newline、中文、emoji 时仍正确传递。
- [ ] Windows target cargo check 通过。
- [ ] macOS + Windows CI 每次 PR 自动运行。
- [ ] frontend 在两个平台 CI 中 build。
- [ ] Executable version probe 有 hard timeout。

### 主要涉及文件

- `src-tauri/src/adapters/mod.rs`
- `src-tauri/src/adapters/codex.rs`
- `src-tauri/src/platform/launcher.rs`
- `src-tauri/src/platform/exec_resolver.rs`

---

## Issue 8 · [P1] 修复 Storage / Domain 一致性问题并补齐 Session Classification / Project Affinity 模型

**目标：** 修复若干数据库/API 正确性问题，并补齐 Project 作为“可选组织层”所需要的领域状态与证据模型。

### 问题背景

- review 发现 upsert_workstream 不更新 project_id、list_sessions 动态参数编号可能错误、delete_project 与 Workstream FK/生命周期语义冲突等具体问题。
- 领域模型中 classification_state、ContextConflict、ProjectAffinityEvidence / ProjectResolver 也尚未完整落地。
- Workstream 是核心连续性单元，Project 只是可选长期组织层，因此路径/repository 只能作为归属证据，不能直接等同 Project。

### 影响

- Workstream 在 Project 间移动可能不持久化。
- Session filter 在特定组合下可能发生参数绑定错误。
- 删除 Project 可能失败或错误影响 Workstream 生命周期。
- Project 自动归类缺少 evidence/score，用户也难以纠正。
- Session 分类状态只能被间接推断，缺乏明确领域语义。

### 建议解决方案

#### 1. 修复 upsert_workstream

- ON CONFLICT 更新 project_id。
- 覆盖 Project A -> Project B -> NULL 的移动测试。

#### 2. 修复 list_sessions 动态参数

- 使用动态参数 Vec、named params、query builder 或明确分支，避免手工维护 ?1/?2/?3 与条件同步。

#### 3. 修正 delete_project 语义

- 先将 Workstream.project_id = NULL，再 DELETE Project。
- 删除 Project 不应删除或自动 archive Workstream。

#### 4. 补齐 Session Classification State

- 支持 unassigned / partially_assigned / assigned，作为正式领域状态或可靠 derived projection。

#### 5. 增加 ProjectAffinityEvidence / ProjectResolver

- cwd、git repo、workspace path、Agent metadata、Session title、历史 Workstream relation 都只能作为 evidence。
- Resolver 基于 evidence source + score 输出 suggested Project，允许用户纠正。

#### 6. 自动 Workstream Discovery 支持 project_id = NULL

- 识别到独立 Workstream 时无需先确定 Project。

#### 7. ContextConflict 独立领域对象

- 建议逐步引入 left_item/right_item、conflict_type、status、resolution 等字段；若实现规模较大可再拆票。

### 验收标准

- [ ] Workstream 可从 Project A 移动到 Project B，再移动为 standalone。
- [ ] 删除 Project 后 Workstream 保留且 project_id = NULL，不会自动 archive。
- [ ] 仅 Agent filter、仅 Project filter、Agent+Project、无 filter 四种 Session 查询均正常。
- [ ] Session 可表达 unassigned / partially_assigned / assigned。
- [ ] cwd/repository 只作为 Project Affinity Evidence。
- [ ] Resolver 记录 evidence source 与 confidence/score。
- [ ] 用户可以纠正自动 Project 建议。
- [ ] 自动创建 Workstream 时允许 project_id = NULL。

### 主要涉及文件

- `src-tauri/src/storage/mod.rs`
- `src-tauri/src/commands.rs`
- `src-tauri/src/domain/models.rs`
- `src-tauri/src/search/mod.rs`

---

## Milestone 建议

### Context Integrity

建议包含 Issue #1~#5。该阶段完成后，NoEnding 才具备验证核心产品假设所需要的最基本可信链路：

- 历史事件不会因 Agent compact/rewrite 被覆盖；
- 每条 Context 可以追溯到正确 Source；
- Sync 不会留下半完成状态；
- 用户明确 Context 不会被 Agent 静默覆盖；
- New Session 的显式 Workstream 选择不会在 discovery 后丢失。

随后再推进 Resume Delta、Cross-platform hardening、ProjectResolver / ContextConflict UI 等能力。