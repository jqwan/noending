# NoEnding 产品与领域设计方案 v0.3

## 1. 产品定位

NoEnding 是一个以长期 Context 为中心的本地多 Agent 工作空间。

它不以“统一查看不同 Agent 的聊天记录”为核心，而以 **Workstream Context** 为核心，将不同 Agent 的 Session 组织到统一、持续演进的语义层中。软件开发只是其中一种使用场景；Project 也可以代表旅行计划、长期研究、购买决策、学习主题、生活咨询或其他需要跨 Session 持续推进的主题。

第一版优先支持 Codex、Claude Code、Pi，但 Domain Model 不以 Coding 场景为前提。

产品解决的核心问题是：

- 同一主题、计划、问题或项目经常由多个 Agent、多个 Session 交替处理；
- Session 历史散落在不同 Agent 自己的数据目录中；
- 新建或恢复 Session 时，Agent 很难快速获得其他 Session 已产生的有效上下文；
- 完整聊天历史过于庞大，而普通摘要又容易丢失关键决策、约束与当前状态；
- 用户需要一种能够跨 Agent 延续“当前事情本身”而不是“某段聊天”的机制；
- 用户有时只是想直接开始一次 Agent Session，而不希望先整理 Project / Workstream；
- 用户需要系统在使用过程中自动完成 Workstream / Project 的归类，而不是把整理成本前置给用户。

本产品的核心思想是：

> **Session 是即时交互入口；Workstream 保存持续演进的语义状态；Project 是可选的长期整理层；Workspace Assistant 负责在 Session 与 Workstream 之间智能同步 Context，并辅助 Workstream / Project 的自动归类。**

一句话概括：

> **Session 会结束，Agent 会切换，Context 持续存在。**

产品交互原则：

> **Workstream-centered, not Workstream-required.**

即：

- Workstream 是长期连续性的核心；
- 但用户不需要为了开始一次 Session 先创建 Workstream；
- Project 更不应成为开始工作的前置步骤。

---

# 2. 核心概念

## 2.1 Project

Project 表示一个长期存在的主题空间，用于聚合相关的 Workstream、Session、Context 与资源。

它回答：

> “这些持续演进的信息最终属于哪个长期主题空间？”

Project 可以是：

- 一个软件项目；
- 一次旅行计划；
- 一个长期研究主题；
- 一项购买或决策过程；
- 一个学习计划；
- 一个持续数周、数月甚至更久的生活或工作主题。

Project 不严格等于，也不要求拥有：

- 一个文件夹；
- 一个 Git Repository；
- 一个 Git Worktree；
- 一个本地 Workspace；
- 任何固定文件路径。

Project 是**可选整理层**，不是使用 NoEnding 的前置对象。

因此以下状态是合法的：

```text
Workstream.project_id = null
```

一个 Project 可以包含：

- 多个 Workstream；
- 多个来自不同 Agent 的 Session；
- 零个或多个可选 Resource，例如 Repository、Workspace、文件、URL、文档或 Artifact。

Project 使用应用自身稳定 ID，不以文件路径、Repository 或 Agent Session 作为身份。

### ProjectResolver

NoEnding 可以根据已有证据自动推断 Workstream / Session 可能属于哪个 Project。

Coding 场景中的强信号包括：

```text
repository identity
git remote
workspace root
path ancestry
worktree lineage
existing session bindings
shared resources
```

但这些只是 **Project 归类证据**，不是 Project 本身。

例如：

```text
~/code/noending
~/code/noending/src-tauri
~/worktrees/noending-sync
```

即使路径不同，也可能属于：

```text
Project: NoEnding
```

非 Coding 场景则可以根据：

```text
Workstream semantics
Context Items
Documents
URLs
Artifacts
Places
Existing Project membership
User explicit assignment
```

进行归类。

高置信度时系统可以自动关联；低置信度时再询问用户。

### 示例：软件项目

```text
Project: NoEnding

Resources
├── Repository: desktop-app
├── Repository: docs
└── Workspace: ~/code/noending

Workstreams
├── Domain Model
├── Context Sync
└── Agent Adapter
```

### 示例：旅行计划

```text
Project: Japan Trip

Resources
├── URL: 航班候选
└── Document: 酒店预订信息

Workstreams
├── 行程设计
├── 酒店选择
└── 预算
```

Repository / Workspace 可以帮助自动发现或增强 Project，但只是可选 Resource，不决定 Project 的最终边界。

---

# 3. Workstream

## 3.1 定义

Workstream 是整个系统的核心连续性单位。

它表示：

> **一个拥有独立目标、可以持续演进、可以跨 Session 和 Agent 继续推进的工作上下文。**

例如：

- 设计 Domain Model；
- 实现 Context Sync；
- 规划日本关西行程；
- 比较三家候选酒店；
- 研究一款相机；
- 制定一个长期学习计划。

Workstream 不要求先归属于 Project。

合法状态包括：

```text
Workstream
Project: —
Sessions: 0
```

也就是说，一个 Workstream 可以只拥有 Goal / Context，而暂时没有 Project、Session 或 Resource。

Project、Workstream、Session 不再被理解为严格的父子生命周期：

```text
Project > Workstream > Session
```

而是：

```text
Project? ── organizes ── Workstream
                         ↕
                SessionWorkstreamBinding
                         ↕
                       Session
```

Project 可能存在几年，Workstream 可能持续几小时到几周甚至更久，而 Session 通常是一次具体的 Agent 交互或执行过程。

## 3.2 Workstream 来源

Workstream 可以：

1. 用户通过 **New Workstream** 显式创建；
2. Workspace Assistant 从一个或多个 Session 中自动识别并创建；
3. Workspace Assistant 建议用户对已有 Workstream 进行合并、拆分或调整。

Workstream 可以在没有任何 Session 时存在。

系统应尽量自动整理，但用户拥有最终编辑权。

## 3.3 Workstream-centered，不是 Workstream-required

Workstream 是 NoEnding 长期 Context 的核心对象，但它不是开始一次 Session 的前置条件。

因此用户可以：

```text
New Session
    ↓
先与 Agent 交互
    ↓
NoEnding Sync
    ↓
匹配已有 Workstream
或识别新的 Workstream
```

用户也可以反过来：

```text
New Workstream
    ↓
定义 Goal / Context
    ↓
从 Workstream 创建 New Session
```

两条路径都属于正常产品流程。

---

# 4. Session

Session 表示某个 Agent 的实际交互环境。

第一版支持：

- Codex
- Claude Code
- Pi

Session 是：

> **Agent 的对话与执行容器，而不是长期主题或 Workstream 的逻辑边界。**

因此：

```text
Session ≠ Workstream
```

一个 Session 可以：

- 不关联任何 Workstream；
- 关联一个 Workstream；
- 同时涉及多个 Workstream。

例如：

```text
Claude Session X

Primary:
Context Sync

Related:
Domain Model
Session Launcher
```

甚至在一个长 Session 中，其主要 Workstream 可以随时间变化。

因此 Session 与 Workstream 是多对多关系。

## 4.1 Session 可以先于 Workstream 存在

用户通过一级入口 **New Session** 可以直接启动 Agent，而不要求选择任何 Workstream。

此时 Session 可以暂时处于：

```text
unassigned
```

NoEnding 在后续 Sync 中根据新增内容：

- 匹配已有 Workstream；
- 建立多个 Binding；
- 识别新的 Workstream candidate；
- 或继续保持未归类状态。

归类不能阻止用户继续使用 Session。

## 4.2 Session 归类状态

领域层可以记录：

```text
unassigned
partially_assigned
assigned
```

其中：

- `unassigned`：尚未建立可靠 Workstream 归属；
- `partially_assigned`：部分活动已归类，但仍存在未归类的重要内容；
- `assigned`：当前重要活动已合理映射到一个或多个 Workstream。

这些状态主要服务自动整理，不要求用户日常维护。

---

# 5. Session 与 Workstream 的关系

系统通过：

```text
SessionWorkstreamBinding
```

表达 Session 与 Workstream 的多对多关系。

Binding 用于记录：

- Session；
- Workstream；
- role：primary / related；
- 关联置信度与来源；
- Session 最后获得的 Context 版本；
- 最后一次同步位置；
- 建立时间；
- 最近使用时间。

用户无需直接管理这些技术字段。

合法关系包括：

```text
Session A → 0 Workstreams
Session B → 1 Workstream
Session C → N Workstreams
```

以及：

```text
Workstream X → 0 Sessions
Workstream Y → N Sessions
```

Binding 的核心目的是让系统知道：

> “这个 Session 对这个 Workstream 已经知道到什么程度？”

从而在 Resume 时只补充新增 Context，而不是重复发送所有信息。

---

# 6. Workstream Context

Workstream Context 是系统真正需要长期维护的核心资产。

它不是：

- Session Summary；
- 完整聊天记录；
- Git 状态；
- 一段 AI 自动生成的大文本。

而是：

> **对当前 Workstream 有效语义状态的结构化表示。**

采用三层结构。

---

## 6.1 L1：Core Context

任何 Workstream 都拥有稳定的核心区：

### Goal

当前真正希望达成的目标。

回答：

> 我们最终在做什么？

### Current State

当前已经推进到什么程度。

回答：

> 现在是什么状态？

### Constraints

当前不能违反的重要限制。

例如：

- 不改变现有 API；
- 保持向后兼容；
- 数据必须本地保存。

### Decisions

当前仍然有效的重要决定。

例如：

- Workstream 是核心工作单位；
- Session 可以同时使用多个 Workstream；
- Context 使用 sync / merge 机制。

### Open Questions

当前仍未解决，并可能影响后续工作的关键问题。

---

## 6.2 L2：Extended Context Items

Core Context 不能容纳所有细节。

因此 Workstream 可以具有可扩展的 ContextItem。

首版内置类型应优先保持领域中立，例如：

```text
todo
finding
issue
risk
note
reference
artifact
requirement
decision_detail
research_note
```

在此基础上允许按领域扩展，例如：

```text
软件开发:
architecture_note
test_result
file_reference
api_contract
ui_requirement
dependency

旅行:
place
booking
itinerary
budget

研究:
source
hypothesis
evidence
```

未来允许继续增加。Core Context 保持稳定，Extended Context Items 允许随场景扩展。

ContextItem 至少包含：

```text
id
type
title
content
status
source
created_at
updated_at
```

可以附加：

```text
confidence
importance
tags
artifact_refs
related_items
supersedes
```

---

## 6.3 L3：Source

ContextItem 必须尽可能可追溯。

Source 可以来自：

- User message；
- Agent message；
- Tool result；
- Session event；
- 文件或文档；
- URL / Web page；
- Image / Screenshot；
- Location / Place；
- Calendar event；
- External data；
- Git diff；
- Test report；
- 用户直接编辑。

因此：

```text
L1 Core Context
      ↓

L2 Context Items
      ↓

L3 Raw Sources
```

形成从高密度语义到原始证据的完整路径。

---

# 7. Current Context 与历史

Workstream 需要同时满足两个目标：

1. Session 获取 Context 时应尽量简洁；
2. 用户仍然可以追溯过去的演进过程。

因此系统区分：

```text
Current View
```

和：

```text
History
```

例如：

```text
旧 Decision
Session A → Session B handoff

↓ superseded

旧 Decision
Workstream Context → Session

↓ superseded

当前 Decision
Session ↔ Workstream Context Sync
```

Session 默认只收到最后一条。

用户查看 History 时可以看到完整演进链。

原则：

> **传递当前状态，保留历史路径。**

---

# 8. Session → Workstream Context Sync

外部 Agent 不需要理解本应用，也不会主动提交 Context。

真正负责同步的是：

```text
Workspace Assistant
```

它定期或在关键节点检查：

```text
last_sync_cursor
        ↓
Session 当前最新事件
```

然后处理新增部分。

每次 Sync 主要回答：

1. 是否产生了值得长期保留的新信息？
2. 信息属于哪个已有 Workstream？
3. 是否形成了新的 Workstream？
4. 是否新增了 ContextItem？
5. 是否更新、解决或取代了已有 Item？
6. 是否存在相互冲突的信息？

同步结果自动 Merge 到 Workstream Current Context。

用户不需要审批每个 Sync。

---

# 9. Sync Point

第一版推荐以下触发点。

## 必选

### 启动 New Session 前

如果用户为 New Session 选择了一个或多个 Workstream Context，则优先同步相关 Workstream 中仍有未处理消息的 Session。

如果没有选择任何 Workstream：

```text
selected_workstreams = []
```

则允许直接启动 Session。

### Resume Session 前

Resume 只针对一个具体已有 Session。

执行前：

1. 先同步该 Session 自上次 Sync 后产生的新消息；
2. 读取已有 Workstream Binding；
3. 如果没有任何 Binding，也允许直接 Resume；
4. 后台可以继续做 Workstream 分类与 Project Resolution。

### Agent 进程退出

触发一次 checkpoint sync。

### 用户手动 Sync

提供显式入口。

### Application Reconcile

NoEnding 启动后检查上次关闭期间产生的新 Session activity。

## 后续增强

长时间 Session 可以进行周期性 Sync，例如：

```text
时间 + 新增有效消息数量
```

共同触发。

系统不要求存在可靠的“Session Completed”状态。

---

# 10. Workstream → Session

NoEnding 在界面上只提供两个一级创建入口：

```text
[ New Workstream ]   [ New Session ]
```

## New Workstream

表达：

> “我要开始一件需要长期持续的事情。”

创建 Workstream 后，用户可以定义 Goal / Context，也可以立即从其中启动 New Session。

## New Session

表达：

> “我现在要直接启动一个 Agent 会话。”

用户选择：

```text
Agent
Optional Workstreams
```

例如：

```text
Agent:
Claude Code

Contexts:
☐ Context Sync
☐ Domain Model
```

Workstream 可以全部不选。

因此：

```text
New Session ≠ New Workstream
```

### Resume 不是一级入口

`Resume Session` 不作为一级创建操作。

Resume 只存在于具体已有 Session 的上下文中，例如：

- Session 列表；
- Session 详情；
- Workstream 的 Related Sessions；
- Search result；
- Workspace Assistant 对具体 Session 的操作。

产品语义是：

> **Workstream 和 Session 可以被创建；已有 Session 可以被恢复。**

不应在 UI 中提供：

```text
Mode
○ New
○ Resume
```

这种把 New / Resume 作为同级创建模式的设计。

---

# 11. Session Context Bundle

传给 Agent 的不是完整 Workstream 数据，而是：

> **本次工作所需的最小充分上下文。**

## New Session

允许：

```text
selected_workstreams = []
```

此时可以不注入任何长期 Workstream Context。

如果用户选择了 Workstream，则通常包括：

- Goal；
- Current State；
- 当前 Constraints；
- 当前 Decisions；
- Open Questions；
- 与当前任务高度相关的 Extended Items；
- Relevant Artifacts；
- 必要的其他 Workstream 引用。

## Resume Session

Resume 的目标始终是一个具体已有 Session。

如果系统知道这个 Session 已经看过哪些 Context，则主要传递：

```text
Changed since your last activity
```

并可重复携带少量最重要的 Goal / Constraint，以避免语义漂移。

如果 Session 尚无 Workstream Binding，也允许直接 Resume；后续 Sync 再继续分类。

---

# 12. Workspace Assistant

第一版只有一个内置智能体：

```text
Workspace Assistant
```

它同时承担两种职责。

## Background Mode

负责：

- Session Context Sync；
- Workstream 分类；
- ContextItem 提取；
- Context merge；
- 新 Workstream 发现；
- Project Resolution；
- 冲突检测；
- Context 压缩与整理。

## Interactive Mode

用户可以直接与其聊天。

它可以访问应用管理的：

- Projects；
- Workstreams；
- Context；
- Items；
- Sessions；
- Session Messages；
- Sources；
- Sync History。

例如：

> 最近 Context Sync 有什么变化？

> 为什么我们不用 handoff 模型了？

> 昨天我和 Codex 做了什么？

> 用 Claude Code 继续 Context Sync，把 Domain Model 也带进去。

> 继续规划日本行程，把酒店选择和预算两个 Workstream 都带进去。

> 我们最后为什么放弃了那家酒店？把当时的来源找出来。

> 把昨天那个还没归类的 Claude Session 归到正确的 Workstream。

Workspace Assistant 可以成为应用的主要自然语言入口。

---

# 13. Workspace Assistant 的权限原则

读取操作可以自动完成。

修改操作遵循：

> **默认智能整理，用户始终拥有纠错权。**

用户至少可以：

### Context Item

- Edit；
- Move；
- Mark obsolete；
- Delete；
- View history；
- View source。

### Workstream

- Edit；
- Merge；
- Split；
- Archive。

Workspace Assistant 可以自动同步和整理 Context。

但以下情况不得静默解决：

```text
用户明确约束
vs
Agent 新结论
```

例如：

```text
Constraint:
不能修改数据库 schema

Agent finding:
当前方案似乎必须修改 schema
```

应保留为 Conflict，而不是覆盖任何一方。

---

# 14. 生命周期

Workstream 生命周期保持简单：

```text
open
completed
abandoned
```

同时单独记录：

```text
Activity:
active / recent / dormant

Visibility:
archived / normal
```

Activity 由系统自动计算。

Lifecycle 表示用户意图，不应根据“多久没活动”自动改变。

Archive 可以由用户操作，也可以支持将 completed Workstream 延迟自动归档。

---

# 15. 第一版范围

## 支持 Agent

- Codex
- Claude Code
- Pi

首版 Adapter 选择偏向 Coding Agent，是为了优先验证高频、多 Session、Context 漂移明显的场景；这不限制 Project / Workstream / Context 的领域范围。

## 核心能力

- 自动发现 Agent Session；
- New Workstream；
- New Session；
- 针对具体 Session 的 Resume；
- Session 可以暂时保持 unassigned；
- 自动识别 / 建议 Workstream；
- 自动 Project Resolution；
- Project 可选；
- Workstream 可无 Project；
- Workstream Current Context；
- L1 / L2 / L3 Context；
- ContextItem History；
- Session → Workstream 自动 Sync；
- 多 Workstream Context 聚合；
- 全文搜索；
- Workspace Assistant；
- Context / Item 人工纠错；
- Context 来源追踪。

## 第一版暂不重点解决

- 团队协作；
- 云同步；
- 跨电脑 Context；
- 多用户权限；
- 大规模 Agent Marketplace；
- 强项目管理能力；
- Jira 式复杂任务状态；
- 对几十种 Agent 的支持。

---

# 16. 产品核心原则

### Workstream-centered, not Workstream-required

长期主题的连续性属于 Workstream，但用户不需要为了开始一次 Session 先创建 Workstream。

### Session is immediate interaction

Session 是 Agent 消费和产生信息的即时交互与执行环境，也可以在未归类状态下独立存在。

### Project is optional organization

Project 是长期主题整理层，不是进入 NoEnding 的前置步骤，也不等于路径 / Repository / Workspace。

### Current state over transcript

新 Agent 应首先知道当前有效状态，而不是重新阅读完整历史。

### History is traceable

当前 Context 必须能够追溯历史和来源。

### Automatic by default

系统默认自动提取、分类、同步、Merge，并在可能时完成 Project Resolution。

### Human corrects, not maintains

用户的主要职责是纠错，而不是手工维护 Context 数据库。

### Context is selective

拥有全部信息不等于把全部信息塞给模型。

### Assistant is the intelligence layer

Workspace Assistant 是系统统一的智能能力，而不是另一个独立的信息孤岛。

---

# 17. 产品最终形态

NoEnding 最终不是：

> 一个跨 Agent 的聊天历史管理器。

也不是：

> 一个要求用户先建立 Project、再建立 Workstream、最后才能开始 Session 的项目管理工具。

而是：

> **一个以持续 Context 为核心的本地 AI 空间：用户可以直接开始 Session，也可以显式创建 Workstream；系统在后台持续把 Session 中的重要信息同步到长期 Workstream Context，并在需要时将 Workstream 自动整理进 Project。**

最终用户心智模型可以简化为：

```text
New Workstream
“我要开始一件持续的事情”

New Session
“我现在直接找一个 Agent 做事”

Resume on Session
“我要继续这个已有会话”
```

而系统内部负责：

```text
Session
   ↕
Workstream Context
   ↕
Project?
```

其中：

- Session 负责即时交互；
- Workstream 负责连续性；
- Project 负责可选的长期整理；
- Workspace Assistant 负责在它们之间智能同步、分类和连接。

软件开发、研究、旅行规划、购买决策、学习或其他长期主题，都可以使用同一套 Context / Sync 模型。

> **Conversations end. Context doesn’t.**
