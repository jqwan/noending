# 全量测试审查（2026-10-09）

本记录对应审查时的代码；已使用完毕的一次性数据库转换工具、专用说明及 9 项 Python 测试随后已移除。

## 范围与统一标准

以 `8d5591c` 为本次审查基线，逐项阅读了现有测试的前提、执行入口、断言和相关夹具：前端 31 个测试文件、Rust 30 个含单元测试的源文件及 29 个集成测试文件、Python 1 个测试文件，共 91 个文件。同时检查了测试配置、公共测试帮助函数和离线转换脚本的用途。附录列出完整文件范围；是否修改不代表测试的重要程度。

评判不依据代码作者、近期是否改动或模块名称，而依据以下标准：

1. 是否保护可观察的产品规则、公开契约、数据完整性或外部协议；普通业务规则和错误分支同样可以值得测试。
2. 前提能否真正触发声称的风险，失败后是否有有效断言；排除自比较、空集合检查、只验证夹具和未执行目标动作的“通过”。
3. 预期是否独立于实现；不为了重构锁定私有结构、图标排列或任意像素值。展示内容若影响业务判断、路由或操作可用性，仍保留测试。
4. 只有入口、条件和结果实质重复时才合并；不同 Agent 格式、平台语义、数据分支或跨层契约不因名称相似而删除。
5. 有价值但薄弱的测试优先修正。检查 Mock 实现、调用历史、缓存、剪贴板和持久状态隔离，明确真实环境测试的条件。

这是对全部**现有测试有效性**的审查，不代表所有产品功能均已有自动化覆盖，也不以覆盖率或减少数量为目标。本次不扩充视觉快照或真实 TUI 的测试体系。

## 删除与合并清单

共移除 23 个独立测试项：前端 7 项、Rust 16 项。合并项的业务断言已由保留项承接，清单中的名称为审查基线的原名称。

| 原文件与测试 | 处理 | 依据与保留覆盖 |
| --- | --- | --- |
| `src/features/sessions/SessionDetailView.test.tsx`<br>`renders agent icon before session title and plain agent name under session info` | 删除 | 只约束图标位置、CSS 容器和文字摆放；会话信息与实际交互保留在同文件其他测试。 |
| `src/features/sessions/SessionDetailView.test.tsx`<br>`offers the same 删除 entry whatever the root source state` | 合并 | 相同 present/missing 前提下，紧随其后的两个永久删除交互测试都已检查入口并执行预览、确认。 |
| `src/features/sessions/NewSessionView.test.tsx`<br>`renders task options containing only task title without directory` | 删除 | 只限制下拉框标签的拼接形式；任务选择、创建、归档限制及目录解析测试保留。 |
| `src/features/sessions/SessionConversationView.test.tsx`<br>`lays the user-message ticks on a fixed pitch, not on their document position` | 删除 | 只锁定刻度的三个精确像素值；定位、分页、滚动保持和用户消息导航测试保留。 |
| `src/features/workstreams/WorkstreamCard.test.tsx`<br>`omits launch actions for archived tasks` | 合并 | 将归档任务不能新建会话的断言并入同文件归档/取消归档/删除交互测试。 |
| `src/features/sessions/terminalTheme.test.ts`<br>`honors an explicit app theme over the system preference` | 合并 | 将显式主题优先级和 ANSI 色差断言并入同文件系统主题变更测试，保留监听与取消监听覆盖。 |
| `src-tauri/src/context/diagnostics.rs`<br>`logging_failure_is_best_effort_and_does_not_fail_operation` | 删除 | 没有检验操作失败是否被忽略，仅检查 UUID 长度；context/mod.rs 的 service_success_survives_unwritable_context_log_directory 验证实际业务成功。 |
| `src-tauri/tests/owner_model_test.rs`<br>`trash_and_restore_preserve_the_owner` | 合并 | session_lifecycle_test.rs 的 archive_keeps_every_fact_and_never_touches_the_source、archive_filters_board_but_preserves_task_projections 和 unarchive_keeps_identity_data_and_search 已覆盖关联及投影。 |
| `src-tauri/tests/owner_model_test.rs`<br>`matched_launch_intent_gives_the_discovered_session_that_owner` | 合并 | launch_context_test.rs 的 pending_intent_matches_new_session_and_sets_owner 覆盖同一匹配入口、所有者和意图状态。 |
| `src-tauri/tests/owner_model_test.rs`<br>`matched_ownerless_intent_leaves_the_session_unowned` | 合并 | launch_context_test.rs 的 contextless_launch_matches_but_stays_unowned 覆盖同一无所有者匹配。 |
| `src-tauri/tests/owner_model_test.rs`<br>`standalone_new_session_has_no_owner_and_falls_back_to_the_default_workspace` | 合并 | base_launch_flow_test.rs 的 standalone_new_session_launches_through_the_prepared_flow、standalone_prepared_launch_reports_the_default_workspace 验证实际准备及启动。 |
| `src-tauri/tests/owner_model_test.rs`<br>`resume_uses_the_sessions_current_owner_and_needs_no_extra_argument` | 合并 | launch_context_test.rs 的 prepare_resume_uses_current_owner_without_writing 和 launcher_workspace_test.rs 的 a_resume_launches_in_the_sessions_own_cwd 覆盖相同准备行为。 |
| `src-tauri/tests/owner_model_test.rs`<br>`resume_uses_the_default_workspace_even_when_its_task_has_a_usable_path` | 合并 | launcher_workspace_test.rs 的 a_resume_fallback_is_recorded_in_the_prepared_payload 已设置任务可用路径、会话目录缺失，并检查默认工作目录回退。 |
| `src-tauri/tests/session_workspace_test.rs`<br>`session_gets_a_derived_project` | 合并 | 相同 reconcile 帮助函数和源文件形状；将项目、工作目录和无任务归属断言并入 discovered_session_gets_a_workspace_path。 |
| `src-tauri/tests/session_workspace_test.rs`<br>`reconcile_discovers_and_attaches_sessions_to_workspace_paths` | 合并 | 相同 reconcile 发现/绑定行为；将标题、工作目录和项目断言并入 discovered_session_gets_a_workspace_path。 |
| `src-tauri/tests/session_workspace_test.rs`<br>`the_detail_ingredients_come_from_storage_queries` | 删除 | 手工组合存储查询，没有调用声称保护的详情 API；消息/前沿/统计/源文件判定分别由 schema、session_meta、context 和详情页测试覆盖。 |
| `src-tauri/tests/sync_integrity_test.rs`<br>`mutation_failure_rolls_back_entire_run` | 合并 | 同文件 retry_after_rollback_applies_once_and_reapply_is_deduped 已包含相同回滚场景；补入冲突行回滚断言。 |
| `src-tauri/tests/sync_integrity_test.rs`<br>`mutation_outside_the_owner_is_skipped_not_written` | 合并 | owner_model_test.rs 的 mutations_outside_the_owner_are_skipped_not_written 同入口验证越界拒绝，并在同批验证合法写入和冲突隔离。 |
| `src-tauri/tests/logical_session_graph_test.rs`<br>`duplicate_commits_dedup` | 合并 | schema_test.rs 的 storage_round_trip_matches_the_schema_promises 及 event_identity_test.rs 的 identity_dedup_follows_chain_semantics/reingest_preserves_message_ids_and_dedups 已检验重复提交与重扫去重。 |
| `src-tauri/tests/workstream_paths_test.rs`<br>`display_order_does_not_change_project_membership` | 合并 | 同文件 every_project_is_projected_and_display_reordering_does_not_change_it 覆盖相同项目投影和重排操作。 |
| `src-tauri/tests/workspace_project_test.rs`<br>`an_unregistered_home_policy_leaves_the_registry_open` | 合并 | 普通路径项目创建及保留目录策略测试已覆盖无保留目录时的开放注册；此项重复正常路径。 |
| `src-tauri/tests/workspace_project_test.rs`<br>`unreferenced_missing_path_is_gc_d` | 合并 | 将缺失路径 GC 报告及路径行移除断言并入同文件 last_path_gc_retires_project。 |
| `src/features/agents/AgentsView.test.tsx`<br>`renders sources with no toggle checkbox and displays total sources metric` | 合并 | 来源数量断言并入同步/来源统计测试；删除对已不存在的复选框控件的静态否定检查。 |

## 保留并补强的测试

- **事务失败与数据删除**：将会话创建的故障注入移到行写入之后、搜索索引失败之处，并覆盖已有会话更新；任务永久删除先捕获 revision/conflict ID，再核实实际子行删除，避免父行已删后子查询总为空。级联删除先种入真实投影行。
- **Schema 契约**：把“必须存在的对象”与独立查询出的 SQLite schema 对比，而非复制一份相同的硬编码名单；保留格式拒绝、数据库身份、FTS 类型、唯一性等不同约束。
- **命令与平台协议**：真实执行生成的 Unix shell 脚本，逐字比较工作目录及含空格、引号、换行、Unicode、变量和命令替换符号的参数；原测试未断言包装结果。真实 Git 测试必须有 Git，不能环境缺失就提前返回且显示通过；只读检查比较文件字节，避免等长改写漏检。
- **摄入、项目和所有者**：工作区刷新测试种入真实消息及可发现的新源文件，验证历史保持和刷新不摄入；所有者独立性测试使用真实注册的第二项目/路径，并实际更新 cwd；运行配置指纹先记录再修改，避免与自己比较。不同格式的读取、断点、重写和去重测试均保留。
- **前端行为与隔离**：清理跨用例 Mock、剪贴板、缓存和 view state；路由测试直接检查功能组件收到的身份/预置参数，去掉无关 IPC/xterm Mock；终端失败后点击重试并核实再次连接和回放；存储设置核实实际 API 参数；Agent 来源操作核实确认门槛和目标 ID。随机执行发现侧栏分组状态泄漏，已加入独立初始状态；等待实际异步更新后再断言，消除 React `act` 警告。
- **离线数据库转换**：保留全部 9 项数据安全测试，补充独立的字面量预期，核对消息、归属、路径、游标、前沿、Context 引用及元数据、历史修订、配置、冲突和 fork 关系。本次仅执行临时夹具测试，没有转换用户数据库。
- **命名与手动测试**：将声称“统计”“不写入”“匹配全部解析结果”的名称改为实际断言行为，删除过时注释和无效 Mock。真实历史与 CLI 发现检查要求至少有一个真实样本/安装；真实 Codex 助手测试显式配置 Codex，并验证返回 runtime，避免走纯检索后仍通过。

## 审查发现的实际缺陷

`Db::upsert_logical_session` 原来把会话写入和搜索索引写入直接放在同一个连接上，并没有事务。补强后的故障注入发现：索引失败后，新会话行残留。同一写入路径的会话更新也缺少回滚保护。

现已复用 `Db::tx`，把行和索引写入放在同一事务；夹具帮助方法复用该入口。回归测试同时覆盖新建与更新失败。API 和数据库格式保持不变，无需迁移。除此之外，本次修改限于测试及审查文档。

## 真实环境测试与验证边界

保留 4 项默认忽略的手动检查，它们需要用户本机环境，不能算作默认套件已通过：

| 测试 | 条件和检查内容 |
| --- | --- |
| `adapters::fingerprint_tests::real_agent_files_match_fingerprints` | 至少一个本地真实历史成员；检查发现的 Agent 归属、来源路径及身份。 |
| `real_agents_answer_discovery_or_warn` | 至少一个已安装 Agent CLI；检查发现结果或明确警告、模型身份和 effort 词表。 |
| `real_pi_local_qwen_headless` | Pi CLI，以及 LM Studio 的 `qwen/qwen3.8-27b`；真实模型执行烟雾检查。 |
| `real_codex_assistant_chat_roundtrip` | 已安装且登录的 Codex CLI；真实助手往返、runtime 及只读回答。 |

本次在 macOS 验证；Windows 专属分支未执行，也没有触发远程 CI。真实 Agent TUI、桌面跳转、浏览器布局和模型调用不在本次自动化通过结果内。构建仍有现有的大 chunk 提示。

## 最终验证结果

| 检查 | 结果 |
| --- | --- |
| `pnpm exec tsc --noEmit` | 通过。 |
| `pnpm test` | 31 个文件、233 项全部通过。 |
| `pnpm test --sequence.shuffle --sequence.seed=1009` | 31 个文件、233 项全部通过；作为额外顺序隔离检查，不能证明所有随机顺序。 |
| `pnpm build` | 通过；仍有已有的大 chunk 提示。 |
| `cargo fmt --check` | 通过。 |
| `cargo check --all-targets` | 通过。 |
| `cargo test --all-targets` | 全部 31 个目标成功退出，547 项通过、4 项默认忽略、0 项失败。 |
| `python3 -m unittest discover -s scripts -p 'test_*.py' -v` | 9 项全部通过。 |
| `git diff --check` | 通过。 |

前端从 240 项降为 233 项，Rust 默认执行项从 563 项降为 547 项；4 项手动检查和 9 项 Python 测试保留。前端最终普通顺序和随机顺序均无 React `act` 警告；错误分支测试仍会输出其预期的错误日志。后端初次完整运行暴露的事务缺陷及补强夹具的问题处理后，重新执行了完整套件，以上结果不是仅重跑失败目标得出的结论。

## 完整文件清单

“调整”包含删除/合并、断言、夹具、隔离或命名修正；“保留”表示逐项审查后无须修改。所有文件都经过语义阅读，并非只统计测试名称。

| 文件 | 结论 |
| --- | --- |
| `src/app/AppShell.test.tsx` | 调整 |
| `src/app/Router.test.tsx` | 调整 |
| `src/components/CommandPalette.test.tsx` | 保留 |
| `src/components/ErrorBoundary.test.tsx` | 保留 |
| `src/components/Toast.test.tsx` | 保留 |
| `src/components/WorkspacePathField.test.tsx` | 调整 |
| `src/components/common.test.tsx` | 保留 |
| `src/features/agents/AgentsView.test.tsx` | 调整 |
| `src/features/projects/ProjectDetail.test.tsx` | 保留 |
| `src/features/projects/ProjectsView.test.tsx` | 调整 |
| `src/features/projects/remoteUrl.test.ts` | 保留 |
| `src/features/search/SearchView.test.tsx` | 保留 |
| `src/features/sessions/NewSessionView.test.tsx` | 调整 |
| `src/features/sessions/SessionConversationView.test.tsx` | 调整 |
| `src/features/sessions/SessionDetailView.test.tsx` | 调整 |
| `src/features/sessions/SessionMessage.test.tsx` | 保留 |
| `src/features/sessions/SessionSubpageTabs.test.tsx` | 保留 |
| `src/features/sessions/SessionTable.test.tsx` | 调整 |
| `src/features/sessions/SessionTerminalView.test.tsx` | 调整 |
| `src/features/sessions/SessionsView.test.tsx` | 调整 |
| `src/features/sessions/continueDesktop.test.ts` | 保留 |
| `src/features/sessions/terminalTheme.test.ts` | 调整 |
| `src/features/settings/SettingsView.test.tsx` | 调整 |
| `src/features/workstreams/WorkspacePaths.test.tsx` | 保留 |
| `src/features/workstreams/WorkstreamCard.test.tsx` | 调整 |
| `src/features/workstreams/WorkstreamDetailView.test.tsx` | 调整 |
| `src/features/workstreams/WorkstreamFormModal.test.tsx` | 调整 |
| `src/features/workstreams/WorkstreamsView.test.tsx` | 保留 |
| `src/hooks/useViewState.test.tsx` | 保留 |
| `src/layout/NavigationRail.test.tsx` | 保留 |
| `src/layout/Sidebar.test.tsx` | 调整 |
| `src-tauri/src/adapters/antigravity.rs` | 调整 |
| `src-tauri/src/adapters/claude.rs` | 保留 |
| `src-tauri/src/adapters/codex.rs` | 调整 |
| `src-tauri/src/adapters/dsh.rs` | 保留 |
| `src-tauri/src/adapters/mod.rs` | 调整 |
| `src-tauri/src/adapters/pi.rs` | 保留 |
| `src-tauri/src/adapters/qoder.rs` | 调整 |
| `src-tauri/src/adapters/workbuddy.rs` | 保留 |
| `src-tauri/src/adapters/zcode.rs` | 保留 |
| `src-tauri/src/agent_runtime/discovery/codex.rs` | 保留 |
| `src-tauri/src/agent_runtime/discovery/pi.rs` | 保留 |
| `src-tauri/src/commands/ingestion.rs` | 保留 |
| `src-tauri/src/commands.rs` | 保留 |
| `src-tauri/src/context/diagnostics.rs` | 调整 |
| `src-tauri/src/context/mod.rs` | 调整 |
| `src-tauri/src/domain/models.rs` | 保留 |
| `src-tauri/src/error.rs` | 保留 |
| `src-tauri/src/platform/exec_resolver.rs` | 保留 |
| `src-tauri/src/platform/exec_runner.rs` | 保留 |
| `src-tauri/src/platform/launcher.rs` | 保留 |
| `src-tauri/src/platform/paths.rs` | 保留 |
| `src-tauri/src/storage/schema.rs` | 调整 |
| `src-tauri/src/sync/policy.rs` | 保留 |
| `src-tauri/src/terminal/identity.rs` | 调整 |
| `src-tauri/src/terminal/mod.rs` | 保留 |
| `src-tauri/src/terminal/spawn.rs` | 调整 |
| `src-tauri/src/workspace/home.rs` | 保留 |
| `src-tauri/src/workspace/identity.rs` | 调整 |
| `src-tauri/src/workspace/project.rs` | 保留 |
| `src-tauri/src/workspace/resolver.rs` | 调整 |
| `src-tauri/tests/agent_runtime_argv_test.rs` | 保留 |
| `src-tauri/tests/agent_runtime_discovery_test.rs` | 调整 |
| `src-tauri/tests/agent_runtime_test.rs` | 保留 |
| `src-tauri/tests/base_experience_test.rs` | 调整 |
| `src-tauri/tests/base_launch_flow_test.rs` | 保留 |
| `src-tauri/tests/context_review_test.rs` | 保留 |
| `src-tauri/tests/context_workbench_test.rs` | 调整 |
| `src-tauri/tests/event_identity_test.rs` | 保留 |
| `src-tauri/tests/launch_context_test.rs` | 调整 |
| `src-tauri/tests/launch_cwd_test.rs` | 保留 |
| `src-tauri/tests/launcher_workspace_test.rs` | 保留 |
| `src-tauri/tests/llm_cli_test.rs` | 调整 |
| `src-tauri/tests/logical_session_graph_test.rs` | 调整 |
| `src-tauri/tests/owner_model_test.rs` | 调整 |
| `src-tauri/tests/project_cards_test.rs` | 保留 |
| `src-tauri/tests/schema_test.rs` | 调整 |
| `src-tauri/tests/search_test.rs` | 保留 |
| `src-tauri/tests/session_lifecycle_test.rs` | 保留 |
| `src-tauri/tests/session_meta_test.rs` | 调整 |
| `src-tauri/tests/session_workspace_test.rs` | 调整 |
| `src-tauri/tests/sync_engine_test.rs` | 保留 |
| `src-tauri/tests/sync_integrity_test.rs` | 调整 |
| `src-tauri/tests/workspace_identity_test.rs` | 保留 |
| `src-tauri/tests/workspace_probe_test.rs` | 保留 |
| `src-tauri/tests/workspace_project_test.rs` | 调整 |
| `src-tauri/tests/workspace_resolver_test.rs` | 调整 |
| `src-tauri/tests/workstream_cards_test.rs` | 保留 |
| `src-tauri/tests/workstream_lifecycle_test.rs` | 调整 |
| `src-tauri/tests/workstream_paths_test.rs` | 调整 |
| `scripts/test_migrate_database_v3.py` | 调整 |
