import Icon from "../../components/Icon";
import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SourcesSettings from "./SourcesSettings";
import AgentRuntimeRow from "./AgentRuntimeSettings";
import { AGENT_LABELS, type Agent, type IngestionDiagnostic, type WorkspaceSettings } from "../../types";
import type { Route, SettingsSection } from "../../app/routes";

const SECTIONS: { key: SettingsSection; label: string; icon: "settings" | "spark" | "folder" | "palette" | "database" }[] = [
  { key: "general", icon: "settings", label: "通用" },
  { key: "agents", icon: "spark", label: "Agent" },
  { key: "sources", icon: "folder", label: "会话来源" },
  { key: "appearance", icon: "palette", label: "外观" },
  { key: "advanced", icon: "database", label: "数据与高级" },
];

/** Settings：Main 内部二级导航 + 内容区。只暴露有用户价值的设置。
 *  实现细节（threshold/authority/cursor）不进 UI。 */
export default function SettingsView({ section, navigate }: {
  section: SettingsSection;
  navigate: (r: Route) => void;
}) {
  const current = SECTIONS.some((s) => s.key === section) ? section : "general";

  return (
    <div className="main settings-page">
      <PageHeader title="设置" />
      <div className="settings-layout">
        <nav className="settings-nav" aria-label="设置分类">
          {SECTIONS.map((s) => (
            <button key={s.key}
              aria-current={current === s.key ? "page" : undefined}
              className={`nav-item ${current === s.key ? "active" : ""}`}
              onClick={() => navigate({ view: "settings", section: s.key })}>
              <Icon name={s.icon} />{s.label}
            </button>
          ))}
        </nav>
        <div className="settings-pane">
          {current === "general" && <GeneralSettings />}
          {current === "agents" && <AgentsSettings />}
          {current === "sources" && (
            <>
              <SourcesSettings />
              <IngestionDiagnosticsSection />
            </>
          )}
          {current === "appearance" && <AppearanceSettings />}
          {current === "advanced" && <AdvancedSettings />}
        </div>
      </div>
    </div>
  );
}

/** 摄入诊断：无法归属到任何会话的内部执行源（child / side 等）。只展示、无动作——
 *  它们不是会话，不参与搜索、上下文与归属。默认只看反复出现的（minObservations 2）。 */
function IngestionDiagnosticsSection() {
  const [rows, setRows] = useState<IngestionDiagnostic[] | null>(null);

  const load = useCallback(() => {
    api.listIngestionDiagnostics()
      .then(setRows)
      .catch((e) => { console.error(e); setRows([]); });
  }, []);
  useEffect(load, [load]);
  // 后台摄入随时可能新观测到问题，刷新信号到了就重读。
  useRefreshSignal(load);

  return (
    <section className="rail-section" style={{ marginTop: 26 }}>
      <div className="section-label">摄入诊断</div>
      <div className="muted small" style={{ marginTop: 6 }}>
        这些是无法归属到任何会话的内部执行源；它们不是会话，不参与搜索、上下文与归属。
      </div>

      {rows === null && <div className="muted small" style={{ marginTop: 10 }}>读取中…</div>}
      {rows !== null && rows.length === 0 && (
        <div className="muted small" style={{ marginTop: 10 }}>没有反复出现的摄入问题。</div>
      )}

      {rows !== null && rows.map((d) => (
        <div className="row-line" key={d.id}>
          <div style={{ minWidth: 0 }}>
            <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
              <span className="small">{AGENT_LABELS[d.agent]} · {d.kind}</span>
              <span className="badge">出现 {d.observation_count} 次</span>
            </div>
            <div className="settings-row-hint" style={{ wordBreak: "break-word" }}>{d.reason}</div>
            {d.source_path && (
              <div className="settings-row-hint mono" title={d.source_path} style={{ wordBreak: "break-all" }}>
                {truncatePath(d.source_path, 72)}
              </div>
            )}
            <div className="settings-row-hint">
              首次出现 {timeAgo(d.first_seen_at)} · 最近出现 {timeAgo(d.last_seen_at)}
            </div>
          </div>
        </div>
      ))}
    </section>
  );
}

/** 长路径取尾段省略：认路径靠的是末段，头部截断会把最有信息量的部分吃掉。 */
function truncatePath(path: string, max: number): string {
  if (path.length <= max) return path;
  return `…${path.slice(-(max - 1))}`;
}

/** General：Default Agent 是最重要设置；Startup Page 第一版固定 Home。 */
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
          用于新建会话；继续会话使用原来的 Agent。
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

/** Agents：安装状态 + Runtime Override。NoEnding 不解析 Agent 默认配置。 */
function AgentsSettings() {
  return (
    <section>
      <h3 style={{ marginTop: 0 }}>Agent</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        管理本机 Agent 和启动参数，未修改的选项沿用 Agent 默认值。
      </p>
      {(Object.keys(AGENT_LABELS) as Agent[]).map((a) => (
        <AgentRuntimeRow key={a} agent={a} />
      ))}
    </section>
  );
}

/** 自动化：只读说明，状态必须是真的。 */
function AutomationSettings() {
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
          <div className="settings-row-label">Context 更新</div>
          <div className="settings-row-hint">
            不再自动提取：在 Session 或任务页面点击「生成 / 更新摘要」与「更新状态」时才调用模型。
          </div>
        </div>
        <span className="badge">手动</span>
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

/** Appearance：主题设置（tokens 支持暗色）；Density 暂缓。 */
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
      <div className="theme-options" role="group" aria-label="主题">
        {(Object.keys(THEME_LABELS) as Theme[]).map((t) => (
          <button key={t} className={theme === t ? "on" : ""} aria-pressed={theme === t} onClick={() => apply(t)}>
            <span className={`theme-preview theme-preview-${t}`} aria-hidden="true"><span /><span /></span>
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

/** Data & Advanced：NoEnding Home + 自动化说明。 */
function AdvancedSettings() {
  return (
    <>
      <AutomationSettings />
      <WorkspaceStorageSettings />
    </>
  );
}

/* NoEnding Home：说的必须是当前真正在写的那一份。`get_workspace_settings` 读的是
 * 启动时解析并 manage 进应用的 Home（不重新解析）；改位置不切库，只写 bootstrap
 * 指针的 `pending_home`，下次启动打开数据库前才移动——所以 `restart_required` 与
 * `pending_home` 必须原样显示，不能吞掉。 */

/** `home_source` 的中文说明。 */
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
              读取数据位置失败，请重试。
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

  /* restart_required 与 pending_home 是两个信号，各自都要露出来：任一字段单独出现
     都意味着有件事还没落地，不能被另一个字段的空值吞掉。 */
  const pending = settings.pending_home;
  const relocationPending = settings.restart_required || pending !== null;
  const envPinned = settings.home_source === "explicit_env";

  return (
    <section>
      <h3>NoEnding Home</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        存放数据库、运行文件和日志；默认工作目录为 <span className="mono">workspace/</span>。
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
 * 更改 NoEnding Home：只登记下一次启动要搬去的地方（同进程换库会留下两份分叉写入）。
 * 说清两句：只搬应用拥有的 data/ runtime/ logs/，旧 Home 的 workspace/ 用户文件不动。
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
          onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing && e.nativeEvent.keyCode !== 229) void submit(); }}
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
