import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SourcesSettings from "./SourcesSettings";
import AgentRuntimeRow from "./AgentRuntimeSettings";
import { refreshBaseExperience, useBaseExperience } from "../../app/experience";
import { AGENT_LABELS, type Agent, type ContextDeliveryLevel, type WorkspaceSettings } from "../../types";
import type { Route, SettingsSection } from "../../app/routes";

const SECTIONS: { key: SettingsSection; label: string }[] = [
  { key: "general", label: "通用" },
  { key: "agents", label: "Agent" },
  { key: "sources", label: "会话来源" },
  { key: "appearance", label: "外观" },
  { key: "advanced", label: "数据与高级" },
];

/**
 * Settings（整体设计方案 §56-§62）：Main 内部二级导航 + 内容区。
 * 只暴露真正有用户价值的设置；实现细节（threshold/authority/cursor）不进 UI。
 *
 * Base Experience（方案 v0.1 §11.7）：Context 相关设置不再是普通入口，
 * 统一收进「数据与高级 → 实验性功能」。
 */
export default function SettingsView({ section, navigate }: {
  section: SettingsSection;
  navigate: (r: Route) => void;
}) {
  const current = SECTIONS.some((s) => s.key === section) ? section : "general";

  return (
    <div className="main narrow">
      <PageHeader title="设置" />
      <div className="settings-layout">
        <nav className="settings-nav">
          {SECTIONS.map((s) => (
            <button key={s.key}
              className={`nav-item ${current === s.key ? "active" : ""}`}
              onClick={() => navigate({ view: "settings", section: s.key })}>
              {s.label}
            </button>
          ))}
        </nav>
        <div className="settings-pane">
          {current === "general" && <GeneralSettings />}
          {current === "agents" && <AgentsSettings />}
          {current === "sources" && <SourcesSettings />}
          {current === "appearance" && <AppearanceSettings />}
          {current === "advanced" && <AdvancedSettings />}
        </div>
      </div>
    </div>
  );
}

/** General：Default Agent 是最重要设置（§57）；Startup Page 第一版固定 Home。 */
function GeneralSettings() {
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [agents, setAgents] = useState<Record<string, { detected: boolean }>>({});

  useEffect(() => {
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
    api.getAgentStatus().then(setAgents).catch(console.error);
  }, []);

  const choose = async (agent: Agent) => {
    await api.setDefaultAgent(agent).catch(console.error);
    setDefaultAgent(agent);
  };

  const selectedUndetected =
    defaultAgent !== null && agents[defaultAgent]?.detected === false;

  return (
    <>
      <section>
        <h3 style={{ marginTop: 0 }}>默认 Agent</h3>
        <p className="muted small" style={{ marginTop: 0 }}>
          所有任务卡片与会话里的新建 / 继续都使用这个 Agent，不再每次选择。
        </p>
        <div className="settings-agents">
          {(Object.keys(AGENT_LABELS) as Agent[]).map((a) => (
            <button key={a}
              className={`settings-agent-row ${defaultAgent === a ? "selected" : ""}`}
              onClick={() => choose(a)}>
              <AgentIcon agent={a} size={16} />
              <span className="grow">{AGENT_LABELS[a]}</span>
              {agents[a] && !agents[a].detected && (
                <span className="muted small">未检测</span>
              )}
              {defaultAgent === a && <span className="muted small">默认</span>}
            </button>
          ))}
        </div>
        {selectedUndetected && (
          <p className="muted small" style={{ color: "var(--warning)", marginBottom: 0 }}>
            当前默认 Agent 未在本机检测到，新建 / 继续会失败。请安装它，或改选其他已检测的 Agent。
          </p>
        )}
        {defaultAgent === null && (
          <p className="muted small" style={{ marginBottom: 0 }}>
            未检测到任何 Agent CLI，新建 / 继续无法启动。安装任意 Agent CLI 后即可恢复。
          </p>
        )}
      </section>
      <section>
        <h3>启动</h3>
        <div className="row-line">
          <div>
            <div className="settings-row-label">启动页面</div>
            <div className="settings-row-hint">应用启动固定进入首页，继续最近的工作。</div>
          </div>
          <span className="badge">首页</span>
        </div>
        <div className="row-line">
          <div>
            <div className="settings-row-label">启动会话前确认</div>
            <div className="settings-row-hint">新建 / 继续一键直达，不经确认页。</div>
          </div>
          <span className="badge">关闭</span>
        </div>
      </section>
    </>
  );
}

/** Agents：安装状态 + Runtime Override（§10）。NoEnding 不解析 Agent 默认配置。 */
function AgentsSettings() {
  return (
    <section>
      <h3 style={{ marginTop: 0 }}>Agent</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        本机检测到的 Agent CLI。未检测到的 Agent 不可启动。
        Runtime 每个字段默认都是 Agent 默认值 —— NoEnding 不传对应参数，也不猜测 Agent 的默认模型。
      </p>
      {(Object.keys(AGENT_LABELS) as Agent[]).map((a) => (
        <AgentRuntimeRow key={a} agent={a} />
      ))}
    </section>
  );
}

const DELIVERY_LEVELS: { key: ContextDeliveryLevel; label: string; hint: string }[] = [
  { key: "off", label: "关闭", hint: "不把 NoEnding 的任务 Context 送进 Agent 会话。" },
  { key: "compact", label: "精简", hint: "只送最重要的当前 Context 与最近变更。" },
  { key: "balanced", label: "均衡", hint: "送核心 Context 加少量相关信息。" },
  { key: "detailed", label: "详细", hint: "在需要更多背景时送更广的支撑信息。" },
];

/** 智能处理开关（§11.1）。它与注入梯度是两个正交开关。 */
function IntelligenceSettings() {
  const { intelligenceEnabled } = useBaseExperience();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const change = async (next: boolean) => {
    if (busy || next === intelligenceEnabled) return;
    setBusy(true);
    setError(null);
    try {
      await api.setContextIntelligenceEnabled(next);
      await refreshBaseExperience();
    } catch (err) {
      console.error(err);
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section>
      <h3>实验性功能</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        NoEnding 首先是一个可靠的本地工作空间：发现会话、留下历史、随时继续。
        下面这个开关决定它是否额外去自动理解你的工作。
      </p>
      <div className="row-line">
        <div>
          <div className="settings-row-label">Context 智能处理</div>
          <div className="settings-row-hint">
            提取 Context 变更、自动归类任务、生成待审阅与冲突。关闭时会话仍会被摄入和索引，
            已有的 Context 与历史不会丢失；重新开启后从冻结的处理位置继续。
          </div>
        </div>
        <div className="settings-seg">
          <button disabled={busy} className={intelligenceEnabled ? "on" : ""} onClick={() => change(true)}>开启</button>
          <button disabled={busy} className={intelligenceEnabled ? "" : "on"} onClick={() => change(false)}>关闭</button>
        </div>
      </div>
      {error && (
        <p className="small" style={{ color: "var(--danger)", marginBottom: 0 }}>{error}</p>
      )}
    </section>
  );
}

/** Context Delivery：注入梯度（实验区，§11.7）。 */
function ContextDeliverySettings() {
  const [level, setLevel] = useState<ContextDeliveryLevel>("off");
  const [loading, setLoading] = useState<boolean>(true);
  const [saving, setSaving] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    api.getContextDeliveryLevel()
      .then((lvl) => {
        if (active) setLevel(lvl);
      })
      .catch((err) => {
        if (active) {
          console.error(err);
          setError(String(err));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  const changeLevel = async (next: ContextDeliveryLevel) => {
    if (saving || loading || next === level) return;
    const prev = level;
    setSaving(true);
    setLevel(next);
    setError(null);
    try {
      await api.setContextDeliveryLevel(next);
      await refreshBaseExperience();
    } catch (err) {
      console.error(err);
      setLevel(prev);
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const currentHint = DELIVERY_LEVELS.find((d) => d.key === level)?.hint;

  return (
    <section>
      <h3 style={{ marginTop: 0 }}>Context 注入</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        控制新建 / 继续会话时，NoEnding 送进去多少任务 Context。
        它只影响对外注入，不会停止摄入与同步。
      </p>
      <div className="settings-seg">
        {DELIVERY_LEVELS.map((d) => (
          <button
            key={d.key}
            disabled={loading || saving}
            className={level === d.key ? "on" : ""}
            onClick={() => changeLevel(d.key)}
          >
            {d.label}
          </button>
        ))}
      </div>
      {currentHint && (
        <p className="muted small" style={{ marginBottom: 0, marginTop: 8 }}>
          {currentHint}
        </p>
      )}
      {error && (
        <p className="small" style={{ color: "var(--danger)", marginBottom: 0, marginTop: 8 }}>
          {error}
        </p>
      )}
    </section>
  );
}

/** 自动化：只读说明，状态必须是真的（§6）。 */
function AutomationSettings() {
  const { intelligenceEnabled, deliveryLevel } = useBaseExperience();
  const deliveryLabel = DELIVERY_LEVELS.find((d) => d.key === deliveryLevel)?.label ?? deliveryLevel;
  return (
    <section>
      <h3 style={{ marginTop: 0 }}>自动化</h3>
      <div className="row-line">
        <div>
          <div className="settings-row-label">会话摄入与索引</div>
          <div className="settings-row-hint">发现会话、存下事件、建立搜索索引，始终运行。</div>
        </div>
        <span className="badge success">开</span>
      </div>
      <div className="row-line">
        <div>
          <div className="settings-row-label">Context 提取与自动归类</div>
          <div className="settings-row-hint">自动归类只影响未显式绑定的会话；你的手动指定优先。</div>
        </div>
        <span className={`badge ${intelligenceEnabled ? "success" : ""}`}>
          {intelligenceEnabled ? "开" : "关"}
        </span>
      </div>
      <div className="row-line">
        <div>
          <div className="settings-row-label">Context 注入</div>
          <div className="settings-row-hint">由「实验性功能 → Context 注入」决定送多少。</div>
        </div>
        <span className={`badge ${deliveryLevel === "off" ? "" : "success"}`}>{deliveryLabel}</span>
      </div>
      <div className="row-line">
        <div>
          <div className="settings-row-label">后台补摄</div>
          <div className="settings-row-hint">应用启动时补摄离开期间产生的会话内容。</div>
        </div>
        <span className="badge success">开</span>
      </div>
    </section>
  );
}

/** Appearance：Theme（tokens 支持暗色，§61/§88）；Density 暂缓。 */
type Theme = "system" | "light" | "dark";
const THEME_LABELS: Record<Theme, string> = { system: "跟随系统", light: "浅色", dark: "深色" };
function AppearanceSettings() {
  const [theme, setTheme] = useState<Theme>(() => {
    const saved = localStorage.getItem("noending.theme");
    return saved === "light" || saved === "dark" ? saved : "system";
  });

  const apply = (t: Theme) => {
    setTheme(t);
    if (t === "system") {
      localStorage.removeItem("noending.theme");
      delete document.documentElement.dataset.theme;
    } else {
      localStorage.setItem("noending.theme", t);
      document.documentElement.dataset.theme = t;
    }
  };

  return (
    <section>
      <h3 style={{ marginTop: 0 }}>主题</h3>
      <div className="settings-seg">
        {(Object.keys(THEME_LABELS) as Theme[]).map((t) => (
          <button key={t} className={theme === t ? "on" : ""} onClick={() => apply(t)}>
            {THEME_LABELS[t]}
          </button>
        ))}
      </div>
      <p className="muted small" style={{ marginBottom: 0 }}>
        跟随系统时自动切换浅色 / 深色。
      </p>
    </section>
  );
}

/** Data & Advanced（§62）：NoEnding Home + 实验性功能（含 Context 相关设置，§11.7）。 */
function AdvancedSettings() {
  return (
    <>
      <IntelligenceSettings />
      <ContextDeliverySettings />
      <AutomationSettings />
      <WorkspaceStorageSettings />
    </>
  );
}

/* ------------------------------------------------------------------ *
 * NoEnding Home（方案 §2 / §3 / §4 / §22）
 *
 * 这一屏只有一个硬规矩：**说的必须是当前真正在写的那一份**。
 * `get_workspace_settings` 读的是启动时解析并 manage 进应用的 Home，
 * 不是重新解析一遍（§42.3-M14：迁移没跑完就报新路径，UI 就在说谎）。
 * 改位置因此不切库，只写 bootstrap 指针的 `pending_home`，
 * 下一次启动、打开数据库之前才移动（§3），所以 `restart_required`
 * 与 `pending_home` 必须原样显示出来，不能吞掉。
 * ------------------------------------------------------------------ */

/** `home_source` 的中文说明（§11 冻结的三个取值；未知值原样显示，不留空白）。 */
function homeSourceLabel(source: string | null | undefined): string {
  switch (source) {
    case "explicit_env":
      return "由环境变量 NOENDING_HOME 指定";
    case "bootstrap":
      return "由 NoEnding 的启动指针记录";
    case "default_home":
      return "系统用户目录下的默认位置";
    case null:
    case undefined:
    case "":
      return "未知来源";
    default:
      return source;
  }
}

function WorkspaceStorageSettings() {
  const [settings, setSettings] = useState<WorkspaceSettings | null>(null);
  const [failed, setFailed] = useState(false);
  const [editing, setEditing] = useState(false);

  const load = useCallback(() => {
    api.getWorkspaceSettings()
      .then((s) => { setSettings(s); setFailed(false); })
      .catch((e) => { console.error(e); setFailed(true); });
  }, []);
  useEffect(load, [load]);

  if (!settings) {
    return (
      <section>
        <h3>NoEnding Home</h3>
        {failed ? (
          <>
            <div className="l1-none">
              读取工作位置失败。本地数据没有被修改，也没有任何东西被移动；可以重试。
            </div>
            <div className="invite">
              <button className="btn small" onClick={load}>重试</button>
            </div>
          </>
        ) : (
          <div className="muted small">读取中…</div>
        )}
      </section>
    );
  }

  /* restart_required 与 pending_home 是两个信号，各自都要露出来：
     今天后端让 `restart_required = pending_home.is_some()`，但任一字段单独出现
     都意味着有件事还没落地，不能被另一个字段的空值吞掉。 */
  const pending = settings.pending_home;
  const relocationPending = settings.restart_required || pending !== null;
  const envPinned = settings.home_source === "explicit_env";

  return (
    <section>
      <h3>NoEnding Home</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        NoEnding Home 是 NoEnding 放在磁盘上的数据根目录：数据库、运行文件和日志都在它下面，
        新建会话的默认工作目录是它里面的 <span className="mono">workspace/</span>。
      </p>

      <PathRow
        label="NoEnding Home"
        value={settings.noending_home}
        hint={homeSourceLabel(settings.home_source)}
        action={
          <div className="row" style={{ gap: 8 }}>
            {relocationPending && <span className="badge warn">待重启生效</span>}
            <button className="btn small" onClick={() => setEditing(true)}>更改位置…</button>
          </div>
        }
      />
      <PathRow label="默认工作目录" value={settings.default_workspace} />
      <PathRow
        label="数据库路径"
        value={settings.db_path}
        hint={relocationPending ? "当前仍在写入的位置；迁移在下次启动时进行。" : undefined}
      />
      <div className="row" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn small ghost" onClick={load}>重新读取</button>
        {failed && (
          <span className="badge warn" title="刚才这次重新读取失败了；上面显示的是上一次成功读到的内容。">
            重新读取失败，显示的是上一次的结果
          </span>
        )}
      </div>

      {envPinned && (
        <WarnCallout title="环境变量优先级更高">
          <div>
            当前 Home 由环境变量 <span className="mono">NOENDING_HOME</span> 决定，它的优先级高于
            NoEnding 自己记录的指针（<span className="mono">NOENDING_HOME → 启动指针 → 默认位置</span>）。
          </div>
          <div>
            在取消这个变量之前，「更改位置」写下的新路径在下次启动也不会生效。
          </div>
        </WarnCallout>
      )}

      {relocationPending && (
        <WarnCallout title="迁移已安排，重启后生效">
          <div>
            {pending ? (
              <>
                下次启动时 NoEnding 会把数据搬到{" "}
                <span className="mono" style={{ wordBreak: "break-all" }}>{pending}</span>
                ，搬完才打开数据库。
              </>
            ) : (
              <>
                位置变更已经登记，但没有读到目标路径（点「重新读取」再看一次）。
                在此之前搬迁不会发生。
              </>
            )}
            当前进程仍在写入上面那个数据库路径。
          </div>
          <div>
            会搬的只有 NoEnding 自己拥有的 <span className="mono">data/</span>、
            <span className="mono">runtime/</span>、<span className="mono">logs/</span>
            ；跨磁盘时会复制并保留原件。
          </div>
          <div>
            <strong>旧 Home 的 <span className="mono">workspace/</span> 里的文件不会被移动。</strong>
            它们留在原处，那个目录之后作为一个普通工作目录被识别（它仍然带着自己的项目与历史）。
          </div>
          <div>
            新 Home 下的 <span className="mono">workspace/</span> 会成为新的默认工作目录。
          </div>
        </WarnCallout>
      )}

      {editing && (
        <ChangeHomeModal
          current={settings}
          onClose={() => setEditing(false)}
          onSaved={(next) => { setSettings(next); setEditing(false); }}
        />
      )}
    </section>
  );
}

/** 一行「标签 + 完整路径」；路径一律整段可换行显示，绝不省略成看不见的样子。 */
function PathRow({ label, value, hint, action }: {
  label: string;
  value: string | null | undefined;
  hint?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="row-line">
      <div style={{ minWidth: 0 }}>
        <div className="settings-row-label">{label}</div>
        <div className="settings-row-hint mono" style={{ wordBreak: "break-all" }}>
          {value && value.trim() !== "" ? value : "—"}
        </div>
        {hint && <div className="settings-row-hint" style={{ wordBreak: "break-word" }}>{hint}</div>}
      </div>
      {action && <div style={{ flex: "none" }}>{action}</div>}
    </div>
  );
}

/** 成段的警告块：仓库里还没有公共 callout 组件，先用现有 tokens 拼一个。 */
function WarnCallout({ title, children }: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{
      marginTop: 12,
      padding: "10px 12px",
      borderRadius: "var(--radius-sm)",
      border: "1px solid var(--warning)",
      background: "var(--warning-wash)",
      color: "var(--text-primary)",
      fontSize: 12.5,
      lineHeight: 1.7,
      display: "grid",
      gap: 4,
    }}>
      <div style={{ color: "var(--warning)", fontWeight: 600 }}>{title}</div>
      {children}
    </div>
  );
}

/**
 * 更改 NoEnding Home（方案 §3、§4）：只登记下一次启动要搬去的地方。
 *
 * 这里不说「已迁移」，也不给「立即生效」—— 在同一个进程里换数据库会留下两份
 * 分叉的写入（§3）。同样要在按钮之前说清的两句：只搬应用拥有的
 * data/ runtime/ logs/，旧 Home 的 workspace/ 里的用户文件不动。
 */
function ChangeHomeModal({ current, onClose, onSaved }: {
  current: WorkspaceSettings;
  onClose: () => void;
  onSaved: (next: WorkspaceSettings) => void;
}) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const trimmed = path.trim();
  const sameAsCurrent = trimmed !== "" && trimmed === current.noending_home.trim();
  const canSubmit = trimmed !== "" && !sameAsCurrent && !busy;

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError("");
    try {
      onSaved(await api.setNoendingHome(trimmed));
      showToast("迁移已安排：重启 NoEnding 后生效。");
    } catch (e) {
      console.error(e);
      // 后端会拒绝空路径、相对路径与「和当前位置相同」，理由原样给用户，不静默回退。
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="更改 NoEnding Home" onClose={onClose}>
      <label className="field">
        <span>新的位置</span>
        <input
          type="text"
          className="mono"
          value={path}
          placeholder={current.noending_home}
          autoFocus
          onChange={(e) => { setPath(e.target.value); setError(""); }}
          onKeyDown={(e) => { if (e.key === "Enter") void submit(); }}
          disabled={busy}
        />
      </label>
      <div className="settings-row-hint" style={{ marginBottom: 10, wordBreak: "break-word" }}>
        当前：<span className="mono" style={{ wordBreak: "break-all" }}>{current.noending_home}</span>
        （{homeSourceLabel(current.home_source)}）。支持 <span className="mono">~</span> 开头的写法；
        相对路径会被拒绝，请给一个绝对路径。
      </div>

      <WarnCallout title="这会怎样发生">
        <div>
          现在这一步只是<strong>登记</strong>：重启 NoEnding 后，它会在打开数据库之前完成搬迁，
          搬迁成功之前不会切过去。所以这一项<strong>要重启才生效</strong>。
        </div>
        <div>
          搬走的是 <span className="mono">data/</span>（数据库）、<span className="mono">runtime/</span>、
          <span className="mono">logs/</span>。
        </div>
        <div>
          如果新位置上<strong>已经有</strong> NoEnding 数据，它不会被覆盖：下次启动会直接打开那里的数据库，
          而现在这份会原样留在旧 Home 里（没有丢，只是不再被这个应用读取）。
        </div>
        <div>
          <strong>旧 Home 的 <span className="mono">workspace/</span> 里的文件不会被移动</strong>
          ：它们留在原来的位置，那个目录此后只是一个普通的工作目录。要保留它们，就自己把它们搬到
          新 Home 的 <span className="mono">workspace/</span> 下面，或者继续用原路径打开。
        </div>
        <div>
          新的默认工作目录会是新 Home 下的 <span className="mono">workspace/</span>，
          已有会话与任务的历史不会因为这次搬迁被改写。
        </div>
        {current.home_source === "explicit_env" && (
          <div>
            注意：<span className="mono">NOENDING_HOME</span> 环境变量仍然优先。只要它还在，
            下次启动依旧会打开它指定的那个 Home。
          </div>
        )}
      </WarnCallout>

      {sameAsCurrent && (
        <div className="muted small" style={{ marginTop: 10 }}>
          这就是当前的位置，不需要迁移。
        </div>
      )}
      {error && (
        <div className="badge warn" style={{
          marginTop: 10, display: "block", padding: "8px 10px", lineHeight: 1.6,
          wordBreak: "break-word",
        }}>
          {error}
        </div>
      )}

      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn" onClick={onClose} disabled={busy}>取消</button>
        <button className="btn primary" disabled={!canSubmit} onClick={submit}>
          {busy ? "登记中…" : "登记并在下次启动迁移"}
        </button>
      </div>
    </Modal>
  );
}
