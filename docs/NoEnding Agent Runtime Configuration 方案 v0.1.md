# NoEnding Agent Runtime Configuration 方案 v0.1

> 状态：实现基线。本版相对上一版最重要的简化是把「native config discovery / effective default resolution」整个从 v0.1 删除。
>
> 一句话定义：**NoEnding 不管理 Agent 的默认配置，只管理用户在 NoEnding 中显式设置的 Runtime Overrides；
> Agent Default 永远通过「不传参数」实现，Model Discovery 只用于辅助用户选择 Override。**

## 1. 目标

为 Codex / Claude Code / Pi 提供统一的运行时配置能力，让用户可以在 NoEnding 中：

- 查看已安装的 Agent；
- 使用 Agent 自己的默认运行配置；
- 在需要时显式覆盖 Model / Provider / Effort；
- 在 Agent 支持时获取可选模型列表；
- 保证 New Session、Resume Session、Context Eval、Workspace Assistant 使用完全一致的 Runtime 配置语义。

核心原则：

> **Agent owns defaults. NoEnding owns overrides.**

```
没有 NoEnding override → 不传对应 CLI 参数 → Agent 自己决定默认配置
```

NoEnding 不解析 `~/.codex/config.toml`、`~/.claude/settings.json`、`~/.pi/agent/settings.json`、项目级 Agent 配置、环境变量配置、profile / managed config。这些始终属于各 Agent 自己的配置系统。

## 2. Runtime 配置语义

NoEnding 只保存显式 override：

```rust
pub struct AgentRuntimeOverrides {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
}
```

| 值 | 语义 | 行为 |
|---|---|---|
| `None` | Agent default | 不传参数 |
| `Some(v)` | NoEnding override | 显式传给 Agent CLI |

Codex 示例——全 `None` 时启动 `codex exec ...`，**而不是** `codex exec -m <NoEnding 猜出来的默认模型>`；用户显式配置 `model=Some("gpt-5.6-sol"), effort=Some("high")` 后才变成：

```
codex exec -m gpt-5.6-sol -c model_reasoning_effort="high" ...
```

## 3. 明确不做 Native Default Parsing

v0.1 不增加：`NativeRuntimeConfig` / `ObservedValue` / `RuntimeConfigSource` / effective model resolver / Agent config file parser。

Settings 中也不尝试显示 `Agent default = gpt-xxx`，只显示 `Agent default`。

真正的默认值可能同时受用户配置、项目配置、cwd、profile、环境变量、managed policy、Agent 版本影响。NoEnding 若自行计算 effective default，就必须复制各家 Agent 的配置解析逻辑，这不是 NoEnding 应承担的职责。

> Default 是一种**运行意图**，不是 NoEnding 解析出来的具体值。

## 4. Model Discovery 与 Default 完全分离

Model Discovery 只服务一件事：用户选择 Override 时，帮助用户找到可用模型。它**绝不**参与 Default resolution。

```
Agent Default          →  NoEnding 不解析  →  不传 runtime 参数
User chooses Override  →  Model Discovery  →  选模型/provider/effort
                       →  AgentRuntimeOverrides  →  显式 CLI 参数
```

因此即使 Model Discovery 完全失败，`Agent default` 仍然永远可用。同时保留 **Custom model ID** 输入，避免 discovery 能力限制用户。

## 5. Runtime Capability 模型

三家能力并不相同，不应强行抽象成完全相同的配置项。

```rust
pub struct AgentRuntimeCapabilities {
    pub model: RuntimeFieldCapability,
    pub provider: RuntimeFieldCapability,
    pub effort: RuntimeFieldCapability,
}

pub enum RuntimeFieldCapability {
    Unsupported,
    /// 可以显式填写，但无法可靠获取候选列表
    FreeForm,
    /// NoEnding 有建议值，但不保证完整
    Suggested,
    /// 可以从 Agent 动态获取
    Discoverable,
}

pub enum ModelCatalog {
    Dynamic(Vec<ModelOption>),
    Suggested(Vec<ModelOption>),
    Unavailable,
}

pub struct ModelOption {
    pub id: String,
    pub display_name: Option<String>,
    pub provider: Option<String>,
    pub supported_efforts: Vec<String>,
}
```

`ModelCatalog` 只是 UI 辅助信息，**不是** Agent 能力的最终真值。

## 6. 三家 Agent v0.1 能力

| Runtime field | Codex | Claude Code | Pi |
|---|---|---|---|
| Model override | Discoverable | Suggested / FreeForm | Discoverable |
| Provider override | Unsupported / 暂不开放 | Unsupported | Discoverable |
| Effort override | Supported | Supported | Supported |
| Native default parsing | 不做 | 不做 | 不做 |

**Codex** — 默认 Model / Effort 均为 Agent default；用户 Override 时 Model 尽可能动态获取 Codex model catalog，Effort 根据所选模型展示支持值。没有 override 时不传 `-m`、不传 `model_reasoning_effort`。

**Claude Code** — 默认 Model / Effort 均为 Agent default。第一版不要求可靠获取完整账户模型列表，UI 可给 `Agent default / Sonnet / Opus / Haiku / Custom…`，这些属于 Suggested 而非「当前账号完整可用模型」。Override 时传 `--model <value>` 与 `--effort <value>`。
> 前置修复：当前 adapter 完全丢弃 `ExecOptions.effort`（`adapters/claude.rs` 只处理 model），必须补齐，不能继续静默忽略。
> Provider 暂时 Unsupported，不出现在 UI。

**Pi** — 默认 Provider / Model / Thinking 均为 Agent default。Override 模式可动态 discovery（`pi --list-models`），从结果生成 Provider → Model 级联选择器；Override 后才传 `--provider` / `--model` / `--thinking`。

## 7. 后端模块

```
src-tauri/src/agent_runtime/
├── mod.rs
├── store.rs          // 只负责 NoEnding override：get_runtime_overrides / set_runtime_overrides
├── capabilities.rs   // 哪些字段支持 override、哪些可 discovery；不含用户配置
├── discovery.rs      // discover_runtime_options(agent) -> AgentRuntimeDiscovery
├── codex.rs
├── claude.rs
└── pi.rs
```

```rust
pub fn discover_runtime_options(agent: Agent) -> Result<AgentRuntimeDiscovery>;

pub struct AgentRuntimeDiscovery {
    pub capabilities: AgentRuntimeCapabilities,
    pub models: ModelCatalog,
    pub warnings: Vec<String>,
}
```

**Discovery 失败不能影响 Agent launch**：

```
Codex model discovery failed → warning → ModelCatalog::Unavailable
                             → Agent Default + Custom 仍可使用
```

## 8. 持久化设计

继续使用现有 Settings KV，不需要 DB migration。key 使用 `Agent::as_str()` 的实际取值：

```
agent.runtime.codex
agent.runtime.claude_code
agent.runtime.pi
```

内容为 `{"model": null, "provider": null, "effort": null}`；也可以**没有 row**，`no row = all Agent defaults`。

不要保存 model catalog、Agent native default、detected effective model —— 这些都是动态信息。

**与旧 `assistant.*` key 的关系**：`assistant.agent/model/provider/effort` 属于旧 Assistant 配置，见 §15；v0.1 期间两套 key 并存，但旧 key 不再承载 runtime 语义，迁移策略为「一次性读入 → 校验 → 写成 override，读到即清空语义」，不做静默双写。

## 9. Tauri API

```
get_agent_runtime_settings(agent)
set_agent_runtime_overrides(agent, overrides)
refresh_agent_runtime_options(agent)
```

```ts
interface AgentRuntimeSettings {
  agent: Agent;
  detected: boolean;
  executable: string | null;
  version: string | null;
  overrides: { model: string | null; provider: string | null; effort: string | null };
  capabilities: {
    model: RuntimeFieldCapability;
    provider: RuntimeFieldCapability;
    effort: RuntimeFieldCapability;
  };
  models: ModelOption[];
  warnings: string[];
}
```

这里不存在 `nativeDefaultModel` / `effectiveModel`，v0.1 不需要。

## 10. Settings UI

继续使用现有 **Settings → Agents**，不新增 section。把只读安装状态升级为「安装状态 + Runtime」：

```
Codex                                              Detected
/path/to/codex                              v0.x

Runtime
──────────────────────────────────────────────────
Model      [ Agent default                    ▾ ]
Reasoning  [ Agent default                    ▾ ]
                                    [ Refresh models ]
```

Model 下拉：`○ Agent default` ───── `○ GPT-… ×N` `○ Custom…`。选中后显示 `Override: GPT-…`，并提示 *NoEnding will explicitly pass this model when launching Codex.* 恢复默认即 `model override = null`。

**Pi** 多一个 Provider 字段（Provider / Model / Thinking 三个选择器）；Provider 被显式 Override 时 Model 列表按 provider 过滤，但仍允许 Custom provider 与 Custom model。

## 11. Unsupported Options 必须显式拒绝

不要再允许「Claude + provider 然后静默忽略」。

```
Unsupported → UI 不展示 → backend 也拒绝
validate_runtime_overrides(agent, overrides)
  Claude: provider != None → Err
  Codex:  provider != None → Err
  Pi:     provider allowed
```

这防止了最坏的情况：用户以为自己测的是配置 A，实际上底层 Agent 根本没收到参数。

## 12. Launcher 统一消费 Runtime Override

架构收口点：

```
Settings → AgentRuntimeOverrides → Runtime Resolver → ExecOptions → AgentAdapter
```

```rust
pub fn runtime_exec_options(db: &Db, agent: Agent) -> Result<ExecOptions> {
    let overrides = get_runtime_overrides(db, agent)?;
    validate_runtime_overrides(agent, &overrides)?;
    Ok(ExecOptions {
        model: overrides.model,
        provider: overrides.provider,
        effort: overrides.effort,
    })
}
```

没有任何 default resolution。New / Resume 不再各自决定 model。

**实际改动范围（比看起来大）**：当前 `AgentAdapter::build_new_command` / `build_resume_command`（`adapters/mod.rs:403`）**没有** `ExecOptions` 参数，交互式会话今天天然不传任何 runtime 参数。本方案要给它加上（`exec` 路径已有该参数，形状与 §2 完全一致，无需新增结构）。这意味着 trait 签名变更 + 三个 adapter 实现 + 既有 launcher 测试同步调整。

## 13. Prepared Launch

`PreparedLaunch` freeze **Agent + NoEnding override intent**：

```rust
PreparedAgentRuntime {
    agent: Agent::Codex,
    model_override: None,
    effort_override: None,
}
```

Preview 显示 `Model: Agent default` → Launch 必须不传 `-m`；Preview 显示 `Model: Override GPT-X` → Launch 必须传 GPT-X。

Runtime invariant：

> **Preview override intent = Launch override intent.**

**落进既有不变量（硬性要求）**：override intent 必须是 `PreparedLaunch.state_fingerprint`（`launcher/mod.rs:44`）的成分之一。否则用户在 Preview 与 Launch 之间改了 Runtime 配置，状态指纹却不变，`Preview-Launch Identity` 会静默失效 —— 违反 AGENTS.md 中「what you preview is what the Agent receives」。Runtime 部分随 bundle 一起进入 `LaunchIntent`，Resume 的同一条路径同理。

## 14. Workspace Assistant 收敛（决策）

Assistant 是与 New / Resume / Context Eval 并列的**第四个消费者**（`assistant/mod.rs:148`、`assistant/mod.rs:262`），走的是同一条 `AssistantConfig → CliExtractor` 组合路径，因此有完全相同的跨 Agent 默认值泄漏。

v0.1 决策：**共用 override 语义，但保留 Assistant 自己的 agent 选择。**

- Assistant 仍然可以独立选择跑在哪个 Agent 上（这是产品功能，不是 runtime 配置）；
- 一旦选定 agent，其 model / provider / effort **只从该 Agent 的 `AgentRuntimeOverrides` 读取**，不再有第二份 model 配置；
- 删除 `AssistantConfig` 里硬编码的 `gpt-5.6-luna / openai-codex / low` 串味默认（`sync/extractor.rs:206-216`）；
- UI 侧 Assistant 面板不再自带 model 输入框，改为「沿用 Settings → Agents 的 Runtime 配置」的只读提示 + 跳转。

效果：`Assistant 在 Claude Code 上跑` 与 `New Session 在 Claude Code 上跑` 使用同一套 runtime 参数，二者不可能分叉。

## 15. Context Eval

`context-eval` 改用完全一样的语义。

现在的问题（已在代码中确认）：`AssistantConfig::default()` 给出 `codex + gpt-… + low`，`--agent claude_code` 只替换 agent 字段（`bin/context-eval.rs:220`），导致 Codex 的默认模型泄漏进 Claude 命令行。彻底去掉这种组合方式，CLI 参数直接映射成 override：

```rust
AgentRuntimeOverrides { model: args.model, provider: args.provider, effort: args.effort }
```

```
context-eval --extractor cli --agent claude_code
  → Model: Agent default / Effort: Agent default
  → claude -p ...

context-eval --extractor cli --agent claude_code --model sonnet --effort high
  → claude -p --model sonnet --effort high ...
```

**Eval 可复现性**：context-eval 的 override 只来自 CLI 参数，**默认不读用户 Settings**。共用的是 capability validation 与 ExecOptions conversion，不是配置来源。以后若需要再显式加 `--use-noending-settings`，而不是默认偷读 GUI 设置。

报告与 extractor 命名同步：现有 `cli:<agent>:<model|"default">`（`sync/extractor.rs:284`）改为区分意图 —— `cli:<agent>:agent-default` vs `cli:<agent>:override:<model>`，让 SyncRun 记录里能看出这次到底有没有传参。

## 16. AssistantConfig 的处置

概念拆开，不再让 `AssistantConfig` 承担 Agent Session runtime：

```
AssistantConfig        → NoEnding 内置 Assistant 的 agent 选择（仅此而已）
AgentRuntimeOverrides  → Codex / Claude / Pi 的 Session runtime
```

`CliExtractor` 的构造从 `CliExtractor::from_config(&AssistantConfig)` 改为：

```rust
CliExtractor::new(agent, ExecOptions)
// 或
CliExtractor::from_runtime(agent, &AgentRuntimeOverrides)
```

避免再次出现跨 Agent 默认值。

## 17. 实施顺序（6 个提交）

| # | Commit | 内容 | 测试 |
|---|---|---|---|
| 1 | `feat(agent-runtime): add agent runtime override model` | `AgentRuntimeOverrides` / `AgentRuntimeCapabilities` / `ModelCatalog` / KV 持久化 / validation / override → `ExecOptions` | `None => ExecOptions None`；Codex model override；Claude effort override；Pi provider/model/thinking override；unsupported field rejection；cross-Agent isolation |
| 2 | `feat(agent-runtime): add model discovery` | Codex dynamic、Pi dynamic、Claude suggested/freeform | discovery failure ≠ launch failure |
| 3 | `fix(adapters): pass effort to claude exec` | 补齐 `claude.rs` 的 `--effort`，停止静默丢弃 | 缺参数即 argv 断言 |
| 4 | `feat(settings): add agent runtime controls` | Settings → Agents 升级：Agent default / Override / discovery / Custom / Refresh | DTO 形状、override 往返 |
| 5 | `refactor(launcher): use shared agent runtime overrides` | New / Resume / `PreparedLaunch`；adapter trait 加 `ExecOptions`；override intent 进 `state_fingerprint`；runtime preview | Preview=Launch 不变量回归测试（改 override 后指纹必须变） |
| 6 | `refactor(assistant+context-eval): use agent runtime semantics` | §14 Assistant 收敛、§15 eval CLI→override、命名调整 | eval 不再泄漏 Codex 默认到 Claude；报告打印 `Agent default` / `Override: sonnet` |

Commit 3 故意提前：它是独立的小修复，且 Commit 1 的 Claude effort 断言依赖它。

## 18. v0.1 明确不做

不做：解析 Agent 默认配置文件、计算 Agent effective model、编辑 `~/.codex` / `~/.claude` / `~/.pi`、Workstream-specific override、Session-specific override、模型自动推荐、模型 benchmark、模型价格管理、自动选择「最佳模型」。

只做：**per-Agent global overrides + model discovery + Settings controls + single shared runtime semantics**（四个消费者共用）。

## 19. 最终架构

```
                    Model Discovery
                          │
                   advisory only
                          ↓
                   Settings → Agents
                          │
                          ↓
                AgentRuntimeOverrides
                          │
                  None = Agent default
                          ↓
               Runtime Override Resolver
                          │
        ┌─────────────┬───┴────────┬──────────────┐
        ↓             ↓            ↓              ↓
  New Session   Resume Session  Context Eval  Assistant
        │             │            │              │
        └────────── ExecOptions ───┴──────────────┘
                          │
                          ↓
                     AgentAdapter
                          │
                          ↓
              Codex / Claude Code / Pi
```

```
No override → no runtime CLI argument → Agent owns its native defaults
```

## 20. 实施落地记录（与上文设计的差异）

6 个提交全部落地后，以下几处与 v0.1 文本不一致，均为实现期确认过的有意选择，后续以本节为准。

1. **§7 discovery 是目录不是单文件**：`agent_runtime/discovery/`，每家 CLI 的模型目录来源与 parse 方式不同。
2. **§9 DTO 多两个字段**：`AgentRuntimeSettings` 带 `model_source`（`not_loaded | dynamic | suggested | unavailable`）和 `effort_levels`。理由：catalog 拉取失败时 effort 选择器仍要有候选值；`model_source` 决定 UI 用哪句提示（`SOURCE_NOTE`）。并且 `get_agent_runtime_settings` **不跑 discovery**（返回 `models: []` + `not_loaded`），catalog 只由 `refresh_agent_runtime_options` 提供 —— 该命令签名刻意不带 `State`，从结构上保证 discovery 不可能在 DB 锁内执行（AGENTS.md）。
3. **§6 Claude effort = `Suggested`**（原写 FreeForm）：候选 `low/medium/high/xhigh/max` 直接来自 `claude --help`，是稳定枚举。
4. **`discover_runtime_options(agent)` 返回值而非 `Result`**：discovery 是 advisory，失败表达为 `models: []` + `warnings`，因此调用方没有任何一条路径能因它失败。
5. **§15 extractor 命名**：实际是 `cli:<agent>:override:model=…,effort=…`，不是 `override:<model>`。Pi 的 provider、Codex 的 effort 都必须出现在名字里，单个 `<model>` 段无法表达意图。该文本、`PreparedLaunch` 的 runtime 摘要、launch audit 三处用的是同一个函数 `ExecOptions::override_summary()`，不可能分叉。
6. **§13 没有新增 `PreparedAgentRuntime`**：`PreparedLaunch` 直接加 `runtime: AgentRuntimeOverrides`。normalized override 本身就是冻结的意图，再包一层只是别名。
7. **§11 在两个消费者上的不同表现**：新增 `CliExtractor::try_for_agent(db, agent) -> Result<Option<CliExtractor>>`。Assistant（用户在等回答）拿到损坏的 override 行必须**报错**，不能静默退化成 retrieval-only；后台 Sync 保留宽松降级到 heuristic，因为同步中断的代价更高且结果可审计。`Ok(None)` 一律表示「Assistant 故意没选 Agent」。
8. **§8 迁移入口**：`migrate_legacy_assistant_runtime(db) -> Result<Option<Agent>>`，在 `lib.rs` setup 里调用一次，返回 `Some(agent)` 仅当确实写入了 override（旧 key 无论如何都会被删除，不留语义）。

## 附录 A：v0.1 落地的代码事实核对

| 事实 | 位置 | 对方案的影响 |
|---|---|---|
| `ExecOptions { model, provider, effort }` 已存在且形状与 §2 一致 | `adapters/mod.rs:75` | 无需新增结构，直接复用 |
| `build_exec_command` 三家已按「`Some` 才传参」实现 | `codex.rs:272` / `claude.rs:275` / `pi.rs:247` | headless 路径已符合不变量 |
| `build_new_command` / `build_resume_command` 无 `ExecOptions` 参数 | `adapters/mod.rs:403` | §12 需要 trait 签名变更 |
| `claude.rs` 丢弃 `opts.effort` | `adapters/claude.rs:282` | §6 前置修复 = Commit 3 |
| `AssistantConfig::default()` = codex + gpt-5.6-luna + openai-codex + low | `sync/extractor.rs:206` | §14/§15 泄漏根因 |
| context-eval 只覆盖 `cfg.agent`，其余沿用 default | `bin/context-eval.rs:220` | §15 修复对象 |
| `PreparedLaunch.state_fingerprint` 已存在 | `launcher/mod.rs:44` | §13 复用，无需新增字段语义 |
| Codex effort 以 `model_reasoning_effort="high"` 作为**单个 argv**；平台层单引号包裹 | `codex.rs:288` + `platform/launcher.rs:30` | 已核对：不生成 shell 片段，符合 AGENTS.md，无需改动 |
| Settings KV 已有 `get_setting/set_setting` | `storage` + `settings/mod.rs:19` | §8 无需 migration |
