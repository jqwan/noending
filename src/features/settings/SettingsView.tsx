import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import SourcesSettings from "./SourcesSettings";
import { AGENT_LABELS, type Agent, type AppInfo, type ContextDeliveryLevel } from "../../types";
import type { Route, SettingsSection } from "../../app/routes";

const SECTIONS: { key: SettingsSection; label: string }[] = [
  { key: "general", label: "General" },
  { key: "agents", label: "Agents" },
  { key: "sources", label: "Session Sources" },
  { key: "sync", label: "Context & Sync" },
  { key: "appearance", label: "Appearance" },
  { key: "advanced", label: "Data & Advanced" },
];

/**
 * Settings（整体设计方案 §56-§62）：Main 内部二级导航 + 内容区。
 * 只暴露真正有用户价值的设置；实现细节（threshold/authority/cursor）不进 UI。
 */
export default function SettingsView({ section, navigate }: {
  section: SettingsSection;
  navigate: (r: Route) => void;
}) {
  const current = SECTIONS.some((s) => s.key === section) ? section : "general";

  return (
    <div className="main narrow">
      <PageHeader title="Settings" />
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
          {current === "sync" && <ContextSyncSettings />}
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
        <h3 style={{ marginTop: 0 }}>Default Agent</h3>
        <p className="muted small" style={{ marginTop: 0 }}>
          所有 Workstream 卡片与 Sessions 里的 New / Start 都使用这个 Agent，不再每次选择。
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
            当前默认 Agent 未在本机检测到，New / Start 会失败。请安装它，或改选其他已检测的 Agent。
          </p>
        )}
        {defaultAgent === null && (
          <p className="muted small" style={{ marginBottom: 0 }}>
            未检测到任何 Agent CLI，New / Start 已停用。安装任意 Agent CLI 后重启应用即可启用。
          </p>
        )}
      </section>
      <section>
        <h3>Startup</h3>
        <div className="row-line">
          <div>
            <div className="settings-row-label">Startup Page</div>
            <div className="settings-row-hint">应用启动固定进入 Home，继续最近的工作。</div>
          </div>
          <span className="badge">Home</span>
        </div>
        <div className="row-line">
          <div>
            <div className="settings-row-label">Confirm before launching Session</div>
            <div className="settings-row-hint">New / Resume 一键直达，不经确认页。</div>
          </div>
          <span className="badge">Off</span>
        </div>
      </section>
    </>
  );
}

/** Agents：只读检测状态（§48）。 */
function AgentsSettings() {
  const [agents, setAgents] = useState<Record<string, { name: string; detected: boolean; executable: string | null; version: string | null }>>({});

  useEffect(() => {
    api.getAgentStatus().then(setAgents).catch(console.error);
  }, []);

  return (
    <section>
      <h3 style={{ marginTop: 0 }}>Agents</h3>
      <p className="muted small" style={{ marginTop: 0 }}>本机检测到的 Agent CLI。未检测到的 Agent 不可启动。</p>
      {(Object.keys(AGENT_LABELS) as Agent[]).map((a) => {
        const st = agents[a];
        return (
          <div className="row-line" key={a}>
            <div>
              <div className="settings-row-label row" style={{ gap: 7 }}>
                <AgentIcon agent={a} />
                {AGENT_LABELS[a]}
              </div>
              <div className="settings-row-hint mono">{st?.executable ?? "未找到可执行文件"}</div>
            </div>
            <div className="row">
              {st?.version && <span className="muted mono small">{st.version}</span>}
              <span className={`badge ${st?.detected ? "success" : ""}`}>
                {st ? (st.detected ? "Detected" : "Not detected") : "…"}
              </span>
            </div>
          </div>
        );
      })}
    </section>
  );
}

const DELIVERY_LEVELS: { key: ContextDeliveryLevel; label: string; hint: string }[] = [
  {
    key: "off",
    label: "Off",
    hint: "Don't send NoEnding Workstream context to Agent sessions.",
  },
  {
    key: "compact",
    label: "Compact",
    hint: "Send only the most important current context and recent changes.",
  },
  {
    key: "balanced",
    label: "Balanced",
    hint: "Send core context plus relevant supporting information. Recommended.",
  },
  {
    key: "detailed",
    label: "Detailed",
    hint: "Send broader supporting context when more background may be useful.",
  },
];

/** Context & Sync：Context Delivery 梯度控制与只读自动化说明（§50）。 */
function ContextSyncSettings() {
  const [level, setLevel] = useState<ContextDeliveryLevel>("balanced");
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
    <>
      <section>
        <h3 style={{ marginTop: 0 }}>Context Delivery</h3>
        <p className="muted small" style={{ marginTop: 0 }}>
          Controls how much Workstream context NoEnding sends when starting or resuming Agent sessions.
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

      <section>
        <h3>Automation</h3>
        <div className="row-line">
          <div>
            <div className="settings-row-label">Automatic Sync</div>
            <div className="settings-row-hint">Session 有新内容时自动提取 Context 变更（带审计 Revision）。</div>
          </div>
          <span className="badge success">On</span>
        </div>
        <div className="row-line">
          <div>
            <div className="settings-row-label">Automatic Workstream Classification</div>
            <div className="settings-row-hint">自动归类只影响未显式绑定的 Session；你的手动指定优先。</div>
          </div>
          <span className="badge success">On</span>
        </div>
        <div className="row-line">
          <div>
            <div className="settings-row-label">Background Reconcile</div>
            <div className="settings-row-hint">应用启动时补摄离开期间产生的会话内容。</div>
          </div>
          <span className="badge success">On</span>
        </div>
      </section>
    </>
  );
}

/** Appearance：Theme（tokens 支持暗色，§61/§88）；Density 暂缓。 */
type Theme = "system" | "light" | "dark";
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
      <h3 style={{ marginTop: 0 }}>Theme</h3>
      <div className="settings-seg">
        {(["system", "light", "dark"] as Theme[]).map((t) => (
          <button key={t} className={theme === t ? "on" : ""} onClick={() => apply(t)}>
            {t[0].toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>
      <p className="muted small" style={{ marginBottom: 0 }}>
        跟随系统时自动切换 Light / Dark。
      </p>
    </section>
  );
}

/** Data & Advanced（§62）：第一版只展示数据库路径。 */
function AdvancedSettings() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    api.getAppInfo().then(setInfo).catch(console.error);
  }, []);

  return (
    <section>
      <h3 style={{ marginTop: 0 }}>Database</h3>
      <div className="row-line">
        <div>
          <div className="settings-row-label">Database Path</div>
          <div className="settings-row-hint mono" style={{ wordBreak: "break-all" }}>
            {info?.db_path ?? "…"}
          </div>
        </div>
      </div>
      <div className="row-line">
        <div>
          <div className="settings-row-label">Data Folder</div>
          <div className="settings-row-hint mono" style={{ wordBreak: "break-all" }}>
            {info?.app_data_dir ?? "…"}
          </div>
        </div>
      </div>
    </section>
  );
}
