# NoEnding Core Workspace Experience v0.1

## 多 Agent 并行执行方案

> **v0.1.1（实施前勘误）** 本版本在派发任何子 Agent 之前，先对照 `b1b8efe` 的真实代码审过一遍。原文与代码不符之处已就地改写，并在 §35 保留完整证据清单。标记 ⚠️ 的四处是产品/架构分叉，采用了默认判断，等你一句话就能翻掉。

---

# 0. 阶段目标

NoEnding 当前暂停继续建设智能能力。

本阶段目标不是继续提高：

```text
Context Extraction
Context Classification
Context Injection
Assistant
Context Review
Context Quality Eval
```

而是：

> **让 NoEnding 在完全不依赖 Context Intelligence 的情况下，也能成为一个每天可用的 Codex / Claude Code / Pi 本地工作空间。**

核心用户闭环：

```text
打开 NoEnding
    ↓
看到最近工作
    ↓
找到一个 Session
    ↓
Resume
    ↓
New Session
    ↓
可选加入 Workstream
    ↓
继续工作
    ↓
第二天重新找到并继续
```

整个过程不要求用户理解：

```text
Context
Context Delivery
Context Review
Conflict
Assistant
```

---

# 1. 本阶段全局产品原则

## 1.1 Base Experience First

当前优先级：

```text
Session
Workstream
Project
Launch / Resume
Search
Navigation
Settings
Usability
```

高于：

```text
Context Intelligence
Assistant
Automation Intelligence
```

---

## 1.2 中文作为当前 UI 主语言

NoEnding v0.1 当前界面统一使用：

> **简体中文为主语言**

这意味着用户可见 UI：

```text
页面标题
按钮
菜单
状态
Empty State
错误提示
设置项
表单标签
帮助文案
Toast
Modal
```

原则上都使用中文。

允许保留少量产品领域词：

```text
Session
Workstream
Project
Agent
Runtime
Codex
Claude Code
Pi
```

这些作为 NoEnding 的正式产品术语，不强行翻译。

例如：

```text
New Session
```

改为：

```text
新建 Session
```

而不是：

```text
新建会话
```

建议统一采用“中文动作 + 英文领域名”。

---

# 2. 统一产品词汇表

主 Agent 在开始并行任务前先冻结以下词汇。

| English         | UI 中文         |
| --------------- | ------------- |
| Home            | 首页            |
| Sessions        | Sessions      |
| Workstreams     | Workstreams   |
| Projects        | Projects      |
| Settings        | 设置            |
| Search          | 搜索            |
| New Session     | 新建 Session    |
| Resume          | 继续            |
| New Workstream  | 新建 Workstream |
| Recent          | 最近            |
| Continue        | 继续工作          |
| Agent default   | Agent 默认值     |
| Default Agent   | 默认 Agent      |
| Runtime         | Runtime       |
| Directory / cwd | 工作目录          |
| Last Activity   | 最近活动          |
| Started         | 开始时间          |
| Archive         | 归档            |
| Unarchive       | 取消归档          |
| Edit            | 编辑            |
| Remove          | 移除            |
| Add             | 添加            |
| Save            | 保存            |
| Cancel          | 取消            |
| Close           | 关闭            |
| Loading         | 加载中           |
| No results      | 没有匹配结果        |
| Detected        | 已检测           |
| Not detected    | 未检测           |
| Active          | 进行中           |

### 2.1 本阶段必须补充的词（原表缺失）

| English               | UI 中文       | 为什么必须定 |
| --------------------- | ----------- | ------ |
| Primary binding       | 主关联        | Session Detail 的 BindingModal 已在渲染 Primary/Related（`SessionDetailView.tsx:182-184`），不定义就一定被机翻 |
| Related binding       | 相关关联       | 同上                  |
| Lifecycle             | 状态          | §14 的新段落名           |
| Open                  | 进行中        | 与 Active 同词，见 §2.2   |
| Completed             | 已完成        | 目前没有切换入口，见 §14      |
| Abandoned             | 已放弃        | 同上                  |
| Refresh / re-ingest   | 刷新          | §11.6 用它替代"同步提取"     |
| Sources               | Session 来源  | 设置一级导航已存在该节          |
| Session ID            | Session ID  | 不翻译，代码/证据可解析要求       |
| Preview               | 预览          | Launcher 的 preview==launch |
| Stale / state changed | 状态已变化      | PreparedLaunch 失效提示，见 §15 |
| Ask Assistant         | 询问 Assistant  | Commit 0 实做时定的，四个入口同词 |
| Workspace (分组标题)     | 工作区         | Sidebar 分组标题，非领域词 |
| Go to X               | 前往 X        | 命令面板；X 保持领域词原样 |
| Delivery Off/Compact/Balanced/Detailed | 关闭 / 精简 / 均衡 / 详细 | 只在实验区出现 |
| Theme system/light/dark | 跟随系统 / 浅色 / 深色 | 设置 → 外观 |
| On / Off (状态徽标)      | 开 / 关       | 自动化状态，必须是真的      |

### 2.2 ⚠️ 两个易撞车的词，先定死

原方案 §13 写 `Last Activity → 最近活动`，§14 写 `Edited → 最近更新`。它们是**不同概念**，子 Agent 极易合并：

```text
Session   的 transcript 最后事件时间  → 最近活动
Workstream 的 record updated_at     → 最近更新
Agent     的运行状态 Active          → 进行中
Workstream 的 lifecycle open        → 进行中   ← 与上一行同词，允许
```

现状已经这么用了（`ContinueSection.tsx:25` 「最近活动」、`SessionDetailView.tsx:80` "Last active"、`WorkstreamDetailView.tsx:273` "Edited"），不要改回去。

### 2.3 ⚠️ Slogan

Home Hero 与 Sidebar 是同一句 slogan 的两种语言（`Sidebar.tsx:67` 已是中文「对话会结束，上下文不会。」，`HomeView.tsx:41` 仍是英文）。默认**统一中文**，英文原句只保留在窗口标题/产品名描述里（`tauri.conf.json:15`、`Cargo.toml:4` 不动）。§12 的目标文案已按此改写。

不要让不同子 Agent 自己发明一套中文。

---

# 3. 不做完整 i18n

本阶段：

```text
不引入 react-i18next
不建立 locale 文件体系
不做语言切换
不做英文 fallback 系统
```

直接统一现有 UI 文案即可。

原因：

> 当前目标是先让产品可用，不是建设国际化基础设施。

未来需要多语言时，再单独做：

```text
Localization / i18n v1
```

---

# 4. 当前阶段产品模型

NoEnding 简化为：

```text
Project
    ↓ optional organization

Workstream
    ↓ explicit continuity / organization

Session
    ↓ actual Codex / Claude Code / Pi execution
```

语义：

```text
Session
= 一次真实 Agent 会话

Workstream
= 用户认为属于同一件持续工作的若干 Sessions

Project
= 可选的 Workstream 组织层
```

继续保持：

```text
Session 可独立存在

Workstream 可无 Project

Project 不是必需层
```

全局主入口：

```text
新建 Workstream
新建 Session
```

Resume 永远针对具体 Session。

---

# 5. 智能模块状态

以下模块暂停或冻结：

```text
Context Extraction / Merge       PAUSED
Context Quality / Eval           PAUSED
Context Review Loop              FROZEN
Current Context Workbench        FROZEN
Assistant                        PAUSED
Context Injection                OFF by default
Agent Runtime                    SEALED
```

已有实现：

```text
保留代码
保留 schema
保留历史数据
不删除
不重构
```

---

# 6. Base Experience 的后台行为

即使智能能力关闭：

```text
Session discovery            ON
Session ingestion            ON
Session event storage        ON
Search indexing              ON
Session metadata             ON
Manual Workstream binding    ON
```

关闭：

```text
CLI Context extraction
Heuristic Context extraction
Automatic Workstream classification
Context mutation
Context conflict creation
Context review generation
Context injection
```

---

# 7. 一个重要数据原则

Context Intelligence 关闭时：

> **不要消费掉未来仍可能需要处理的 Context processing frontier。**

即：

```text
Session events 继续 ingest

read cursor
→ 正常前进

Context processed cursor
→ 不因为 Intelligence OFF 前进
```

这样未来重新开启 Context Intelligence 时，可以对真实历史数据重新：

```text
extract
evaluate
replay
```

而不会丢失 corpus。

---

# 8. 执行模型

采用：

```text
Main Agent
    ↓
Commit 0
Base Experience Mode
+ 中文语言规则
    ↓
建立统一并行基线
    ↓

┌──────────┬──────────┬──────────┬──────────┐
│ Agent A  │ Agent B  │ Agent C  │ Agent D  │
│ Home     │ Sessions │Workstream│ Launcher │
└──────────┴──────────┴──────────┴──────────┘

                     ↓
                  Main Agent
                  Integration
                     ↓
                   Agent E
               UX / Language QA
                     ↓
                  Final Gate
```

Commit 0 完成以前：

> **不要启动 A / B / C / D。**

## 8.1 Commit 0 必须先冻结的跨边界契约

原方案的 ownership 划分有个真实漏洞：**四个 Agent 里有三对存在跨文件调用，谁都可能改签名。**

```text
SessionsView.tsx  (归 B)  ──挂载──▶ NewSessionModal.tsx      (归 D)
SessionDetailView.tsx (归 B) ──挂载──▶ ResumeSessionModal.tsx  (归 D)
WorkstreamDetailView.tsx (归 C) ──直启──▶ launcher prepared flow (归 D)
useWorkstreamCards.ts (共享 hook) ──▶ HomeView (归 A) + WorkstreamsView (归 C)
```

所以 Commit 0 除了开关，还要**定死这四处调用的 props 形状**（见 §11.9）。子 Agent 只能在契约内实现；要动契约＝SHARED FILE CHANGE＋独立 commit＋报告给 Main。否则 §19 的 cherry-pick 顺序会在集成时才发现四个分支互相不兼容。

### 8.1.1 冻结形状（D 实现，B/C/A 消费）

原则：**新增的一律 optional**，这样 B 从基线出发不改一行也能编译。

```ts
// D 拥有。B/C/A 只许传，不许改签名。
type NewSessionModalProps = {
  onClose: () => void;
  workstreamId?: string | null;  // 预置选中的 Workstream；省略或 "none" = standalone
};

// D 拥有。形状保持不变（B 的 SessionDetailView 正按此调用）。
type ResumeSessionModalProps = {
  sessionId: string;
  onClose: () => void;
};
```

```text
C 的 Workstream 页新建：改为挂载 <NewSessionModal workstreamId={ws.id} …/>
A 的 Home 空状态新建：挂载同一个组件（禁止再写第四条启动路径）
启动路径唯一：NewSessionModal → prepareNewSession → launchPrepared
useWorkstreamCards() 的返回形状不动（A/C 都不改 hook；
  reviewSummaries 何时不再 fetch 由 Main 在集成阶段处理）
```

## 8.2 ⚠️ 并行的真实成本

`worktrees/` 方案下，每个 worktree 都是独立的 `node_modules`（不在 git 里）和独立的 `src-tauri/target/`。四个并行子 Agent 意味着 4 次冷 cargo 构建。两个选择：

```text
A. 四个 worktree 真并行：墙钟最短，磁盘和 CPU 峰值最高，Main 集成 4 次
B. 串行 2 批（D+B 一批、C+A 一批）：构建产物可复用，与 §19 的集成顺序天然一致
```

默认走 **B**，理由就是 §19 已经承认了依赖顺序（Launcher 先于 Sessions/Workstream 先于 Home）——真要"四个一起"反而要回头处理契约冲突。

---

# 9. 主 Agent 职责

Main Agent 负责：

```text
阶段边界
Commit 0
全局中文词汇规范
共享文件
子 Agent 派发
diff 审查
cherry-pick
冲突解决
集成
最终 QA
```

Main Agent 不负责把所有 feature 自己串行写完。

---

# 10. Git / Worktree 方案

Commit 0 合并后得到：

```text
BASE_EXPERIENCE_SHA
```

所有子 Agent：

```text
必须从同一个 SHA 开始
```

建议：

```text
worktrees/
  core-home
  core-sessions
  core-workstream
  core-launcher
```

branches：

```text
core/home
core/sessions
core/workstream
core/launcher
```

子 Agent 禁止：

```text
直接 merge main
直接 push main
修改其他 worktree
rebase 其他子 Agent 分支
```

---

# 11. Commit 0 — Base Experience Mode

由 Main Agent 执行。

建议：

```text
refactor(workspace): default to base experience mode
```

---

## 11.1 Intelligence 开关

增加：

```text
context.intelligence_enabled
```

语义：

```text
missing row
= false
```

即：

> 新用户默认 Base Experience。

**实现约束（原方案未写，但决定成败）：**

```text
settings 是一张裸 KV 表（settings(key,value)），没有 typed struct、没有 serde default、
migrations 从不插入 setting row（storage/mod.rs:130-458）。
"missing row = false" 只能落在一个代码级 accessor 上，
必须和 get_setting 放在一起，且只允许这一处读它。
```

照 `context_delivery_level_of` 的现例写（`settings/mod.rs:15` 就是 fallback 到 Balanced），别再散出第二个 fallback 点。

**⚠️ 与 delivery_level 的关系必须钉死**，因为 AGENTS.md 的不变式是：

> Context Delivery controls outbound context injection only. Turning delivery Off MUST NOT disable ingestion, sync, extraction, Workstream bindings, or context evolution.

所以这是**两个正交开关**，不是一个：

```text
intelligence_enabled = off → 侧关闭 extraction / classification / context mutation / conflict creation
delivery_level       = off → 只关闭 outbound 注入；ingestion 与 sync 照常

delivery_level = off 且 intelligence_enabled = true  → 仍然提取，仍然写 Context，只是不投递
intelligence_enabled = off                            → 不再产生 Context；Context 只读冻结
```

两个既有事实容易被误当成开关，禁止复用：

* `ContextDeliveryPolicy.enabled` 是由 delivery_level **派生**的（`context/mod.rs:57,94,248`），拿它当 intelligence 开关会直接违反上面那条不变式；
* `assistant.agent == "none"` 的含义是"不用模型、只走检索"（`sync/extractor.rs:262`），不是关闭智能。

第一版可以不在普通 UI 暴露。

如必须保留调试入口：

```text
设置
→ 数据与高级
→ 实验性功能
```

---

## 11.2 Context Delivery 默认 Off

默认：

```text
context.delivery_level = off
```

保留：

```text
compact
balanced
detailed
```

实现，但不是默认体验。

**⚠️ 落地语义（原方案的"没有 setting row → Off"与现状冲突）：**

现状是 **没有 row → `Balanced`**，两处写死：`settings/mod.rs:15` 的 fallback 和 `context/mod.rs:28-32` 的 `impl Default`。已有本地库的用户**都没有这一行**，所以"改 fallback 为 Off"会在升级当场静默改变他们的注入行为——这正是 AGENTS.md 禁止的"静默覆盖用户意图"式副作用。

默认方案：

```text
新库          → 不写行，代码 fallback = Off（新语义）
既有库 migrate → 显式 INSERT context.delivery_level = 'off'，并记录迁移前的行为
```

即：**Off 对新老用户都成立，但对老用户是一次显式、可审计、可一键回开的写**，不是一个静默的常量翻转。这与 §11.1 的 `intelligence_enabled`（全新键，缺行即 Off，无需迁移）刚好对称。

**必须同时更新的既有断言**（否则 §27 的 build gate 直接红）：

```text
tests/launch_context_test.rs:1012  context_delivery_level_default_and_roundtrip  ← 断言默认 Balanced
```

另外这些测试依赖"默认是 Balanced"这一前提，需要显式在测试里设 level，不能靠默认：

```text
:2031 prepare_new_does_not_create_intent_or_delivery_or_file
:2068 prepare_resume_does_not_commit_extra_bindings_or_delivery
:902  token_budget_limits_sections_to_actually_delivered_content
:1047 context_delivery_level_monotonicity
:1174 balanced_off_balanced_preserves_revisions_in_delta
```

---

## 11.3 Reconcile 拆分

现有启动 reconcile 需要确保：

```text
Session ingestion
```

不依赖：

```text
Context processing
```

Intelligence OFF：

```text
scan session sources
    ↓
discover Sessions
    ↓
ingest events
    ↓
search/index
    ↓
STOP
```

不进入：

```text
SyncEngine.prepare
Extractor
MergeEngine
Context mutation
processed cursor advance
```

**STOP 到底放在哪（原方案只描述了形状，没给落点，且只提了"启动 reconcile"）：**

ingestion→sync 的耦合**不止 reconcile 一处**，一共三条路径共用同一个尾部。只改 reconcile 会留下两个后门，Resume 一次就把提取重新点着：

```text
1. ingestion/mod.rs:81   ingest_and_sync_session_nb   ← 异步版，reconcile 走这里（:243）
2. ingestion/mod.rs:61-77 run_session_sync 的加锁孪生   ← reconcile_source(:274) / reingest_source(:311)
3. launcher/mod.rs:1090-1120                            ← prepare_new / sync_stale_for_workstreams /
                                                          prepare_resume / commands::sync_session
```

所以门禁必须是**一个共享函数**（例如 `context_processing_enabled()`），三处调用它，而不是三处各写一遍 `if`。

**游标结论（好消息，原方案的 §7 数据原则可以零成本成立）：**

`processed_sequence` 在生产代码里只有一个前进点——`sync/mod.rs:472`，位于 `commit` 的事务内。跳过 `prepare/extract/commit` 就天然不前进；read cursor 走完全独立的 `upsert_source_cursor_conn`（`storage/mod.rs:887,1048-1070`），并且它用 `COALESCE(...,0)` 保住 `processed_sequence` 不被覆盖（`:2268`）。

**注意一个已有的假门：** `sync/mod.rs:310-312` 在批次为空时短路返回 runtime `"none"`，但它**仍然往下 commit、仍然前进游标**。别把它当成"跳过提取"的实现复用。

**重开智能时的积压（原方案完全没提，属于 §7 的必然后果）：**

Off 期间 `processed_sequence` 冻结，历史 corpus 全部留在事件里。重新打开后第一次 Sync 要一次吞掉整个积压，这符合设计意图，但必须承认两件事：

```text
积压规模 = Off 期间的全部新事件
→ 首次重开必须是分批的（现有 SyncEngine 已按 session/批次推进，不要在这一步改它）
→ UI 不能把"第一次同步很慢"报成失败；Home/Sessions 在此期间显示"加载中"而不是空状态
```

---

## 11.4 Sidebar

当前：

```text
Workstreams
Sessions
Assistant
```

改为：

```text
Workstreams
Sessions
```

Assistant route 保留。

只是从主导航隐藏。

**⚠️ Assistant 的入口不止 Sidebar 一处。** 原方案只列了导航，实际有四个入口，漏掉的话 §24 的"主路径不得出现 Ask Assistant"在验收时必然失败：

```text
src/layout/Sidebar.tsx:85-88                              主导航
src/components/CommandPalette.tsx:20                      "Go to Assistant"
src/features/workstreams/WorkstreamDetailView.tsx:212-215  "Ask Assistant"（§14 已覆盖）
src/features/projects/ProjectDetail.tsx:50-51              "Ask Assistant"（原方案未提）
```

四个入口统一由 §11.9 的开关挂载，路由 `/assistant` 保持可达（§24 的"除非用户主动进入 Experimental"）。

---

## 11.5 Home

欢迎页结构 **保持现状**。

空状态继续保留：

```text
Logo

对话会结束，上下文不会。

新建 Workstream

or

新建 Session
```

不重新设计 Hero。

已有数据时：

```text
欢迎回来

继续上次的工作
```

结构保持：

```text
ContinueSection
RecentSessions
```

只移除：

```text
ContextUpdatesSection
```

不要重做整个 Home。

**移除成本已实测，原文的顾虑可以删掉：** `reviewSummaries` 在 Home 里只有一个消费者（`HomeView.tsx:85` 传给 ContextUpdatesSection）。删除是 3 行（import 7、JSX 83-87），`useWorkstreamCards` hook 本体不用动——`WorkstreamsView.tsx:26` 本来就只解构 `cards, defaultAgent, refresh`。**不要**顺手删 hook 字段，那是共享文件。

---

## 11.6 Session Detail

移除：

```text
同步提取
```

以及：

```text
同步之后系统会尝试自动归类
```

这类智能提示。

保留：

```text
Session metadata
Workstream binding
Messages
Resume
```

**要移除的精确清单（现文件已经半中文化，别照原方案的语言去找英文）：**

```text
:59  "同步提取"                  按钮
:35  "同步中…"
:37  "提取了 ${r.applied} 个 Context 变更" / "没有新的有效上下文"
:90  「尚未关联 Workstream。同步之后系统会尝试自动归类；也可以不带上下文直接 Resume。」
:108 "还没有摄入消息。"
:109 "同步这个 Session"          按钮
:34-40 doSync → api.syncSession  处理函数
```

**⚠️ 原方案的洞：删掉"同步提取"之后，用户在一次使用中没有主动摄入的入口了。** §28 的人工验收（第 14-16 步）只测"重开应用后被发现"，靠的是启动 reconcile；但 dogfooding 时"我刚在终端里跑的那个 Session 呢"会立刻发生。

处理：把 `:59` 和 `:109` 两个动作**改名为「刷新」并保留**，语义降级为**只摄入、不提取**——`commands.rs:1001 sync_session` 在 `intelligence_enabled=false` 时走完 ingestion 后按 §11.3 的同一门禁 STOP。这条命令因此仍然被前端使用（§34 里"成为不可达"的清单要把它排除掉）。:90 的提示改成只讲绑定与 Resume，不再承诺自动归类。

---

## 11.7 Settings

`Context & Sync` 不再作为普通设置入口。

采用：

```text
移动到 数据与高级 / 实验性功能
```

（不是删除、不是原地隐藏：§11.1 的 intelligence_enabled 调试入口需要同一个落点，否则实验区无处安放。）

同时统一中文。现状 `SettingsView.tsx:10-17` 的六个 section 标签全是英文（`General / Agents / Session Sources / Context & Sync / Appearance / Data & Advanced`），`ContextSyncSettings` 在 `:166-269`，四级 delivery 选项在 `:142-163`。

**不要碰：** `agents` 一节（`:127-140` 渲染 `AgentRuntimeSettings.tsx`）与 `GeneralSettings:76-104` 的 Default Agent——Agent Runtime 已按 §5 封板。

---

## 11.8 Commit 0 语言清理

Main Agent 只负责公共层：

```text
Sidebar
Settings 一级导航
Global buttons
Global modal common labels
Home 公共文案
```

不要在 Commit 0 一次扫完整仓。

各 feature 的语言清理由对应子 Agent负责。

**工作量已经量过，按此分配即可（约 170-190 条字面量）：**

```text
Main  Commit 0   Sidebar ~7 + Settings 一级导航 ~37 + 全局按钮/弹窗
A     Home       HomeView ~9 + ContextUpdatesSection ~6（后者大概率整体卸载）
B     Sessions   SessionsView ~23 + SessionDetailView ~17 + SessionTable ~10
C     Workstream WorkstreamsView ~16 + WorkstreamContext/SinceLastReview ~5 each
D     Launcher   NewSessionModal/ResumeSessionModal ~5 each + LaunchResultModal
Main  集成后      SourcesView/SourceDetailModal ~13 + ProjectDetail ~13（两个都不在 A-D 的 ownership 内）
```

最后那行不是笔误：Sources 与 Projects **没有任何子 Agent 认领**。默认由 Main 在集成阶段自己扫，不扩包给子 Agent，避免 ownership 重叠。

---

## 11.9 ⚠️ 统一的"不挂载"机制（原方案缺）

§11.4 说"从主导航隐藏"、§11.7 说"隐藏或移动到实验区"、§12 说"移除挂载"——三种做法同时下发给四个 Agent，结果必然是四套互不兼容的条件渲染，Agent E 再花一轮去磨平。Commit 0 必须提供唯一机制：

```text
src/app/experience.tsx        ← 含 JSX，所以是 .tsx
  useBaseExperience(): { intelligenceEnabled, deliveryLevel }
  useDeliveryOff(): boolean   ← §15 用
  refreshBaseExperience()     ← 设置页改完开关后必须调用
  <IntelligenceOnly>          ← 包住所有智能 UI，off 时不挂载（不是 CSS 隐藏）
```

读不到两个开关时**落回 Base Experience**（宁可少显示，绝不误显示智能界面）。

规则：

```text
读一次、判一处
off = 不挂载组件，但路由/深链/命令全部保持可达
实验入口：设置 → 数据与高级 → 实验性功能
禁止：为隐藏而删 import、改组件签名、动 backend 命令
```

Commit 0 同时冻结 §8.1 列出的四处跨边界 props 契约。


---

# 12. Agent A — Home Minimal Adaptation

## Mission

保持当前欢迎页设计，仅做 Base Experience 适配和中文统一。

---

## Ownership

```text
src/features/home/**
```

必要时：

```text
Home 专用组件
```

不要修改：

```text
sessions/**
workstreams/**
launcher/**
src-tauri/**
```

---

## 目标

保留现有视觉结构。

空状态：

```text
NoEnding

对话会结束，上下文不会。

开始一件可以跨 Session、跨 Agent 继续推进的事。

[新建 Workstream]

或

[新建 Session]
```

（按 §2.3，Hero 不再保留英文原句。现状对应 `HomeView.tsx:41,43`，副标题已经是中文，只需换主句。）

已有内容：

```text
欢迎回来

继续上次的工作
```

下面继续使用：

```text
ContinueSection
RecentSessions
```

---

## 必做

移除：

```text
Context Updates
Needs Attention
Review
Conflict
```

Home 不再依赖：

```text
reviewSummaries
```

如果删除这个依赖会导致大规模 hook 重构，则仅停止使用，不扩大 scope。

---

## 中文化

例如：

```text
Good to see you again.
→ 欢迎回来

Continue where you left off.
→ 继续上次的工作

Recent Sessions
→ 最近 Sessions

Resume
→ 继续

New Workstream
→ 新建 Workstream
```

---

## Commit

```text
refactor(home): align home with base experience
```

---

# 13. Agent B — Sessions Experience

## Mission

让 Sessions 成为当前阶段最可靠的核心页面。

---

## Ownership

```text
src/features/sessions/SessionsView.tsx
src/features/sessions/SessionTable.tsx
src/features/sessions/SessionDetailView.tsx
src/features/sessions/SessionMessage.tsx
```

不要主动修改：

```text
NewSessionModal.tsx
ResumeSessionModal.tsx
```

这些归 Agent D。

**但它们的调用点归你**（`SessionsView.tsx:187` 挂 New、`SessionDetailView.tsx` 挂 Resume）。D 会给 `NewSessionModal` 加 wsId 预置入参（§15），签名在 Commit 0 冻结（§8.1）——你只按冻结后的形状传，不要自己发明 prop 名，否则集成时三方冲突。

---

## 当前已有能力

不要重复实现：

```text
Search
Agent filter
Workstream filter
Project filter
Assigned filter
Recent sorting
Resume
Session Detail
Binding Edit
```

重点是体验打磨。

---

## Sessions 页面

核心字段：

```text
Agent
Session
Workstream
Project
工作目录
最近活动
```

主要操作：

```text
打开
继续
```

---

## 需要重点处理

```text
无标题 Session
长标题
长路径
Windows 路径
无 cwd
无 Workstream
无 Project
未知 Session title
```

---

## Filters 中文化

例如：

```text
All Agents
→ 全部 Agent

All Workstreams
→ 全部 Workstream

All Projects
→ 全部 Project

Unassigned
→ 未关联

Clear filters
→ 清除筛选

Search sessions...
→ 搜索 Sessions...
```

---

## Session Detail

改成：

> Execution-oriented，而不是 Context-oriented。

Header：

```text
Session 标题
Agent
开始时间
最近活动
工作目录
Session ID
```

Workstreams：

```text
关联 Workstream

编辑
```

Messages：

```text
消息
```

移除：

```text
同步提取
Context 变更
自动归类
```

---

## Empty States

统一中文：

```text
尚未导入任何 Session

没有符合当前筛选条件的 Session

还没有摄入消息
```

并给出清晰下一步。

---

## Commit

```text
feat(sessions): strengthen session browsing and detail experience
```

---

# 14. Agent C — Workstream Basic Experience

## Mission

让 Workstream 即使没有任何 Context，也具备完整价值。

定义：

> **Workstream = 用户显式组织的一组持续相关 Sessions。**

---

## Ownership

```text
src/features/workstreams/WorkstreamDetailView.tsx
src/features/workstreams/WorkstreamSessions.tsx
src/features/workstreams/WorkstreamsView.tsx
basic Workstream components
```

不要删除或大改：

```text
WorkstreamContext.tsx
NeedsAttentionSection.tsx
SinceLastReview.tsx
ConflictReviewModal.tsx
RecentChangesTimeline.tsx
```

只是暂时不挂载。

---

## 当前 Workstream Detail

现有内容：

```text
Current Context
Since Last Review
Needs Attention
Recent Changes
Sessions
Ask Assistant
```

改成：

```text
Workstream 概览
Sessions
Project
工作目录
Lifecycle
```

---

## 目标

```text
NoEnding

进行中

描述
本地多 Agent workspace 开发

工作目录
~/code/noending


Sessions

Codex
今天
[继续]

Claude Code
昨天
[继续]

[新建 Session]


Project
NoEnding
```

---

## Header

保留：

```text
新建 Session
继续最近 Session
•••
```

移除：

```text
Ask Assistant
```

---

## Workstream 编辑

至少支持：

```text
重命名
编辑描述
编辑默认工作目录
归档
取消归档
```

**实测现状：四项里有两项的 UI 根本不存在**（backend `update_workstream` 已支持整对象写入，`commands.rs:173-180`）：

```text
编辑默认工作目录  ✅ 已有：••• → 工作目录… 弹窗（WorkstreamDetailView.tsx:191-202, 333-349）
归档 / 取消归档    ✅ 已有：archive_workstream（visibility 字段，:185-189, 223-225）
重命名            ❌ 标题是静态文本（:209），无任何输入口
编辑描述          ❌ 只在卡片上只读显示（WorkstreamCard.tsx:60）
```

所以 §14 对 Agent C 来说**不是纯减法，包含两块新增编辑 UI**。这是真实工作量，不要当成"清理一下"。

**⚠️ Lifecycle 需要先定范围。** §14 目标结构里列了 Lifecycle，但现状：

```text
只有 badge 只读显示（:262-264, 354-359）
创建时固定 "open"（commands.rs:157）；"abandoned" 只在 merge_workstreams 里产生（:389-）
没有任何把状态改成 completed / abandoned 的 backend 命令
```

默认按**只读显示**处理。要做成可切换就得新增命令 + schema 允许值语义，那与 §14 自己写的"不要为了接口命名去重构 backend"以及 §16 的"不修改领域模型"相冲，属于超范围。

**rename 有一个必须写进验收的副作用：** `title` / `description` / `updated_at` 都进了 launch 状态指纹（`launcher/mod.rs:475-478`）。改名的瞬间，一个尚未消费的 `PreparedLaunch` 会变 stale 并被拒绝启动。这是**正确行为**（Preview-Launch Identity），但用户会看到"状态已变化"而不是启动成功——不能为了顺手而把 title 从指纹里摘掉。

**移除 SinceLastReview 的连带后果（必须知情）：** 该组件是本页**唯一**可达的 `markWorkstreamReviewed` 入口（`SinceLastReview.tsx:142-149`，`api.ts:54` 是唯一调用方）。卸载它 = Base Experience 下没有任何地方能推进 ReviewState。这符合 §5"保留代码/不删除"和 Review State Integrity（它本来就只管人工观察），但**重开智能时 ReviewSummary 会一次性显示整个 Off 期间的积压**。默认接受这个结果，并在 §29 的数据保留验收里明确记录，不要临场"顺手"给 Home 加个静默 mark-reviewed——那会直接违反 §"Home MUST NOT advance ReviewState"。

---

## 中文化

例如：

```text
Start
→ 开始

New
→ 新建 Session

Resume
→ 继续

Active
→ 进行中

Archived
→ 已归档

Edited
→ 最近更新
```

---

## Backend

允许继续消费现有：

```text
getWorkstreamContext()
```

仅使用：

```text
workstream
project_name
related_sessions
```

不要为了接口命名去重构 backend。

---

## Commit

```text
refactor(workstreams): focus workstream detail on session continuity
```

---

# 15. Agent D — New / Resume Launch Experience

## Mission

让 New / Resume 成为简单可靠的主流程。

---

## Ownership

Frontend：

```text
src/features/sessions/NewSessionModal.tsx
src/features/sessions/ResumeSessionModal.tsx
src/features/launcher/**
```

Backend：

```text
src-tauri/src/launcher.rs
相关 launcher commands
```

---

## 不修改

除非遇到实际 bug：

```text
Agent Runtime
Context Eval
MergeEngine
AuthorityPolicy
Agent adapters
```

这些已经封板。

---

## New Session

目标：

```text
新建 Session

Agent
Codex

工作目录
~/code/noending

Workstream
NoEnding
可选

[启动]
```

Standalone：

```text
Workstream = 无
```

完全合法。

---

## Workstream 发起

从 Workstream 页面进入：

```text
Workstream 自动选中
```

cwd：

```text
workstream.default_cwd
→ latest relevant session cwd
→ otherwise empty
```

cwd 只是启动建议，不决定 Workstream 身份。

**⚠️ 这一节描述的路径今天不存在，而且是全方案最大的缺口。** 实测：

```text
NewSessionModal 只在一个地方挂载：SessionsView.tsx:187（action === "new"）
它的 props 只有 onClose —— 没有任何预置 Workstream 的入参
Workstream 页的 New 按钮根本不开 modal：直接 launchNewSession(agent, [ws.id])
                                              （WorkstreamDetailView.tsx:161-171）
NewSessionModal 里没有任何 cwd 控件，cwd 完全由后端解析（launcher/mod.rs:113, 774-790）
```

也就是说 §15 想要的"Agent / 工作目录 / Workstream / \[启动\]"表单，**三个字段里有一个（工作目录）根本没有输入口，另一个（Workstream 预置）从 Workstream 页进来时走的是另一条不经过 modal 的路**。

更严重的是那条捷径**绕过了 prepare→fingerprint→single-use→launch_prepared 整条完整性链**，直接违反你自己的两条不变式：

```text
Preview-Launch Identity：what you preview is what the Agent receives —— 这里没 preview
Launch Preparation Integrity：直启路径不受 prepared 校验保护
```

默认处理（D 的范围，不属于 C）：

```text
1. Workstream 页的 New 改为打开同一个 NewSessionModal，新增 wsId 预置入参（契约见 §8.1）
2. 所有"新建"统一走 prepare_new → launch_prepared，废除 UI 侧的 launchNewSession 直启
3. NewSessionModal 增加 cwd 显示，来源优先级就是 §15 写的三级：
   workstream.default_cwd → latest relevant session cwd → 空
   （resolve_new_session_cwd 已经实现了这套优先级，launcher/mod.rs:784，直接消费，别在前端重算）
4. 如果 3 要开放编辑 cwd，那是新增能力，需要 backend 参数；默认只读显示 + 允许覆盖为可选后续
5. Home 的空状态「新建 Session」也是直启（`HomeView.tsx:30 plainNewSession` → `api.launchNewSession`）。
   入口由 D 提供，A 负责把 HomeView 接上去；A 不得自己再写一条启动路径
```

这条改动同时影响 B 的 `SessionsView.tsx` 和 C 的 `WorkstreamDetailView.tsx`（两个调用方），所以 **modal 签名必须在 Commit 0 冻结**，否则集成时是三方冲突。

**Context Preview：Delivery Off 时"直接不存在"这件事需要主动删除，不是不渲染。** 现状 off 时区域照旧显示，且带着明示文案：

```text
NewSessionModal.tsx:117-144   off 时显示 "Context Delivery 已关闭 (Off) · 不注入"(:125)、
                              "未选择 Workstream · 0 tokens"(:123)
ResumeSessionModal.tsx:245-271 同样显示 "No Workstream · 0 tokens"(:250) / "Off · 不注入"(:252)
ContextPreviewModal.tsx:72, 112-116  有显式 isOff 分支 + token 估算 badge(:78-85)
```

**⚠️ 拆这些文案时不得连带删掉 prepare 调用。** `ResumeSessionModal` 在 mount 时就无条件 eager prepare（`:53-81`）、关预览后重 prepare（`:296-315`）、unmount 时 cancel（`:83-91`），`handleResume` 用的是 `launchPrepared(preparedRef)`（`:133-141`）。"不显示 Context Preview"是**可见性**变更；把 prepare 当冗余一起删掉就会破坏 §15 自己列的 PreparedLaunch 完整性。

好消息：**这一项是纯前端改动，backend 不需要新分支。** off 时 `build_bundle` 仍然被调用但早退返回空 bundle（`launcher/mod.rs:117, 170-171` → `context/mod.rs:248-258`），状态指纹仍计算且包含 `delivery_level`（`:119-120, 173-180, 456`），launch 时 level 或指纹变化仍会中止（`:210-228`），context 文件在 Off 或空 WS 列表时不生成（`:234-246`），投递快照按 `ctx_file.is_some()` 门控（`:249-260, 379-394`）。所以"off → 完全不显示"与后端既有行为天然一致，且**不会推进 ContextDelivery snapshot**，符合不变式。

---

## Resume

目标：

```text
继续 Session

Agent
Codex

工作目录
...

Workstream
...

Runtime
Agent 默认值

[继续]
```

不要要求用户重新选择没有必要的参数。

---

## Prepared Launch

必须继续保留：

```text
PreparedLaunch
single-use
stale validation
runtime intent freeze
preview-launch identity
```

UX 简化不能绕开 integrity。

---

## Context Preview

Delivery Off 时：

```text
完全不显示 Context Preview 区域
```

也不显示：

```text
0 tokens
No context
Context disabled
```

直接不存在。

---

## Preview

仅展示：

```text
Agent
Runtime
工作目录
新建 / Resume
Workstream
```

---

## 中文化

例如：

```text
Launch
→ 启动

Resume
→ 继续

Cancel
→ 取消

Agent default
→ Agent 默认值

Directory
→ 工作目录
```

---

## Commit

```text
feat(launcher): streamline base new and resume flows
```

---

# 16. 子 Agent 公共约束

所有子 Agent 都必须遵守。

---

## 禁止事项

不要：

```text
优化 Context extraction
增加 Context fixtures
修改 Context prompt
修改 AuthorityPolicy
增加 Assistant
修改 Context Eval
改变 Agent Runtime semantics
加入模型推荐
加入自动 Workstream classification
做完整 i18n
大规模 CSS 重构
引入新框架
```

**再加一组来自不变式的（本轮最容易被"顺手清理"破坏）：**

```text
不要用 CSS 隐藏冒充不挂载，也不要反过来为隐藏而删 import / 改组件签名
不要因为 Context Preview 不显示就连带删掉 prepare / cancel 调用
不要为了让新建"更顺"而绕开 prepare → fingerprint → launch_prepared
不要摘掉 state fingerprint 里的任何字段来消除 stale
不要在 off 时推进 processed_sequence 或 ContextDelivery snapshot
不要删 setting row、不要新增第二个 fallback 点
不要给 Home 加任何形式的隐式 mark-reviewed
不要用 ContextDeliveryPolicy.enabled 或 assistant.agent=="none" 当智能开关
不要在子 Agent 内部改 src/types.ts / src/api.ts 的既有签名
```

---

## 中文规范

所有新增 UI 文案：

```text
必须中文优先
```

领域术语保持：

```text
Session
Workstream
Project
Agent
Runtime
```

不要出现一半：

```text
继续 Session
```

另一半却写：

```text
Resume Session
```

---

## 代码语言

继续英文：

```text
type
interface
function
DB field
API name
Rust struct
enum
constant
test name
```

只有用户可见文本中文化。

---

# 17. Shared Files

以下文件由 Main Agent 控制：

```text
src/types.ts
src/api.ts
src/app/routes.ts
src/app/AppShell.tsx
src/app/experience.ts        ← §11.9 新增，唯一开关层
src/layout/Sidebar.tsx
src/components/CommandPalette.tsx   ← 原方案漏：里面有 Assistant 入口
全局 CSS
src-tauri/src/lib.rs
src-tauri/src/settings/mod.rs       ← 原方案漏：两个开关的 fallback 都在这
src-tauri/src/ingestion/mod.rs      ← 原方案漏：§11.3 的三条门禁调用点在此有两条
src-tauri/src/sync/mod.rs           ← processed cursor 唯一前进点
```

另外**跨 Agent 的 hook 也算共享**：`src/features/workstreams/useWorkstreamCards.ts` 同时喂 Home（A）和 WorkstreamsView（C）。它在 C 的目录里但 A 依赖它，任何字段增删都要报 SHARED。

如果子 Agent必须改：

1. 最小修改。
2. 独立 commit。
3. 明确标记：

```text
SHARED FILE CHANGE
```

---

# 18. 子 Agent 交付格式

必须返回：

```text
Branch:
...

Base SHA:
...

Commits:
...

Changed files:
...

Implemented:
...

Chinese UI changes:
...

Not implemented:
...

Shared-file changes:
...

Tests:
...

Manual verification:
...

Known issues:
...
```

Main Agent 必须审真实 diff，而不是只看总结。

---

# 19. Main Agent Integration 顺序

建议：

```text
1. Agent D — Launcher
2. Agent B — Sessions
3. Agent C — Workstream
4. Agent A — Home
```

原因：

```text
Launcher
→ 提供 New / Resume 最终行为

Sessions / Workstream
→ 消费 Launcher

Home
→ 最后对齐入口
```

每 cherry-pick 一个 Agent：

```text
解决冲突
build
必要测试
```

不要四个一起 merge。

**补充（v0.1.1）：** 顺序成立的前提是 Commit 0 已经把 §8.1 的四处契约冻住；否则 D 的 `NewSessionModal` 新签名会在 B、C 两个分支里各冲突一次，cherry-pick 顺序救不回来。每合一个 Agent 除了 build，还要重跑一次 §27 里点名的 launcher/delivery 测试组——B 和 C 都会改到调用 launch 的组件，而它们改的是**调用方**，D 改的是**被调方**。


---

# 20. Integration 阶段 Main Agent 额外任务

统一：

```text
routes
API 调用
公共按钮
共享 Modal
Toast
Sidebar
全局导航文案
```

同时做第一轮语言 sweep：

搜索：

```text
New
Resume
Settings
Search
Cancel
Save
Loading
Recent
Active
Archive
Detected
```

检查是否仍有明显英文 UI。

但不要机械替换：

```text
Session
Workstream
Project
Agent
Runtime
```

---

# 21. Wave 2 — Agent E：UX / Language QA

A/B/C/D 合并后启动。

Agent E 从：

```text
integrated main HEAD
```

开始。

---

## Mission

Agent E 不做新功能。

只负责：

```text
体验
一致性
语言
错误状态
空状态
视觉小问题
```

---

# 22. Agent E — 语言审计

需要系统检查用户可见页面：

```text
Home
Sessions
Session Detail
Workstreams
Workstream Detail
Projects
Project Detail
Search
Settings
New Session
Resume Session
Launch Preview
Errors
Toasts
Empty States
```

目标：

> 正常主流程不再出现明显中英混杂。

允许：

```text
Codex
Claude Code
Pi
Session
Workstream
Project
Agent
Runtime
NoEnding
```

其他普通 UI 尽量中文。

---

# 23. Agent E — 必测用户流程

```text
首次启动

无 Agent

一个 Agent

多个 Agent

无 Session

已有 Session

Standalone 新建 Session

Workstream 中新建 Session

Sessions 页面 Resume

Session Detail Resume

Workstream Resume latest

手动绑定 Workstream

移除 Workstream

搜索

筛选

归档 Workstream

超长 cwd

Windows 路径

无标题 Session

Agent 未检测

Session source 丢失
```

---

# 24. 主路径不得出现

普通 Base Experience 中不得出现：

```text
Context Updates
Needs Attention
Mark Reviewed
Sync Extract
Ask Assistant
Context Preview
Conflict Review
```

除非用户主动进入 Experimental / Advanced。

---

# 25. Agent E 可以修改

```text
Empty State
Loading State
Error State
文案
Spacing
Overflow
Button state
Disabled state
Navigation consistency
```

不允许：

```text
新增业务能力
重构 backend
修改领域模型
```

---

# 26. Agent E Commit

建议：

```text
polish(workspace): unify chinese base experience
```

---

# 27. 最终 Build Gate

Main Agent 必须执行：

```bash
cargo fmt --check
cargo check --all-targets
cargo test --all-targets

pnpm install --frozen-lockfile
pnpm build
```

必须全部通过。

**v0.1.1 补两条：**

**1. Commit 0 必须自带回归测试，不是只改行为。** AGENTS.md 明确要求"涉及 ingestion / sync / launcher / resume / storage 的改动要为所改不变式加回归测试"，而 Commit 0 五条全中。至少要三条：

```text
intelligence off：ingest 后 read cursor 前进，processed_sequence 不变
intelligence off：context_items / context_revisions / context_conflicts / sync_runs 行数零增长
intelligence off 且 delivery_level=balanced：仍然不提取（证明两个开关正交，见 §11.1）
intelligence on 复原：冻结期间的积压可被一次 Sync 消费（证明 §7 的 corpus 没被吃掉）
```

第四条是 §7 数据原则唯一可证伪的形式，也是这整个方案里最值得测的一条。

**2. 前端没有任何测试设施**（`package.json` 只有 dev/build/tauri，`src/` 下无 test/spec），并且**没有任何测试断言 UI 字符串**（已核实：Rust 测试只测 DB/命令层）。两个推论：

```text
语言 sweep 不会挂测试 → §28 的人工验收是 UX 的唯一防线，不可跳过
卸载智能 UI 也不会挂测试 → 唯一保护是 §11.9 的机制统一 + Main 审真实 diff
```

---

# 28. 最终人工验收

Main Agent 自己执行：

```text
1. 打开 NoEnding

2. 欢迎页结构基本保持原设计

3. Assistant 不在主导航

4. Home 不显示 Context Updates

5. 主界面语言以中文为主

6. 新建 standalone Session

7. 创建 Workstream

8. 从 Workstream 新建 Session

9. 去 Sessions 找到刚创建的 Session

10. 打开 Session Detail

11. 修改 Workstream binding

12. Resume

13. 退出应用

14. 在外部 Agent 产生 Session

15. 重开 NoEnding

16. Session 被正确发现 / ingest

17. Context extraction 没有运行

18. Context injection 没有运行

19. Workstream / Sessions / Home 正常

20. 无明显中英混杂

21. **从 Workstream 页点新建**：走的是 modal + prepare + 预览，不是直启（§15）

22. **新建后立刻在 Workstream 里改名**，再去点尚未消费的预览：必须被"状态已变化"拒绝，而不是照常启动

23. **不退出应用**，在外部终端新开一个 Agent Session，然后在 Session Detail 点「刷新」：事件应出现（§11.6 的主动摄入入口）

24. 上述全部操作之后查库：`context_items` / `context_revisions` / `context_conflicts` 行数不变，`session_events` 增长，`processed_sequence` 不变（§7、§29）

25. 设置里把 intelligence 打开、delivery 设回 balanced：冻结期间的积压能被消费（证明没丢 corpus）

26. 把 `context.intelligence_enabled` 关掉后，Assistant / Conflict Review / review 深链路由**直接输入**仍然可达（§11.4、§24 的例外条款）
```

---

# 29. 数据保留验收

在 Base Experience 使用期间：

```text
Session
Session Events
Agent
cwd
timestamps
bindings
Projects
Workstreams
```

必须持续保存。

Context Intelligence 关闭不应该破坏未来：

```text
historical replay
Context evaluation
real corpus
extractor testing
```

---

# 30. 当前阶段明确不做

延期：

```text
Context Quality corpus expansion
agent_proposal_not_user_decision
real extractor comparison
prompt optimization
automatic Context extraction
automatic Workstream classification
Context injection
Assistant improvements
Context notifications
Context analytics
model recommendation
full i18n
language switcher
```

---

# 31. 封板标准

## 产品层

完全不使用智能模块，也能：

```text
发现
→ 查找
→ 新建
→ Resume
→ 组织
→ 返回
→ 继续
```

---

## 体验层

用户主流程：

```text
结构简单
语言统一
错误可理解
空状态明确
操作路径短
```

---

## 数据层

NoEnding 持续积累真实 Session 数据，为未来 Intelligence v0.2 做准备。

---

# 32. 阶段结束后的动作

完成后：

> **暂停继续开发。**

进入一段真实 dogfooding。

优先记录：

```text
Session 是否容易找
Resume 是否可靠
Workstream 是否有价值
cwd 是否符合预期
Agent 切换是否顺手
搜索是否够用
Project 是否有价值
界面哪些地方仍然让人困惑
```

未来优先级由真实使用问题决定。

而不是立即恢复 Context Intelligence 开发。

---

# 33. 最终阶段交付报告

Main Agent 最终必须给出：

```text
Core Workspace Experience v0.1

Base Experience:
...

Home:
...

Sessions:
...

Workstream:
...

Launcher:
...

Chinese UI unification:
...

UX hardening:
...

Final HEAD:
...

CI:
...

Manual flows:
...

Known issues:
...

Deferred intelligence work:
...
```

然后进入 dogfooding。

---

# 34. Intelligence Off 后的命令可达性清单

原方案没有这一节，但它决定了 §30 的"保留不删"到底能不能守住。基线 `b1b8efe` 下，off 之后各命令的状态：

| 命令                                                          | Off 后         | 说明                                  |
| ----------------------------------------------------------- | ------------- | ----------------------------------- |
| `sync_all` / `sync_source` / `reingest_source`（:927/:945/:974） | **仅摄入**       | 走 §11.3 门禁，不再进 prepare/extract/commit |
| `sync_session`（:1001）                                       | **仅摄入，仍被前端使用** | §11.6 的「刷新」，不要当成不可达删掉               |
| `list_sync_runs`（:1013）                                     | 不可达           | 只有 Context & Sync 实验区读              |
| `get_workstream_context`（:546）                              | 仍调用，只读         | §14 只用 workstream / project_name / related_sessions |
| `get_context_revision_source`（:585）                         | 不可达           | provenance 跳转随 Context UI 一起卸载      |
| review 五支（:593/:601/:609/:620/:630）                         | 不可达           | 见 §14 的 ReviewState 冻结后果            |
| conflict 五支（:639/:650/:658/:677/:693）                       | 不可达           | ConflictReviewModal 不挂载             |
| `get/set_context_delivery_level`（:357/:364）                 | 仍可达           | 实验区要能回开                            |
| prepare / launch / cancel / resolve 六支（:1039-:1205）         | **完全不变**      | off 只影响 bundle 内容，不影响完整性链           |
| `launch_new_session`（:1058）/ `launch_resume_session`（:1079） | **前端零调用点**    | 集成后四条直启全改为挂 modal；命令与 `api.*` 包装按 §5 保留（见 §36.6 第 1 条） |
| `search`（:1288）、sessions/binding（:719-:908）、sources（:1234-:1287） | 不变            | §6 要求 ON 的部分                       |

一句话：**off 只砍"写 Context"和"读 Context 的智能视图"，launch 链一条都不动。**

---

# 35. v0.1.1 勘误清单

审的是 `b1b8efe`。F1–F4 是需要拍板的分叉，其余按证据直接改写。

| #   | 原方案位置           | 现状证据                                                     | 处置                      |
| --- | --------------- | -------------------------------------------------------- | ----------------------- |
| F1  | §11.2 默认 Off     | 缺行→Balanced：`settings/mod.rs:15` + `context/mod.rs:28-32`；老库无行 | 迁移为老库显式写 off 行；§11.2      |
| F2  | §11.1/§11.2 关系  | 不变式要求 delivery off 不禁提取；`ContextDeliveryPolicy.enabled` 是派生态 | 两开关正交，单 accessor；§11.1    |
| F3  | §15 Workstream 发起 | 直启绕过 prepare：`WorkstreamDetailView.tsx:161-171`；modal 无 wsId 入参 | 统一 prepared flow，归 D；§15  |
| F4  | §11.4/§11.7/§12 三种卸载做法 | 无统一机制                                                     | Commit 0 建 `experience.ts`；§11.9 |
| F5  | §11.3 STOP 落点   | 三条耦合路径：`ingestion/mod.rs:81` / `:61-77` / `launcher/mod.rs:1090-1120` | 单一共享门禁；§11.3             |
| F6  | §7 游标可保留（假设）     | `processed_sequence` 唯一前进点 `sync/mod.rs:472`，read cursor 独立 | 假设成立，写明；§11.3            |
| F7  | —               | 假门：`sync/mod.rs:310-312` 空批次仍 commit 并前进游标                   | 禁止复用；§11.3               |
| F8  | §7 重开时积压        | 无任何描述                                                     | 新增分批 + 加载中语义；§11.3        |
| F9  | §15 Preview 隐藏  | off 时仍渲染 "0 tokens"/"Off · 不注入"：`NewSessionModal:117-144`、`Resume:245-271` | 主动删文案；§15                |
| F10 | §15 保留 integrity | `ResumeSessionModal:53-81` mount 即 eager prepare，`:83-91` cancel | 只改可见性；§15、§16            |
| F11 | §15 是否需要后端分支（假设） | off 早退空 bundle `context/mod.rs:248-258`，快照按 `ctx_file.is_some()` | 假设成立：纯前端；§15             |
| F12 | §14 重命名/编辑描述    | UI 不存在（`:209` 静态文本；描述只读 `WorkstreamCard.tsx:60`），后端 `update_workstream` 已支持 | 标明为新增工作量；§14             |
| F13 | §14 Lifecycle   | 无切换命令；`"open"` 固定（`commands.rs:157`），badge 只读               | 降为只读；§14                 |
| F14 | §14 编辑（隐含）      | `title/description/updated_at` 在指纹内 `launcher/mod.rs:475-478` | rename 致 PreparedLaunch stale；§14、§28 |
| F15 | §12/§14 ReviewState | `SinceLastReview.tsx:142-149` 是唯一 mark-reviewed 入口          | 知情接受，禁隐式推进；§14            |
| F16 | §11.4 Assistant 入口 | 实际四处：`Sidebar:85-88`、`CommandPalette.tsx:20`、`WorkstreamDetailView:212-215`、`ProjectDetail:50-51` | 四处全走开关；§11.4              |
| F17 | §12 hook 重构顾虑   | `reviewSummaries` 唯一消费者 `HomeView.tsx:85`，删 3 行             | 顾虑删除；§11.5                |
| F18 | §11.6 移除同步提取    | 移除后无主动摄入入口（`sync_session` 是唯一手动路径）                          | 改名「刷新」保留；§11.6            |
| F19 | §2 词表           | Primary/Related 已在 `SessionDetailView:182-184` 使用；Lifecycle/Sources/Refresh 缺 | §2.1 补全                 |
| F20 | §13 vs §14       | `最近活动`(Session) 与 `最近更新`(Workstream) 会被子 Agent 合并            | §2.2 定死                 |
| F21 | §12 Hero 文案      | Sidebar 已中文 `:67`，Home 仍英文 `:41`                            | 统一中文；§2.3、§12             |
| F22 | §11.8 语言分工      | Sources ~13 + Projects ~13 无人认领                             | Main 集成阶段自扫；§11.8         |
| F23 | §27 Gate        | 前端零测试设施、零 UI 字符串断言                                        | §28 是唯一 UX 防线；§27        |
| F24 | 全方案             | Commit 0 未要求写回归测试，违反 AGENTS.md 测试条款                         | §27 四条必测；§27              |
| F25 | §8/§10 执行模型     | worktree ×4 = 4 次冷 cargo 构建；`node_modules` 不在 git           | 默认两批串行；§8.2               |
| F26 | §10 ownership   | B/D、C/D 跨文件调用，A/C 共用 `useWorkstreamCards`                   | Commit 0 冻契约；§8.1        |
| F27 | §11.2 影响面       | `launch_context_test.rs:1012` 断言默认 Balanced，另 5 支依赖该前提       | Commit 0 一并改；§11.2       |
| V1  | §15 cwd 假设      | `default_cwd` 端到端已存在（schema `storage/mod.rs:167`、`domain/models.rs:49`、`types.ts:31`、UI `WorkstreamDetailView:193-200`） | 假设成立，无需扩方案 |
| V2  | §6 后台行为        | 启动顺序：`lib.rs:74` 建索引 → `:93/:95` SyncEngine + reconcile     | 与 §11.3 门禁位置一致 |

---

# 36. 实施落地记录

实施与本文不一致时在此追加，不得让文档与代码互相矛盾。

## 36.1 Commit 0 — `refactor(workspace): default to base experience mode`

基线 `b1b8efe`。`cargo test --all-targets` 198 passed / 0 failed / 5 ignored，`cargo fmt --check` 与 `pnpm build` 通过。

**与方案不同或方案未覆盖之处：**

| # | 方案 | 实做 | 原因 |
| --- | --- | --- | --- |
| L1 | §11.3 要一个共享门禁函数 `context_processing_enabled()` | 不新增包装，三处直接调 `settings::context_intelligence_enabled(db)` | 包装只是同义改名；判断点仍只有一个（§16"不新增第二个 fallback 点"） |
| L2 | §11.2 新库不写行、老库 migrate 写行 | migrate 对**所有**缺行库写 `off`，代码 fallback 同时改为 Off | 二者本来就分不出新老库（`PRAGMA user_version` 对新库也是 0）。结果：行恒在、可审计、可回开，fallback 只兜"行被删" |
| L3 | — | `run_pending_sync_nonblocking`（`sync/mod.rs:515`）**未加门禁** | 无生产调用方；加门禁要顺手改 `sync_integrity_test`，属推测性改动。将来若接进生产路径必须补门禁 |
| L4 | §11.6 只说删掉同步提取 | `sync_session` 响应扩为 `{ applied, ingested, context_processing_enabled }`，Session Detail 的按钮改名「刷新」并保留 | §11.6 的主动摄入入口；这是给 B 的契约 |
| L5 | §11.4/§11.7 未规定设置节怎么做 | 直接从 `SettingsSection` union 里删掉 `"sync"`（不是隐藏），Context 注入与自动化并入「数据与高级」 | 路由不持久化，全仓无 `section:"sync"` 跳转，保留死键没有意义 |
| L6 | §11.7 | Automation 三条徽标改为**真状态**：摄入与索引=开（恒定）、提取与归类=跟随智能开关、注入=跟随 level | 原来硬编码三个 On，智能关掉后就是假信息（违反 §31 错误可理解 / §24） |
| L7 | §11.5 与 §12 都写了"移除 ContextUpdatesSection" | Commit 0 用 `<IntelligenceOnly>` 包住，**不删** | §11.9 的机制优先；A 从基线接手时只需做剩余文案与 hook 收尾 |
| L8 | §11.4 列 Assistant 四个入口 | Commit 0 接线 Sidebar + CommandPalette + ProjectDetail 三处，`WorkstreamDetailView:212-215` 留给 C | 后三处无人认领或本就在别 Agent 范围内；C 的分支从含前三处的基线开始 |
| L9 | §11.6 归属 | Commit 0 只改了 §11.6 明列的四处（doSync 文案、:59、:90、:109），`Resume`/`Edit`/role 徽标/BindingModal 全部留给 B | 避免吃掉了 §13 的活，B 的 ownership 才成立 |
| L10 | §11.1 | 新增 `get/set_context_intelligence_enabled` 两支命令，前端 `api.ts` 同步 | 实验区要能回开；也为了 §27 的测试可用命令层验证 |
| L11 | §11.9 文件名 `experience.ts` | 实为 `experience.tsx`，并导出 `useDeliveryOff()` 供 §15 用 | 含 JSX |

**新增测试（`src-tauri/tests/base_experience_test.rs`，6 支）：**

```text
intelligence_is_off_until_explicitly_enabled        缺行=off，非显式值不得开智能
migration_pins_delivery_level_but_not_intelligence  L2 的两半各测一次
off_stops_after_ingestion_on_the_launch_and_refresh_path
                                                    摄入/索引/read cursor 前进，processed 与 4 张 Context 表全冻结
off_stops_after_ingestion_on_the_reconcile_path     同一个门的第二处（reconcile 用的非阻塞孪生）
delivery_level_never_gates_extraction               两个开关正交：balanced+off 智能 不提取；off 注入+开智能 仍提取
backlog_ingested_while_off_is_replayed_after_reenabling
                                                    §7 唯一可证伪形式：Off 期间的 corpus 重开后一次消费掉
```

**改动的既有测试（`launch_context_test.rs` 4 支）：** 默认值断言改为 Off 并加"缺行不得复活注入"；三支原本隐含依赖"默认 Balanced"的用例改为**显式**设 Balanced——它们测的是 prepare/指纹语义，不该测默认值。

**Commit 0 未做（按 §12–§15 归属）：** Workstream Detail 的智能段落卸载（C）、New/Resume 的 Context Preview 隐藏与 prepared flow 统一（D，含 §15 默认处理清单的第 5 条）、Sessions 页面打磨（B）、Home 其余文案（A）。

## 36.2 Agent D — `feat(launcher): streamline base new and resume flows`

分支基线是 `5092e46`（含 `c684869`）。§15 的 D 范围已落地：`NewSessionModal` 增加 §8.1.1 的 optional `workstreamId`、打开即 prepare、显示 `prepared.cwd`、启动只走 `prepareNewSession → launchPrepared`；Delivery Off 时 Context Preview 整块不挂载。与方案的差异：

| # | 方案 | 实做 | 原因 |
| --- | --- | --- | --- |
| D1 | §15「纯前端改动，backend 不需要新分支」 | 仍然成立；但 `launcher/mod.rs` 多了一个 `launch_prepared_with(db, prepared, spawn)`：把**进程 spawn 这一步**做成注入参数，`launch_prepared` 原样委托给它 | AGENTS.md 要求为改动的 launcher 不变式写回归测试，而 §27/§35 的 F24 只补了智能侧。真实 `platform::launcher::launch` 在 macOS 上会开 Terminal 打字，测试不能这么跑。所有完整性门禁（level 校验、状态指纹、ctx 文件门控、LaunchIntent、delivery 快照）都留在原路径里，注入只替换最后那一脚 |
| D2 | §15 只点名 `launchNewSession` 直启 | Resume 侧 `launchResumeSession` 兜底也删了：没有 prepared 令牌时「继续」按钮禁用 + 提供重试 | 该兜底内部虽是 `prepare_resume + launch_prepared`（不是完整性旁路），但它启动的参数和面板上预览的 `prepared` 不是同一份，违反 Preview-Launch Identity 的显示一致性 |
| D3 | §15「不显示 0 tokens / No context / Context disabled」 | token 估算在 `ContextPreviewModal` 与 `LaunchResultModal` 里**整体移除**（不只是 off 时隐藏） | 唯一能读出"零"的地方就是这两处；实验区仍可用「条目详情 (N)」看真实内容量 |
| D4 | §11.9 | 两个 modal 改用 `useBaseExperience()` 读 level，不再自己 `api.getContextDeliveryLevel()`；顺带修掉一个既有 bug：它们把 level 的初值写成 `balanced`，而真实默认是 off，于是 off 时也会先渲染一版"注入中"的提示 | 读一次、判一处 |
| D5 | §2 词表 | `ContextPreviewModal` 不再 import `RuntimeIntentBadges`，改用 launcher 内部的 `runtimeIntentText`（"Agent 默认值"） | `AgentRuntimeSettings.tsx:41` 的「全部 Agent default」不在 D 的 ownership 内，没动它；那处仍待 Main/Agent E |
| D6 | §15 第 4 条（cwd 可编辑） | 只读显示 | 开放编辑要新增 backend 入参，方案自己标为后续可选项 |

**新增测试（`src-tauri/tests/base_launch_flow_test.rs`，7 支）：** standalone（`workstream_ids = []`）prepare → launch_prepared 成功且 `context_deliveries` 恒 0、不写 context 文件、intent 不声称交付过 bundle；off + 有 Context 的 Workstream 同样零注入；`delivery_level` off→balanced 与 balanced→off 双向都判 stale 且不提交 intent；prepared 令牌 single-use；`prepared.cwd` 就是后端解析结果；off 的 bundle 字面为空。

**D 未做（不是遗漏）：** `WorkstreamDetailView.tsx:165`、`WorkstreamCard.tsx:38`、`WorkstreamSessions.tsx:30`、`HomeView.tsx:31` 四条直启路径在 C/A 的文件里，§8.1.1 已给出接法（挂 `<NewSessionModal workstreamId={ws.id}/>`）；`launchNewSession` / `launchResumeSession` 命令与 `api.*` 包装按 §5「保留代码」原样留着，集成后它们在前端无人调用，§34 的不可达清单要收这四条。

## 36.3 Agent B — `feat(sessions): strengthen session browsing and detail experience`

分支基线 `5092e46`。只动 SessionsView / SessionTable / SessionDetailView / SessionMessage，未碰 Rust。

| # | 方案 | 实做 | 原因 |
| --- | --- | --- | --- |
| B1 | §13「移除 Context 变更」 | `SessionDetailView` 里那句「提取了 N 个 Context 变更」**保留**，但只在 `context_processing_enabled && applied > 0` 时出现 | Base Experience 下后端恒报 false，主路径不出现（§24）；用户主动回开智能时，不报真实提取数才是更坏的失败。Main 采纳 |
| B2 | — | 列表原先显示 `sBindings[0]`（多绑定时是任意一条），改为主关联优先 + `+N` | 与 §2.2「主关联」同源的显示错误 |
| B3 | — | 三个 fire-and-forget 请求合成一次 `Promise.all` + 显式「读取 Sessions 失败 / 重试」 | `listSessionBindings` 静默失败会让「未关联」筛选给出**错误答案**，不只是少数据 |
| B4 | §13 字段表 | Session ID 进 Header 且可选中复制 | §11.6/§24 的 provenance 要求：报障时得能给出 id |
| B5 | — | 省略号规则（保尾省中、Windows 盘符与 UNC、CJK 宽度）集中在 `SessionTable.tsx` 导出复用 | 为了不出 ownership；Main 认可，若再有第 5 个消费者则提 `sessions/display.ts` |
| B6 | — | `get_session_detail` 每次上限 500 条事件，消息段落改为说明"读到了多少" | 只改显示诚实度；真分页要动 `api.ts` + Rust，超范围 |

**Main 在集成时改掉的一处 bug（B 的注释与代码不符）**：`ellipsisPathMiddle` 承诺「POSIX 段名里的 `\` 不当分隔符」，但 split 用的是 `/[\\/]+/`，真溢出时会把 `/tmp/my\dir/weird/sub` 切成多段、编造出层级（宽度 18 实测应为 `…/my\dir/weird/sub`）。改为只有盘符与 UNC 两种分隔符通吃。见 `fix(sessions): keep a POSIX backslash inside a segment name`。

## 36.4 Agent C — `refactor(workstreams): focus workstream detail on session continuity`

| # | 方案 | 实做 | 原因 |
| --- | --- | --- | --- |
| C1 | §15/§14 只点名 3 条新建直启 | 其中一条实为 `launchResumeSession`；C 另把 3 处 Resume 直启（Detail / 卡片 / 每行）改挂 `ResumeSessionModal` | 与 D2 同一理由；集成后 `launch_new_session` / `launch_resume_session` 在前端**零调用点**（已 grep 验证，仅剩 `api.ts` 的包装） |
| C2 | §14「只是暂时不挂载」 | 智能段落搬进同文件内的私有组件 `IntelligenceSections`，再由 `IntelligenceOnly` 包住 | 直接包 JSX 时 review 那几支请求仍在 mount 时发出；§34 要的是"不可达"而不是"发了再藏"。代码一行未删，Review 逐行保留（含 `reviewWindow.mark_through` 与 `entry==="conflicts"` 一次性语义） |
| C3 | §14 五段结构 | Base 内容单列，`ws-detail-grid` 只留给智能段落 | 右栏卸载后只剩一段，双列会留大片空白；未新增未修改任何 CSS |
| C4 | §14 编辑能力 | 新增「重命名」与描述行内编辑，走既有 `update_workstream`，**没有新增后端命令**；改名弹窗明说会让待启动的计划失效（⚠️ 该弹窗与提示已由 §36.10 撤下） | F12/F14：指纹含 title/description/updated_at，stale 是正确行为，不掩盖 |
| C5 | §14 Lifecycle | 只读 badge，无状态切换 | F13 |

C 交给 Main / E 的遗留：`WorkstreamCard` 摘要优先级仍是 `current_state || description || goal`（Agent 推断压过用户自己写的描述）；`.ws-card-error` 变成死 CSS 规则；`toggleArchive` 无错误处理；••• 菜单无点击外部关闭。

## 36.5 Agent A — `refactor(home): align home with base experience`

Home 摘掉 `reviewSummaries` 依赖、空状态「新建 Session」改挂 `NewSessionModal`（§15 第 5 条点名的第四条直启路径）、结构与 Commit 0 中文案原样保留、组件文件零删除。

**Main 采纳的一个取舍（A 主动上交的矛盾）**：§11.5/§12 说 Home「只移除 ContextUpdatesSection」，而 §11.9 的机制是 off=不挂载、on=挂载的对称。Home 按 §11.5/§12 字面执行——**即使把智能回开，Home 也不再显示 Context Updates**；智能段落的挂载点保留在 Workstream Detail（C2）。理由：§24 把 Context Updates 归为"非主路径"，而本阶段 Home 的定位就是纯继续入口。要回退是 5 行的事（重新解构 + 重新包 `IntelligenceOnly`）。

A 的其他遗留：Home 里「配置 Agent」链接在 `getDefaultAgent()` 解析期间可能闪现（要共享 hook 加 resolved 标志，属 SHARED）——该链接后来被 Agent E 移除，此条已作废；新建 Session 比原来多一步弹窗（这是 Preview-Launch Identity 的预期代价，进 dogfood 观察项）。

## 36.6 Main 集成决策（合完 D/B/C/A 之后）

```text
1. 直启清零：src/ 下 api.launchNewSession / launchResumeSession 调用点为 0（只有 api.ts 定义），
   §34 的不可达清单据此补上这两支命令（后端与包装按 §5 保留）
2. reviewSummaries 请求：useWorkstreamCards() 改为跟随智能开关才发
   fix(workstreams): stop fetching review summaries while intelligence is off
   （返回形状不动，关掉时留 null）→ §34「review 五支不可达」现在是真的
3. 预览令牌泄漏（D 自己报的 Known issue #1）：ContextPreviewModal 在预览打开期间
   注入被关掉时，先 cancelPrepared 再交回 onClose
4. 启动详情的「注入的 Context」改为按这次实际投递的 markdown 决定显隐，不读当前设置
5. 卡片摘要优先级 current_state > description：当时判断「不改」（旧《前端整体设计
   方案》§4 的显示优先级），**已被独立复核推翻**，见 §36.9。理由：这与本阶段的
   产品定义直接冲突——Base Experience 下 Workstream 应由用户显式组织的信息驱动。
6. .ws-card-error 死规则、AgentRuntimeSettings 的「全部 Agent default」、AssistantView
   三处 "Settings → Agents"、Sources/Projects 两页英文：交 Agent E
```

## 36.7 Agent E — Wave 2 语言与体验审计（代提交）

**E 没有按 §18 交报告**：它在子 Agent 的 150 轮上限处中断（177 次工具调用、约 29 分钟），
21 个文件的改动停在**已暂存、未提交**状态。Main 逐项审过 `git diff --cached` 后代为提交
（`913241f polish(workspace): unify chinese base experience`）。这意味着 §22/§23 的**覆盖面
是 Main 事后核的，不是 E 自报的**——E 原本还打算改什么、有没有中途放弃的项，无人知晓。

Main 的核对结果：

```text
未删除任何智能组件文件；未移除一处 IntelligenceOnly 门控（diff 里 0 行）
前端 launchNewSession / launchResumeSession 调用点仍为 0
未碰 全局 CSS / src-tauri/** / src/types.ts / src/api.ts      → 无需重跑 cargo 门禁
pnpm build 通过
```

E 实际做完的（Main 认可）：

```text
逐页语言 sweep（Sources / Projects / Project Detail / Search / Settings / Assistant /
  launcher 预览行 等 21 个文件），领域词未被机翻
三处静默失败修成显式错误：Sidebar 建 Project、ProjectsView 建 Project、
  NewWorkstreamModal 创建失败（原先 await 抛错就无声返回，用户以为已建成）
SessionsView 空状态区分「未启用任何 Session 来源」与「已启用但没发现 Session」，
  并用 source.exists 提示来源路径缺失（B 的 Known issue #4、§23 的 source 丢失）
Modal 外壳：Escape 关闭，且只有最上层响应；键盘退出仍走 onClose，
  因此不会漏掉释放 PreparedLaunch 令牌
••• 菜单补点击外部关闭（C 的遗留）
「全部 Agent default」→「Runtime：Agent 默认值」；AssistantView 三处
  "Settings → Agents" → 「设置 → Agent」（D 的 D5 遗留）
```

合并后 Main 自己再扫了一遍主路径，剩余英文只有：`Override`（Agent Runtime 的显式覆盖标记，
与封板方案用词一致，保留）、`Unknown view`（Router 的未知路由兜底，非正常主路径）、
以及 Projects / Workstreams / Sessions 三个作为导航标签的领域词（§1.2 允许）。

**§2.1 补两个 E 期间定下的词：**

```text
Override（Agent Runtime 的显式覆盖标记）→ 保留英文，与 Runtime 方案文档同一术语
Empty/loading 提示里的 Ingest Source → Session 来源
```

## 36.8 集成后端与门禁现状

```text
main tip           df76634（含 Commit 0 + D + B + C + A + E）
cargo fmt --check  通过
cargo check        通过
cargo test         205 passed / 0 failed / 5 ignored（基线 198 + D 的 7）
pnpm build         通过
```

仍未收口的项（都是 Main 名下的小账，不阻塞封板）：

```text
.ws-card-error 成为死 CSS 规则（全局 CSS 归 Main，未删）
（原列于此的）HomeView「配置 Agent」链接解析期闪现：已不存在。Agent E 把
  Home 的新建入口改成常驻按钮 + 弹窗内报错，该链接连同它的问题一起被移除（§36.9 复核）
新建 Session 比方案冻结前多一步弹窗 —— Preview-Launch Identity 的预期代价，进 dogfood 观察
run_pending_sync_nonblocking 仍无门禁（L3：无生产调用方；若将来接进生产必须补）
```

## 36.9 封板前收口 — `fix(workstreams): keep base experience independent from context summaries`

独立复核指出一个与 §14 产品定义冲突的残留：`intelligence_enabled = false` 时，
冻结期写下的 `current_state` 仍在主路径上冒充 Workstream 的自我介绍。
本阶段的定义是「Workstream 由用户显式组织的信息驱动」，因此这不是显示偏好问题。

规则集中成三处调用、一处判定（`WorkstreamCard.tsx` 导出）：

```text
cardSummaryLine(card, intelligenceEnabled)
  off → card.description          （只有用户自己写的描述）
  on  → current_state || description || goal   （原 §4 优先级完整回来）

cardSearchFields(card, intelligenceEnabled)
  off → title / description / project_name
  on  → 再加 current_state / goal

searchFieldHint(intelligenceEnabled)
  placeholder 由同一份字段集派生
```

判定门一律取 §11.9 的 `useBaseExperience()`，不是新开关。

三个消费点：

```text
WorkstreamCard.tsx        卡片摘要（Home 的 ContinueSection 同源，一起修好）
WorkstreamsView.tsx       检索字段 + placeholder 文案
ProjectDetail.tsx         Project 页的 Workstream 行原先无条件渲染
                          (w as any).current_state —— 复核未点名的第三处，同类同修
```

**为什么 placeholder 必须一起改**：原先搜索实际命中
`title / description / current_state / project_name`，提示却写「标题、描述、Project」。
Base Experience 下会出现"搜到了、页面上却看不到任何命中这个词的内容"——
用户拿到一个无法解释的结果。字段集与声明由同一个函数派生，杜绝再次漂移。

同时纠正两处文档/注释与代码不符：

```text
storage/mod.rs v11 迁移注释里的「pre-migration behavior (implicit balanced) is
  recorded here」与实际动作矛盾（实际是把旧的隐式默认**改写成**显式 off 行），
  按复核建议重写为 intentionally converts the old implicit default to explicit Off，
  并补上「用户已选过的行不覆盖」「删行后仍读作 Off」两句事实。
§36.5 / §36.8 里 Home「配置 Agent」链接闪现一条已标注作废：Agent E 把 Home 的
  新建入口改为常驻按钮 + 弹窗内报错，该链接不再存在。
```

未做：`WorkstreamContext.tsx` 里的 `Current State` 标签与排序不动（它在
`IntelligenceOnly` 内，智能关闭时不挂载）；`goal` 的字段语义与存储一律不改。

## 36.10 编辑能力合并 — 一个「编辑任务」弹窗

dogfood 反馈：任务详情页右上角的「编辑描述」改成「编辑任务」，弹出与**新建任务
同一个弹窗**用于编辑当前任务，页面上的其它编辑入口一并撤掉。落在
`WorkstreamFormModal.tsx`（原 `NewWorkstreamModal.tsx`）——创建与编辑共用一个
组件，传 `workstream` 即为编辑模式，所以「一样」是构造上保证的，不是两边对齐的。

```text
••• → 编辑任务…          新增，取代「重命名…」与「编辑描述…」两项
任务概览的行内编辑         删除（铅笔按钮 + textarea 一起走，描述只读展示）
「重命名任务」弹窗         删除（标题在同一个弹窗里改）
「工作目录」面板           转为只读：新增 / 选择已有 / 移除 / 设为主要 全部删除，
                          第 1 条补一枚「主目录」徽标（顺序就是角色，§1.5）
其它动作                   不动：状态切换、新建 / 继续会话、归档 / 恢复 / 永久删除
```

保存时的命令拆分 —— **没有新增后端命令，也没有新增领域概念**：

```text
标题 / 描述   update_workstream（整对象写；description 在弹窗里 trim，
              因为创建路径是 trim 后落库的，而 apply_whole_object_edit 不规范化）
工作目录      add / remove / reorder 三个既有命令
```

两个与 §14 / C4 一致、但值得写下来的实现选择：

```text
1. 工作目录按「认领 → 解析 → 移除」三步落地，每一步只有一个权威：
   认领  草稿里与已有行 canonical_path 逐字相同的那条，直接用已有行的 id。草稿行
         就是从 canonical_path 预填的、用户改不了它的拼写，所以这是身份而不是猜测。
   解析  只有用户这次新加的路径才交给 add_workstream_path —— 拼写归一与 identity
         由后端决定，前端不猜 canonical。
   移除  只针对「用户真的从列表里拿掉」的路径。某一次解析失败不是删除的理由：那条
         路径留在原地，改动如实报告。否则一条已附上的目录会因为「此刻解析不出来」
         （例如 Home 搬家后它落进了自留目录）在保存时被静默移除并解绑它的会话。
2. 路径一个字都没动时（草稿与 canonical 列表逐项全等）一个路径写命令都不发：
   只改标题不该顺手碰工作目录，也不该把「状态已变化」的面无谓扩大。
3. 差分用的是**弹窗打开那一刻**的列表快照，不是会跟着后台刷新变的 props：否则
   别处刚附上的一条路径会因为「不在草稿里」被当成用户拿掉了它而静默移除并解绑
   会话。取快照后最坏只是 reorder 因集合不完整而报错 —— 宁可失败得难看。
```

跟随 §14「不能确定就不猜」：解析不到的路径不静默丢掉 —— 其余改动照常落盘，
弹窗转成报告逐条说破（与创建时的报告同一个形状，只换文案；两种拼写指向同一条
目录也按创建时的口径报成「与前面一条指向同一目录」）。移除已有路径会在保存前
点名「会解除 N 个会话关联」，这条提醒与保存时的移除判据是同一个（草稿里彻底看
不见），所以不会多提醒也不会少提醒。

**撤下的一条提示（§36.4-C4 的对应物）。** 旧「重命名任务」弹窗里那句「改完会让已经
预览、还没启动的会话变成『状态已变化』」没有搬进新弹窗，而且**永不再搬**：它描述的
交互在这套 UI 里不可达。弹窗背板是 `position: fixed; inset: 0`（`global.css:157`），
启动弹窗开着时页面上的 ••• 点不到（第一下点中的是背板，而背板就是关闭）；而那份
PreparedLaunch 只活在启动弹窗开着期间 —— 弹窗一关，`usePreparedLaunch` 的卸载清理就
`cancelPrepared` 掉了（`usePreparedLaunch.ts:56-63`）。所以等你能进编辑弹窗时，手里已经
没有会被编辑搞成 stale 的计划；真正会触发 stale 的是后台状态变化（如同步摄入新内容）。
指纹校验本身一点没动，仍然在启动弹窗里照常拦。

未做：有序路径的语义、`workstream_paths` 的 schema 与命令一律不动；
`PathListEditor` 的草稿行仍是只读展示（只能新增 / 移除 / 换序）；Project 仍只从
主工作目录派生。

已知的呈现代价（留给下一次 wording pass）：`PathListEditor` 的徽标仍写「主路径」，
只读面板写「主目录」；旧文里出现的 `NewWorkstreamModal.tsx` 指的是同一个组件。

---

## 36.11 撤下工具事件 — 不再摄入 `tool_call` / `tool_result`

**起因是用户的一个观察**：老会话有「工具调用 / 工具结果」，新会话几乎没有。查证后发现
是两个叠加的缺口，而不是哪一版把它们排除了：

```text
1. codex.rs 只映射 function_call / function_call_output。当前 Codex 写的是
   custom_tool_call / custom_tool_call_output（调用参数在 input 而不是 arguments），
   落到 `_ => ("unknown", String::new())`；而 read_jsonl_delta 丢弃文本为空的解析
   结果（adapters/mod.rs:373-380）—— 连一条 unknown 都留不下。
2. 即使走老路径，function_call_output.output 现在是内容块数组
   [{"type":"input_text","text":…}]，`as_str()` 取不到 → 空文本 → 同样被丢。
```

实测 `f982539d-…`：转录里 340 + 340 条 custom 工具事件，库里只有 3 条 `tool_call`、
0 条 `tool_result`。库里全部 821 / 335 条工具事件都来自 6 月那批 `output` 仍是纯字符串
的老转录 —— 所以「越老越有」正是这个规律。

**决定（用户：不保留工具调用和工具结果）。** 不补 custom_* 的映射，而是反向收口：三个
adapter 一律不再产出这两类事件。

```text
codex   function_call(_output) 与 custom_tool_call(_output) 四类 payload 一律 return None
claude  content_text 不再把 tool_use / tool_result 折进消息文本（原 "[tool_use:name] …"）
pi      content_text 只返回文本（原返回 (text, tool_parts) 并把 "[tool:name] …" 追加回
        所在消息），read_delta 里的追加逻辑随之删除
```

Claude / Pi 的工具内容本来就不作为独立事件，而是拼进所在消息的文本里，所以这两处是
「从消息文本里摘掉」，不是「停发事件」。摘掉后，纯工具调用的消息只剩空文本，会被既有的
空文本过滤整体跳过 —— 这正是想要的结果。

**pi 的等价物。** pi 不发这两类事件：`toolCall` 块被折进所在消息的文本（§36.11 一并摘掉），
而工具结果是一条 **`toolResult` 角色**的消息，落到 `_ => ("system", …)` —— 也就是说 pi 的
工具结果一直以「系统」事件的形式在库里（1293 条 `system` 里混着它们）。这次同样按 role 挡掉
（`pi.rs` 里 `"toolResult" | "tool_result" => return None`）。

**历史行已清除（2026-09-23）。** 备份后删掉 1156 行（`tool_call` 821 + `tool_result` 335）
以及 `search_index` 里对应的 1102 行（其余 54 条文本不足 20 字，本就没进索引）；
`session_events` 从 6701 降到 5545，孤儿索引 0。删除不影响游标：`session_cursors` 存的是
源文件链的尾哈希，与已存行无关，后续追加照常。副作用是 codex 会话的 `sequence` 出现空洞
（序列由 app 分配、只增不减，UI 上的 `#序号` 会跳号）。`SessionMessage.tsx` 的
「工具调用 / 工具结果」标签随之删掉（已无产出方）；`domain::SessionEvent.kind` 仍是开放
字符串，schema 不变。

**pi 的摄入已整体重置（2026-09-23）。** 上一轮说 pi 那些工具结果「无从区分真伪」是错的：
事件的 `source_position` 就是转录行号（`line:N`），所以完全可以反查。但既然要重读，就没必要
逐行挑：删掉 pi 的 `session_events`（2626）+ `session_cursors`（12）+ 索引行（2459），留下会话
行本身，下一次 reconcile 就会把 12 份转录整份按新规则重读 —— 会话 id、标题（write-once）、
Workstream 绑定全部不受影响，也不存在身份链错位（全量重扫从 genesis 起算，且该会话已无旧行）。
备份 `noending.db.bak-pireingest-20260923-004523`。

这也回答了一个操作上的判断：**要「重新录入」时，该删的是游标 + 事件，不是会话行。** 删会话行会
换掉 id、让 `session_workstream_bindings` / `launch_intents` / `context_deliveries` 里的引用悬空
（schema 里 `session_id REFERENCES sessions(id)` 没有 ON DELETE CASCADE，不会自动清），而收益
完全一样。

**已知代价。** 之后无法回答「这次会话动了哪些文件 / 跑了什么命令」；工具输出里偶含用户
粘贴的内容，也不再进 Context 提取的候选。若要恢复，正确做法是补齐 custom_* 的 payload
形状，而不是恢复事件前缀过滤。

---

## 36.12 页头并入应用标题栏

**用户要求**：把各页面内容区的那一行标题（标题 + 右侧动作）搬到应用自己的标题栏里；标题行里原本是文字的按钮改成图标按钮。

**为什么可以搬。** 窗口本来就是 `titleBarStyle: Overlay` + `hiddenTitle: true`（`tauri.conf.json`），系统标题不显示，App 自己在顶部画了一条 48px 的 `.app-titlebar`，里除 macOS 拖拽区和三个历史按钮外全是空的。

**怎么搬：不动 React 结构，改 `PageHeader` 内部顺序 + CSS 吸顶。** 没有给 AppShell 加"标题上报"通道（那需要 context + portal，还要防重渲染死循环），而是：

```text
PageHeader   标题行（h1 + 动作区）排在返回链接 / 副标题 / children 之前 —— 顺序不能倒，
             吸顶的必须是第一个元素
layout.css   .app 去掉 padding-top，那 48px 交给页面自己；.sidebar 用 margin-top 让开
             （用 margin 不用 padding：侧栏背景才不会涂进标题栏那条）
             .main 去掉顶内距，.page-head 成为 sticky、top: 0、48px 高、负 margin 抵消
             .main 的左右内距（--main-gutter），背景盖住滚过去的正文
             没有页头的页面（加载中 / 错误态 / Assistant）靠 :has() 规则自己让出 48px
```

**踩到并修掉的两个坑**（都在静态预览页里实测过，不是推演）：

```text
1. sticky + 负 top margin 会被"粘性盒不得越出包含块"钳回去，标题行停在 y=48。
   改成 .main 不留顶内距，标题行天然就在 y=0，sticky 才成立。
2. .history-controls 与 .page-head 同为 z-index 6，而标题行在 DOM 里更靠后，
   侧栏收起时（.main 顶到 x=0）标题行的背景会盖住那三个按钮 —— 提到 z-index 8。
   （当时还写了 `-webkit-app-region: drag` 想让标题行"自己承担拖动"，那是错的，
   见 §36.14——这个属性在 WKWebView 里不生效，窗口拖动一度就是这样坏掉的。）
```

**对齐**：标题左边缘与正文左边缘同源（都用 `--main-gutter`），动作区右边缘与正文右边缘同源；窄屏（≤1100px）只改变量，两处自动一起走。侧栏展开时标题行从侧栏右缘起算，天然错开历史按钮；**收起时标题行内缩 `--titlebar-gutter + 16px`（mac 195px / 非 mac 120px）**——这是搬运不可避免的代价：那条 band 与历史按钮共用同一段横向空间，收起状态下标题不再与正文左对齐。

**文字按钮 → 图标按钮，且样式统一（用户两轮要求）。** 标题行里的动作一律 `btn ghost icon-button`：

```text
新建任务（Home / 任务列表）、新建会话（会话列表）   plus
刷新工作区状态（项目列表）                        refresh
询问 Assistant（项目详情）                        chat
继续（会话详情）                                  play（Icon.tsx 新增）
更多操作（任务详情 ·••• 菜单）                     more（Icon.tsx 新增，取代 "•••" 文字）
任务列表 / 会话列表 / 回收站（分段控件）            tasks / chat / archive
```

分段控件里的「回收站」用**归档盒**而不是垃圾桶：同一个 `trash` 原本同时表示「回收站（入口）」与「移入回收站（动作）」，两个含义共用一个图形；换形状后入口与动作分开，动作那三处（`WorkstreamDetailView`、`SessionDetailView`、`SessionTable`）继续用垃圾桶。

统一的两层含义：一是**变体**——原先 `新建 / 继续` 是 `primary` 实心、其余是 `ghost`，现在全部 ghost，标题栏里不再有哪个动作被强调（要恢复强调只需把那几个类名改回 `btn primary icon-button`）；二是**尺寸**——`button.btn` 自带 `min-height: 32px`，而 `button.icon-button` 只改了宽度，不覆盖的话按钮是 28×32 的长方形；标题栏作用域内统一成 28×28，与左侧三个历史按钮同形（实测四个按钮 `top` 都是 10、高 28）。

`aria-label` + `title` 承接原文字名，分段控件另加 `aria-pressed`（去掉文字后，状态只剩底色，可访问性上必须显式表达）；进度类状态（刷新中）也退化为 `aria-label` 与 `disabled`，`ProjectsView.test.tsx` 相应改成按 role 断言。

**标题字号**：24px（全局 h1）→ 20px → **16px**（用户两轮都嫌大）。这是唯一一处并非"标题行该多大"的推演，而是按观感调的，改的是一个数字。

## 36.13 看板的搜索筛选区吸顶

**用户要求**：三个看板（项目 / 会话 / 任务列表）上下滚动时，把上边的搜索筛选区固定在标题行下方。

**改动**：只动 CSS（`layout.css` 抽一个变量、`components.css` 用两条规则），三个视图都复用 `.board-toolbar`，无需改任何 TSX：

```text
layout.css     .main 上定义 --head-gap: 20px（页头与正文的间距），.page-head 的 margin-bottom 改用它
               .main:has(> .board-toolbar) { --head-gap: 0px }   ← 看板页清零
components.css .board-toolbar { position: sticky; top: calc(var(--titlebar-height) + var(--head-gap)); z-index: 5 }
               .board-toolbar::before { inset: calc(-1 * var(--head-gap)) 0 0; background: var(--bg-app) }
```

**关键是把「间距」和「钉在哪」变成同一个变量。** page-head 的 `margin-bottom`（间距）与 `.board-toolbar` 的 `sticky top`（钉在哪）都读 `--head-gap`：静态位置 = band 高 + 间距，钉住位置 = band 高 + 间距，两式恒等，**travel 天然为 0**。看板页把 `--head-gap` 清零，就同时得到"紧贴页头 + 不跟着滚"两个效果；回收站模式下工具栏不渲染，`:has` 不成立，间距自动回到 20px。若两处各写死一个数字，改间距时就会重演下面那个坑。

**踩到的坑：`top` 不能写 `--titlebar-height`。** 第一版写的是 `top: var(--titlebar-height)`（48px），但工具栏的静态位置在页头那条 20px 间距之后、即 y=68。sticky 的语义是"先跟着滚，走到 top 才钉住"，于是它先跟着滚了 20px 再固定——用户的原话是「会跟着往下移一点，然后固定住」。修成"钉住位置等于静态位置"后 travel 归零。

**间隔的取舍（用户第二轮决定不留）。** 我先按"保住现有外观"选了留 20px 间距的版本（`top = 48 + 20`，`::before` 往上多铺 20px 把那条缝盖住，否则正文会从缝里穿过去）；用户看过说「我觉得不留缝吧，现在这个间隔我觉得挺大的」，于是把 `--head-gap` 在看板页清零，卡片直接贴着 band。留缝版不需要了，但 `::before` 仍要留——卡片是圆角的，钉住后 12px 圆角缺口会漏出从下面滚过去的正文。

**验证**（`/tmp/noending-header-preview/board.html`，浏览器实测非推演）：看板页 `--head-gap = 0px`、`.page-head` 的 `margin-bottom = 0`、`sticky top = 48px`，`restTop = 48`、滚到 `scrollTop = 400` 后 `scrolledTop = 48`、**travel = 0**，band 底边 48（紧贴）。把一条通栏长文本滚上去，`elementFromPoint` 在卡片圆角处（y=52 / 62）命中 `.board-toolbar`（不露字）、卡片下沿（y=180）命中正文；再把工具栏从 DOM 里摘掉，`--head-gap` 回到 20px（对应回收站模式）。`tsc --noEmit` / `vitest run`（64 passed）/ `vite build` 全绿（纯 CSS 改动）。

**代价**：钉住期间固定占用约 105px 视口（搜索框 + 一行筛选控件）。若嫌占地方，下一步可做"滚动后折叠成一行"的紧凑态，需要 JS 监听滚动位置，本次没做。

## 36.14 修复：页头搬到标题栏后窗口拖不动了

**用户报告**：§36.12 之后，「顶部标题栏不会触发点按拖动应用窗口了」。

**原因**：窗口拖动在 Tauri 里只有一条路——**带 `data-tauri-drag-region` 的元素**。tauri 注入的 `scripts/drag.js`（`tauri-2.11.5/src/window/scripts/drag.js`，由 window 插件 `js_init_script` 注入）在 `mousedown` 时沿 `composedPath` 从里往外找这个属性，命中就 `invoke('plugin:window|start_dragging')`。

```text
原来：.app 有 padding-top: 48px，顶部那条 48px 带里只有 .app-titlebar 的 .window-drag-region
      （z-index 4）→ 直接点在它上面 → 拖动正常。
现在：.app 不再留顶部空间，.main 顶到 y=0，吸顶的 .page-head 覆盖了同样这段（z-index 6 > 4）
      → 命中目标变成 .page-head 或它的子元素 → 沿路径上溯找不到属性 → 不拖。
```

而 §36.12 里我给它加的 `-webkit-app-region: drag` 是 **Electron 的属性**：WKWebView 不实现它，`wry`/`tauri` 的源码里也搜不到任何 `app-region` 字样。加上它等于没加，所以"标题行自己承担拖动"这个说法从一开始就是错的。

**修法**：`PageHeader` 的 `.page-head` 上写 `data-tauri-drag-region="deep"`，并把三处不生效的 `-webkit-app-region` 删掉（`.window-drag-region`、`.page-head`、`.history-button` 以及标题/动作区的 `no-drag`）。

**为什么是 `deep` 而不是裸值**：脚本对属性值有三种解释——裸值/`"true"` 只认"正好点在这个元素上"；`"deep"` 认子树内任意位置；`"false"` 禁用。标题行里点得最多的是标题文字（h1）和按钮之间的空隙，裸值这两处都落空；`"deep"` 才覆盖整条。**可点元素会自动豁免**：脚本遇到 button/input/a/`[tabindex]`/`role=button` 且它自己没这个属性时直接 `return false`，所以标题栏里的按钮照常可点，不需要（也不能）再用 no-drag 表达——这一点和 Electron 的写法正相反。

**验证**（`/tmp/noending-header-preview/board.html`，把 `drag.js` 的 `isDragRegion` 原样搬进页面跑）：标题行空白处 `DIV.page-head => DRAG`、标题文字 `H1.page-title => DRAG`、动作按钮 `svg => no-drag`、历史按钮 `BUTTON => no-drag`、正文 `P => no-drag`；交通灯区与侧栏列上方仍是 `DIV.window-drag-region => DRAG`。再把属性摘掉，同一位置复现为 `no-drag`——即用户报的那个故障，装上属性后回到 `DRAG`。`tsc --noEmit` / `vitest run`（64 passed）/ `vite build` 全绿。

**遗留的代价**：拖动区要求 mousedown 不被 `preventDefault` 以外的处理占用，脚本对命中拖动区的按下会 `preventDefault()`（防文本光标），所以标题文字**不再能选中**——原生标题栏本来也选不中，换来的是整条可拖。另外该属性与平台无关：Windows 下窗口有原生装饰栏，标题行变可拖只是多一处能拖的地方，无害（本地只能验 macOS）。

## 36.15 详情页加载态不再整页替换（沿用 ProjectDetail 的写法）

**用户报告**：进入会话 / 任务详情页会先闪一个"加载中的页面"。

**原因**：路由只带 id（`{ view: "session"; sessionId }`），`Router` 直接换组件、没有 keep-alive，页面把数据放在自己的 `useState(null)` 里、也没有跨页缓存——导航后的第一帧必然没有数据。真正让"等一个 IPC 往返"看起来像"闪了一整页"的是下一件事：**两个详情页的加载分支都在 `PageHeader` 之前 return**。

```tsx
// 改前
if (!detail) return <div className="main narrow">加载中…</div>;                       // SessionDetailView:94
if (!ctx) return <div className="main narrow" role="status">{…}</div>;                // WorkstreamDetailView:134
```

标题行是吸在应用标题栏那条 band 上的（§36.12），加载态不渲染它，**整条 band 会先消失、数据到了再补回来**（左侧三个历史按钮还在）——那一下比"内容区里一行加载中"显眼得多。（`WorkstreamDetailView` 还会在每次 `[workstreamId, retry]` 的 effect 开头清空状态，见 `:99-101`，所以进入时一定经过加载态，这是它有意为之的行为，不改。）

**改法**：照 `ProjectDetail`（`:133` 无条件渲染 `PageHeader`、只换标题文案）把状态名挪进标题，页面骨架在加载 / 失败 / 成功三种情况下都是同一条：

```text
SessionDetailView    if (!detail) → <div className="main narrow">
                       <PageHeader title={failed ? "读取会话失败" : "加载中…"}>，失败时正文放原有的说明与「重试」
WorkstreamDetailView if (!ctx)    → <PageHeader title={loadError ? "读取任务失败" : "加载中…"}>，失败时正文放错误详情与「重试」
```

两处顺带的结果：`SessionDetailView` 原先"失败"与"加载中"两个分支合并成一个（判据本来就是同一条 `!detail`）；`WorkstreamDetailView` 正文里原写的「读取任务失败：{loadError}」去掉了前缀，因为标题已经说了状态、正文只该放详情（与 ProjectDetail 把 `PathError` 放在正文一致）。外层 `role="status"` 保留，无障碍行为与改前一致。

**验证**：新增一条回归测试（`WorkstreamDetailView.test.tsx`，"keeps the title band while loading"，让读取永不返回、断言标题仍是 `加载中…` 且正文为空），防止以后又有人把带 `PageHeader` 的骨架短路掉；`tsc --noEmit` / `vitest run`（65 passed）/ `vite build` 全绿。视觉上仍需在真机点一下确认——这条改动没法在静态预览页里验（它依赖真实的 IPC 往返时长）。

## 36.16 任务详情页去掉标题下那行元信息

**用户要求**（先说要删状态徽标，随后更正）：「这一行信息都去掉，因为其他地方都有显示，重复了」——即 `ws-detail-head-meta` 整行。

**改动**：`WorkstreamDetailView` 里整块删掉，连带这些只被它使用的代码：`const primary = paths?.[0]`、`cwdDisplayLabel` 这个 import、以及上一轮已变成孤儿的 `LIFECYCLE_LABELS` 词表（同名的另一份在 `WorkstreamCard`，看板卡片在用，保留）。`ws-detail-head-meta` / `dot-sep` 的 CSS 不动——`ProjectDetail` 与 `SessionDetailView` 的同名行还在用。

**逐项对照"其他地方有没有"**（都成立）：

```text
项目        → aside「项目」块（列出每个 Project，主关联标注「主项目」，可点进项目页）
主工作目录   → aside「工作目录」块（position 0 那条标「主目录」）
最近更新     → aside「状态」块底部「创建于 … · 最近更新 …」
回收站       → aside「状态」块里的「在回收站中」徽标
无工作目录   → aside「工作目录」块的「未设置工作目录，新会话使用默认目录。」
```

**唯一一处没有替代品的**：`{!primary.exists && <span className="badge warn">主路径目录不存在</span>}`。`WorkstreamPathList` 只渲染路径与「主目录」徽标，不带 `exists` 判断（`MissingBadge` 目前只有 `ProjectDetail` 在用），所以这一行删掉之后，任务详情页上再没有"主目录在本机读不到"的提示（项目侧仍有：「有目录缺失」筛选、项目详情里的 `MissingBadge`）。用户未要求补回，故按原样删除并在交付说明里点出；若要补，最小的做法是给 `WorkstreamPathList` 的条目加上现成的 `MissingBadge`。

**验证**：`WorkstreamDetailView.test.tsx` 里那条断言改成 `getByText("/repo/main")`（原来写 `getAllByText(...).length > 0`，注释还提到"路径同时出现在头部的主目录摘要里"——那行已经不存在了，改成单处断言正好把去重这件事锁住）；`tsc --noEmit` / `vitest run`（65 passed）/ `vite build` 全绿。

## 36.17 项目详情页：标题下的元信息移到右侧栏

**用户要求**：「项目详情页中的这部分可以移到右侧区进行显示」——截图是标题下那行 `Git 项目 · 自动命名 · 创建于 2 天前`。

**改动**：`ProjectDetail` 的 `PageHeader` 不再有 children（那行是它唯一的 children），三件事搬到 `<aside>` 里成为**第一块**，标签 `属性`；形状照抄 `WorkstreamDetailView` 的「状态」块（`section-label` + 徽标行 + 一行 `small muted` 的「创建于 …」）。随后用户追加「概览内容也可以放到属性里」，于是主栏那个 `概览` section（§19 的三个数字）也并进同一块，「概览」这个标签随之消失。

```text
标题栏        只留标题 + 三个图标动作（询问 Assistant / 刷新 / 重命名）
aside 属性     [Git 项目 | 目录项目] 自动命名 | 自定义名称
               3 个工作目录 · X 个任务 · Y 个会话      ← 原「概览」section 的内容
               创建于 2 天前
aside 工作目录  原来的内容（不变）
```

概览那行原用 `.ws-card-meta`（卡片的 flex + `margin-top: auto`，为卡片底部设计的），搬进 aside 后改用与「创建于」一致的 `small muted`，块内两行间距统一 `marginTop: 6`；`.ws-card-meta` 本身仍被看板卡片与 `ProjectsView` 使用，CSS 不动。

**两处细节**：aside 里原来的 `marginTop: 26` 在「工作目录」那个 section 上，作用是让右栏第一块与左栏对齐。现在它挂在新的「属性」块上、「工作目录」改回不带内联样式的 `.rail-section`——右栏起点位置不变，两块之间用 `.rail-section` 自带的 30px，不必叠加出 56px 的空档。（右栏第一块比左栏的「任务」低 26px 是**原来就有**的：那个 26px 从前挂在「工作目录」上，我只是让它跟着上移，没有改动对齐关系。）

**与 §36.16 合起来看**：任务详情页那行是重复信息、整行删掉；项目详情页这行没有别的去处、于是挪进右栏。两页现在的分工一致——**标题栏只放标题与图标动作，只读事实一律放右栏**。`.ws-detail-head-meta` / `.dot-sep` 仍被 `SessionDetailView` 使用，CSS 保留。

**验证**：`tsc --noEmit` / `vitest run`（65 passed）/ `vite build` 全绿。视觉上未在真机确认（这条改动依赖真实渲染，静态预览页只放了本页的两个 section 形状，没有整页数据）——新块与 `WorkstreamDetailView` 右栏的「状态」块同形，后者是已经在用的样子。

## 36.18 项目详情页的任务列表不再分「主关联 / 关联」

**用户要求**：「项目的关联任务不再区分主关联和次关联」。

**改动**：`ProjectDetail` 的「任务」section 去掉分组，`detail.workstreams` 平铺。连带删掉只为此存在的 `const primary` / `const related`（两个 filter）、两个分组标题 `<h3>{title} · {count}</h3>`、以及只为 `React.Fragment` 存在的 `React` 默认导入（本项目用自动 JSX runtime，其余文件本来就不导入 React）。总数不会因此丢失——右栏「属性」块里的「N 个任务」就是它。

**顺序没变**：后端本来就 `ORDER BY is_primary DESC, w.updated_at DESC`（`storage/workstream_paths.rs:490`），所以主关联仍在前面、其余按最近更新；这次只是不再把它显式标出来。要改成纯时间序，得动那条 SQL。

**留下的**：`types.ts` 的 `ProjectWorkstream.is_primary` 前端已无人读，但后端 `ProjectWorkstream` 仍在 payload 里带着它（`commands/project.rs:376` 记的就是这个形状），清掉它要连后端一起，故未动。

## 36.19 会话详情页加右栏，会话信息不再默认收起

**用户要求**：先「给会话详情页也加个侧边区，把图中的两部分内容放右侧」（图里是 `◇ Codex · 122 条消息` 与收起的「会话信息」），随后「会话信息不再是默认收起的样式，其他详情页的显示样式保持一致」。

**改动**（`SessionDetailView.tsx`）：

```text
标题栏      只剩标题 + 三个图标动作（移入回收站 / 刷新 / 继续）
左栏 main   关联任务（含「编辑关联任务」）+ 消息流
右栏 aside  rail-section「会话信息」：◇ Agent · 条数 · 无标题/回收站徽标，随后是展开的字段表
           （开始时间 / 最近活动 / 项目 / 工作目录 / 原始会话文件 / 会话 ID / Agent 侧会话 ID）
回收站横幅  仍是全宽，放在两栏之上
```

三件事值得记：

1. **整行元信息搬走，而不是部分搬走。** 那一行里还有「无标题」「回收站」两枚徽标。只搬 Agent 与条数、把徽标留在标题下，常见情况下那行就成了一条空行（多数会话既无标题问题也不在回收站）——所以整行一起进了右栏。
2. **`<details>` 换成 `rail-section` + `section-label`**，与 `WorkstreamDetailView` 的「状态 / 工作目录 / 项目」、`ProjectDetail` 的「属性 / 工作目录」同形；`.session-info` 那套卡片式边框样式随之删除（全仓只有这里用）。summary 上那枚「原始文件缺失」徽标也删了：字段「原始会话文件」下面本来就印着「找不到原始会话文件」，同一件事说两遍。
3. **字段表在右栏必须改成「标签在上、值在下」。** 原来是 `grid-template-columns: auto minmax(0, 1fr)` 的标签|值两列；在 240–300px 的右栏里值只剩一百来像素，一条 Codex 转录路径会被拆成五六行。改法：把内联样式收进 `.session-info-fields`，并利用 `Field` 渲染的「标签 div + 值 div」两个兄弟节点，用 `:nth-child(odd)` 给每个标签加 12px 上间距。**这一条是在静态预览页里看出来的**（`/tmp/noending-header-preview/session-aside.html`，240px 与 300px 并排、真实样式表）：两列版本在 300px 下已经很难看、240px 下日期都被折成两行。

**已知的一处不一致**：右栏只有这一个大标题块，没有像另外两页那样按主题分块（那是它们的既有结构，这里没有对应内容可分）。左栏第一个块「关联任务」原来带 `marginTop: 30`（左栏因此比右栏低 30px），用户随后反馈「关联任务上面有一个很大的间隔」——去掉后这一段的间距就只剩标题行自己那条 `--head-gap`（20px），与任务详情页一致，两栏起点也自然对齐了。

## 36.20 会话详情的关联任务：不标角色，编辑改成 + 的添加弹窗

**用户要求**：「关联任务不再区分主关联和相关关联。编辑按钮改为 + 按钮，点击出现会话所属项目下的任务列表弹窗可供添加任务，这个弹窗可参考新建任务中添加已有目录的弹窗」。

**改动**：

```text
关联任务列表   去掉每行右侧的 主关联/相关关联 徽标（bindingRoleLabel 因此成为死代码，已从 SessionTable 删除）
section 右上角 edit 图标 → plus 图标，aria-label「添加关联任务」
弹窗          新组件 ExistingTaskPicker，形状照抄 ExistingPathPicker：
              候选 = 这条会话所属项目下的任务（api.listWorkstreams(projectId)），
              已关联的置灰打勾且不可点（.existing-path-option.is-added），底部「添加 (n)」
```

**"所属项目"怎么确定**：`workspacePath.project_id ?? session.project_id ?? null`——派生链优先，退回 v0.2 之前手工指派留下的 project_id；两者都没有时不筛，退回全部任务（否则这条会话再也关联不了任何任务）。归档任务不进候选（`visibility === "normal"`），与旧弹窗的下拉一致。

**提交方式**：仍然只调 `replaceSessionBindings`，并且提交的是**完整集合**（已有关联原样带上 + 新选的按 `role: "related"` 追加）。后端在单事务里 diff，未改动的 row 连 created_at / provenance 都不动；不这样做就会把已有关联整批冲掉。`role` 字段保留在数据模型里（`domain/models.rs:509` 注释即 `primary | related`），只是不再出现在界面上——它现在只影响排序（`primaryFirst`），新加的一律 `related`（与旧弹窗的默认值相同）。

**留下的一个洞（明确记录）**：解除关联原先只有那个「编辑关联任务」弹窗能做，换成添加弹窗后**界面上没有移除入口了**。后端能力还在（`replace_session_bindings` 传一个更小的集合即可），回收站确认框里「从任务移除 = 只修改这个任务的成员关系」那句话也还在，但暂时无路可走。已向用户点明，等其选择补法（关联任务行上加 ✕，或让弹窗里已关联的项可取消勾选）。




## 36.21 会话消息：列表截断到 240 字，点开弹窗看全文

**用户要求**：三条一起提的——「1、消息内容过长时，不用『展开全文』，而是消息栏可点击出现文本显示弹窗；2、希望文本显示支持 md 格式预览；3、希望能呈现出一种聊天记录的感觉，用户消息标题置右」。讨论后定了顺序（先 1 后 2 再 3），并明确「截断要做，420 字多了，可以缩到 240 字」。本节记第 1 条，第 2、3 条各见 §36.22 / §36.23。

**改动**：

```text
SessionMessage.tsx  TRUNCATE_AT = 240（原 420）；删掉「展开全文 / 收起」那个 link 按钮与 expanded 状态
                    整行 onClick 开弹窗；head 右侧新增 .event-open 图标按钮（expand）作为键盘 / 可发现性入口
                    新组件 MessageModal：标题「who · #序号 时间」，正文 pre-wrap，底部「复制全文 / 关闭」
common.tsx          Modal 增开 wide 开关；copyToClipboard 从 SessionDetailView 搬到这里（两处共用）
global.css          .modal.wide { width: min(920px, 92vw) }
components.css      .event 左右各 8px 内距 + 负 margin；.event.openable 悬停底色；.event-open 平时淡出；
                    .msg-full 保留原始换行
```

**为什么保留截断、而不是整段就地铺开**：这个流是密排的一列，就地展开会把后面的消息越推越远，读到一半就丢了位置；而消息列在右侧栏上线后只有 ~724px，长内容（代码块、JSON、diff）就地折行会非常难读。弹窗给的是整块宽度加一屏可滚动区域。

**弹窗必须比消息列宽，否则这事没意义**：`.modal` 默认 640px，比 724px 的消息列还窄——那开弹窗只会更难受。所以给 `Modal` 加了 `wide` 开关（`min(920px, 92vw)`），只给全文类弹窗用。

**两个交互细节**：

1. **正文仍要能拖选复制**。整行可点，所以点击时先确认「没有选中任何文字」（`window.getSelection()`）并且落点不是按钮——否则一次划选就会弹出弹窗。
2. **展开按钮平时 `opacity: 0`，悬停 / `:focus-visible` 才现形**。整行可点已经足够可发现（配合 `cursor: pointer` 与 title），每条消息常驻一个图标会把密排列表弄脏；但它必须留在 DOM 里，键盘用户才能 Tab 到它。

**验证**（`/tmp/noending-header-preview/messages.html`，真实样式表，量的是坐标不是观感）：`.event` 左右各外扩 8px（正文位置不变，分隔线同宽）；展开按钮 22×22、距事件右缘 8px；`min(920px, 92vw)` 在 634px 视口下算出 583px（真机 1360px 视口则是 920px）。`tsc --noEmit` / `vitest run`（65 passed）/ `vite build` 全绿。

## 36.22 会话消息：弹窗内的 Markdown 预览（默认仍是原文）

承接 §36.21 的第 2 条。弹窗开了以后，正文默认按**原文**（pre-wrap）显示，只在内容看起来像 Markdown 时，标题栏右侧才出现「原文 / 预览」分段开关——默认停在原文，切到预览才走渲染。这样"复制全文"拿到的永远是 Agent 的原始输出，不会被渲染结果替换掉；预览是一个**可选的阅读视图**，不是新的真相来源（AGENTS.md: Provenance Fidelity）。

**依赖**：`react-markdown@10.1.0` + `remark-gfm@4.0.1`（表格 / 删除线 / 任务列表）。两者都只做 AST → React 元素，不碰 `innerHTML`。

**安全边界（为什么不开 `rehype-raw`）**：转录文本是**不可信输入**——里面是 Agent 与工具的原始输出，用户没法预先审查。这个 webview 又能调 Tauri IPC（`window.__TAURI__`），一旦 raw HTML 被渲染，一个 `<img onerror>` 就等价于本地代码执行。所以：

```text
rehype-raw        不装、不启用 —— 原始 HTML 一律当纯文本节点渲染
a / img           components 里改写成 <span>：链接显示文字 + title 提示、图片降级为「[alt] src」
                  —— 防两件事：把应用导航走（webview 跳走就回不来了）、以及远程请求泄露阅读行为
```

`img` 不渲染成真 `<img>` 还有个副作用是好的：消息里的图片链接不会去外部拉取，弹窗打开不发任何网络请求。`md-link` / `md-img` 只是上色 + 弱化，不带 `href` / `src`，点了没有任何行为。

**触发条件 `looksLikeMarkdown(text)`**：有围栏代码块、`#` 标题、列表、引用、表格、`**加粗**` 或行内 `` `code` `` 之一才算。普通散文不给开关——给一段没有 Markdown 的纯文本加个切不出差别的开关，只会让人怀疑自己看错了。该函数与 `MD_COMPONENTS` 一起从 `SessionMessage.tsx` 导出，供测试直接断言。

**样式**：`.md-body` 是作用域内的完整一套（标题 14px、`pre`/`code` 走 `--bg-panel`、引用、表格、`hr`、链接用强调色），不继承全局 Markdown 约定，避免污染应用其它地方。行内代码与围栏代码同底色，靠 `pre` 的内距区分块级。

**验证**：`SessionMessage.test.tsx` 新增 5 条——截断 + 弹窗（§36.21）、无文本事件不弹窗、**预览注入的 DOM 里 `script` / `a` / `img` 均为 0 个且 `h1` 正常渲染**、纯散文不给「预览」页签、`looksLikeMarkdown` 正反例。`/tmp/noending-header-preview/messages.html` 用真实样式表 + 手写的 react-markdown 产物渲染截图核对：分段开关、标题、行内 code + 加粗、被中和的链接（蓝色 span）与图片（`[图] …`）、代码块、列表、引用、表格、`hr`、底部「复制全文 / 关闭」吸底。`tsc --noEmit` / `vitest run`（70 passed）/ `vite build` 全绿。

## 36.23 会话消息：聊天记录观感（用户右 / Agent 左）

承接 §36.21 的第 3 条。列表从「一列等宽、每条一条细分隔线」改成**分侧气泡**：用户消息整体靠右、气泡用强调底色，Agent 消息靠左、气泡用面板底色；技术事件（system / compact / 其它）不变，仍是弱化的整行 + 细分隔线。

**分三类的依据**：这个流里只有两方是「说话的人」，剩下的都是 Agent 跑出来的系统噪音——它们没有"立场"，右对齐或加气泡都会假装成对话的一方。所以类名不是按 kind 逐个映射，而是 `is-user` / `is-agent` / `is-tech` 三档，`is-tech` 把原来的分隔线观感整套接回来（`.event.is-tech { padding: 12px 8px; border-top: … }`），对话行则不画线，靠 12px 间距分区。

**气泡只占 82%**：铺满整列就没有左右可言——对侧的留白本身就是"谁在说"的信号。`.body` 上加 `max-width: 82%` + `align-self`，圆角在朝向对方的那一角收到 `--radius-sm`（用户右下、Agent 左下），做出指向感。

**标题行跟着气泡走，不横铺整列**：`.head` 也限 82% 并按侧对齐。一开始想保留 §36.21 的「展开按钮贴行的最右缘」（`margin-left: auto` 需要标题行撑满整列），但那样右对齐的气泡下面会横挂一行从左边起头的名字，读起来是断裂的；改成标题行只跟气泡同宽，展开按钮就贴在时间戳后面。它仍然是同一行里最靠外的元素，只是"右缘"从整列变成了气泡的右缘。

**没有新开一条左对齐的竖线 / 头像位**：多一列头像会把每条消息的正文再压窄 30px，而 240 字截断本来就短，代价不值。两侧的底色差异已经足够。

**验证**（`/tmp/noending-header-preview/chat.html`，真实样式表，724px = 右侧栏上线后的实际消息列宽，明暗两套都看过）：用户气泡右缘 748px = 行右缘（行宽 740 + 左右各 8px 内距），Agent 气泡左缘 24px = 行左缘；两侧气泡宽度上限 594px = 724 × 82%；用户气泡圆角 `12px 12px 6px`（右下收角）、Agent `12px 12px 12px 6px`（左下收角）；技术事件仍是 1px 顶边线 + 724px 整行。`SessionMessage.test.tsx` 增加一条钉住三个类名（CSS 挂了单测看不出来，所以把类名钉在测试里）。`tsc --noEmit` / `vitest run`（71 passed）/ `vite build` 全绿。

## 36.24 会话消息：气泡内直接预览 Markdown，热区收到气泡（修订 §36.21–§36.23）

用户看过 §36.23 的效果后提了五条，都在这一节：

```text
1  悬浮选中的范围是气泡，不是整条消息栏
2  去掉「查看完整消息」图标按钮 —— 气泡自己就是入口，给它悬浮效果就够
3  全文弹窗默认显示 Markdown 预览
4  弹窗右下角「复制全文」「关闭」两个按钮样式要一致
5  列表气泡里显示的就是 Markdown 预览版
```

**热区从整行收到气泡（1、2）**：整行可点会让「悬停到哪儿」变成一条与内容无关的宽条，而"点开这条消息"这件事属于这条消息本身。于是 `.event.openable` 整行悬停底色、`.event-open` 图标按钮、以及 §36.21 为它准备的 `expand` 图标全部删掉；点击与 `:hover` 都移到 `.body` 上。键盘可达不能跟着丢——`.body` 变成 `role="button"` + `tabIndex=0` + Enter/Space 处理，焦点环交给全局的 `:focus-visible`（a11y 树里它读作 "button" + "点击查看完整消息"，与原来的图标按钮等价）。悬停底色不能用 `--bg-hover`：那会把用户气泡的蓝调洗掉，看起来像换了个人在说话，所以新增 `--accent-wash-hover`（亮 `#e2e8ff` / 暗 `#2d3763`）。

**气泡里直接渲染 Markdown（5）**：看着更好，但**不能再用字数截断**——240 字切在 ``` 或 `**` 中间，后半段会被整块渲染成代码或凭空多出几个星号。改成按高度收口：`max-height: 132px`（约 6 行）+ `overflow: hidden`，再叠一层底部 26px 的渐隐提示还有下文。渐隐必须**只在真的收角时才挂**，否则内容本来就只有两行的气泡会被擦掉最后两行——所以 `is-clamped` 由 JS 量 `scrollHeight > clientHeight` 得出（`useLayoutEffect`，早于绘制）。纯文本消息仍按 240 字截断，因为纯文本可以安全地切。

**弹窗默认预览（3）**：`previewable` 由列表侧算好的 `md` 传进来（现在两处判断一致，弹窗不再自己 `looksLikeMarkdown`），`useState(previewable)` 直接默认落在预览上。「原文」页签保留——复制全文拿到的仍然是原始文本（Provenance Fidelity）。

**两个按钮同款（4）**：都改成 `btn`（原为 `btn small` + `btn primary`）。这是个只读查看器，没有主次动作，硬分主次反而让人以为「关闭」是推荐操作。

**验证**（`/tmp/noending-header-preview/chat2.html`）：把那段收角判断原样搬到页面里跑，短 md 气泡 `scrollHeight 38 = clientHeight 38` → 不加 `is-clamped`、无渐隐；长 md 气泡 `252 > 132` → 加了，`mask-image` 生效。`--accent-wash-hover` 解析为 `#e2e8ff`，`hover` 命中后用户气泡底色实测变成 `rgb(226,232,255)`，同排 Agent 气泡仍是 `#f5f5f3`。两个页脚按钮 class 都是 `btn`、高度都是 33px。明暗两套都截了图。`tsc --noEmit` / `vitest run`（72 passed）/ `vite build` 全绿。
