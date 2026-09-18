import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import SourcesSettings from "./SourcesSettings";
import AgentRuntimeRow from "./AgentRuntimeSettings";
import { refreshBaseExperience, useBaseExperience } from "../../app/experience";
import { AGENT_LABELS, type Agent, type AppInfo, type ContextDeliveryLevel } from "../../types";
import type { Route, SettingsSection } from "../../app/routes";

const SECTIONS: { key: SettingsSection; label: string }[] = [
  { key: "general", label: "通用" },
  { key: "agents", label: "Agent" },
  { key: "sources", label: "Session 来源" },
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
          所有 Workstream 卡片与 Sessions 里的新建 / 继续都使用这个 Agent，不再每次选择。
        </p>
        <div className="settings-agents">
          {(Object.keys(AGENT_LABELS) as Agent[]).map((a) => (
            <button key={a}
              className={`settings-agent-row ${defaultAgent === a ? "selected" : ""}`}
              onClick={() => choose(a)}>
              <AgentIcon agent={a} size={16} />
              <span className="grow">{AGENT_LABELS[a]}</span>
              {agents[a] && !agents[a].detected && (
                <span className="muted small">未检测到</span>
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
            未检测到任何 Agent CLI，新建 / 继续已停用。安装任意 Agent CLI 后重启应用即可启用。
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
            <div className="settings-row-label">启动 Session 前确认</div>
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
  { key: "off", label: "关闭", hint: "不把 NoEnding 的 Workstream Context 送进 Agent 会话。" },
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
        NoEnding 首先是一个可靠的本地工作空间：发现 Session、留下历史、随时继续。
        下面这个开关决定它是否额外去自动理解你的工作。
      </p>
      <div className="row-line">
        <div>
          <div className="settings-row-label">Context 智能处理</div>
          <div className="settings-row-hint">
            提取 Context 变更、自动归类 Workstream、生成待审阅与冲突。关闭时 Session 仍会被摄入和索引，
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
        控制新建 / 继续 Agent 会话时，NoEnding 送进去多少 Workstream Context。
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
          <div className="settings-row-label">Session 摄入与索引</div>
          <div className="settings-row-hint">发现 Session、存下事件、建立搜索索引，始终运行。</div>
        </div>
        <span className="badge success">开</span>
      </div>
      <div className="row-line">
        <div>
          <div className="settings-row-label">Context 提取与自动归类</div>
          <div className="settings-row-hint">自动归类只影响未显式绑定的 Session；你的手动指定优先。</div>
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

/** Data & Advanced（§62）：数据库位置 + 实验性功能（含 Context 相关设置，§11.7）。 */
function AdvancedSettings() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    api.getAppInfo().then(setInfo).catch(console.error);
  }, []);

  return (
    <>
      <IntelligenceSettings />
      <ContextDeliverySettings />
      <AutomationSettings />
      <section>
        <h3>数据库</h3>
        <div className="row-line">
          <div>
            <div className="settings-row-label">数据库路径</div>
            <div className="settings-row-hint mono" style={{ wordBreak: "break-all" }}>
              {info?.db_path ?? "…"}
            </div>
          </div>
        </div>
        <div className="row-line">
          <div>
            <div className="settings-row-label">数据目录</div>
            <div className="settings-row-hint mono" style={{ wordBreak: "break-all" }}>
              {info?.app_data_dir ?? "…"}
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
