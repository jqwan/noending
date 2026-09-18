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

