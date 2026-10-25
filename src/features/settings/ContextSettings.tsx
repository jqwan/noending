import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type AgentRuntimeOverrides, type ExtractionSettings } from "../../types";
import { useAgentRuntime } from "../agents/useAgentRuntime";
import { NATIVE_RUNTIME } from "../agents/RuntimeFields";
import RuntimePopover from "../sessions/RuntimePopover";

const EXTRACTION_AGENTS = ["codex", "claude_code", "pi", "none"] as const;

export default function ContextSettings() {
  const [settings, setSettings] = useState<ExtractionSettings | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [agentMenuOpen, setAgentMenuOpen] = useState(false);
  const agentRef = useRef<HTMLDivElement>(null);
  const active = useRef(true);

  const currentAgent = settings?.agent === "none" ? "codex" : (settings?.agent ?? "codex");
  const runtime = useAgentRuntime(currentAgent, Boolean(settings && settings.agent !== "none"));

  useEffect(() => {
    active.current = true;
    setError("");
    let cancelled = false;
    api.getExtractionSettings().then(value => {
      if (!cancelled) {
        setSettings(value);
      }
    }).catch(e => {
      if (!cancelled) setError(String(e));
    });
    return () => {
      cancelled = true;
      active.current = false;
    };
  }, [attempt]);

  useEffect(() => {
    if (!agentMenuOpen) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (agentRef.current && !agentRef.current.contains(e.target as Node)) {
        setAgentMenuOpen(false);
      }
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setAgentMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [agentMenuOpen]);

  const applySettings = async (next: ExtractionSettings) => {
    setSettings(next);
    setError("");
    try {
      const saved = await api.setExtractionSettings(next);
      if (active.current) {
        setSettings(saved);
      }
    } catch (e) {
      if (active.current) {
        setError(String(e));
      }
    }
  };

  const handleAgentChange = async (nextAgent: ExtractionSettings["agent"]) => {
    if (!settings || nextAgent === settings.agent) return;
    const nextSettings: ExtractionSettings = {
      agent: nextAgent,
      runtime: nextAgent === "none" ? NATIVE_RUNTIME : NATIVE_RUNTIME,
    };
    await applySettings(nextSettings);
  };

  const handleRuntimeChange = async (nextRuntime: AgentRuntimeOverrides) => {
    if (!settings || settings.agent === "none") return;
    const nextSettings: ExtractionSettings = {
      agent: settings.agent,
      runtime: nextRuntime,
    };
    await applySettings(nextSettings);
  };

  const openDiagnosticsLogs = async () => {
    try {
      await api.openContextExtractionLogs();
      showToast("已打开 Context 更新日志目录");
    } catch (err) {
      console.error(err);
      showToast("无法打开 Context 更新日志目录");
    }
  };

  const agentLabel = !settings
    ? ""
    : settings.agent === "none"
    ? "停用后台提取"
    : (AGENT_LABELS[settings.agent] ?? settings.agent);

  return (
    <section>
      <div className="settings-section-header">
        <div>
          <h3 style={{ display: "flex", alignItems: "center", gap: 8, margin: 0 }}>
            <Icon name="spark" />
            <span>摘要</span>
          </h3>
          <p className="muted small" style={{ margin: "4px 0 0" }}>
            会话、任务与项目摘要的后台提取运行配置与更新诊断。设置自动生效。
          </p>
        </div>
      </div>

      {!settings ? (
        error ? (
          <div role="alert">
            {error}
            <button className="btn small" onClick={() => setAttempt(value => value + 1)}>重试</button>
          </div>
        ) : (
          <p className="muted small" role="status">正在读取提取配置…</p>
        )
      ) : (
        <div className="extraction-settings-card">
          <div className="extraction-controls-row">
            <div className="extraction-control-group">
              <span className="extraction-control-label">提取 Agent</span>
              <div
                className={`new-session-picker new-session-agent has-custom-menu${agentMenuOpen ? " open" : ""}`}
                ref={agentRef}
                onClick={(e) => {
                  if ((e.target as HTMLElement).closest(".new-session-dropdown-menu")) return;
                  setAgentMenuOpen((v) => !v);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                    if (!agentMenuOpen) {
                      e.preventDefault();
                      setAgentMenuOpen(true);
                    }
                  }
                }}
                tabIndex={0}
                role="button"
                aria-haspopup="menu"
                aria-expanded={agentMenuOpen}
                aria-label={`提取 Agent：${agentLabel}`}
                title={agentLabel}
              >
                {settings.agent !== "none" ? (
                  <AgentIcon agent={settings.agent} size={16} />
                ) : (
                  <Icon name="minus" />
                )}
                <span className="new-session-picker-label">{agentLabel}</span>
                <Icon name="chevronDown" className="new-session-chevron" />
                <select
                  aria-label="提取 Agent"
                  title={agentLabel}
                  value={settings.agent}
                  onChange={(e) => void handleAgentChange(e.target.value as ExtractionSettings["agent"])}
                  tabIndex={-1}
                >
                  {EXTRACTION_AGENTS.map((a) => (
                    <option key={a} value={a}>
                      {a === "none" ? "停用后台提取" : AGENT_LABELS[a]}
                    </option>
                  ))}
                </select>

                {agentMenuOpen && (
                  <div className="new-session-dropdown-menu dropdown-down" role="menu" aria-label="提取 Agent 列表">
                    <div className="new-session-menu-list">
                      {EXTRACTION_AGENTS.map((a) => {
                        const isSelected = a === settings.agent;
                        const label = a === "none" ? "停用后台提取" : AGENT_LABELS[a];
                        return (
                          <button
                            key={a}
                            type="button"
                            className={`new-session-menu-item${isSelected ? " selected" : ""}`}
                            onClick={() => {
                              void handleAgentChange(a);
                              setAgentMenuOpen(false);
                            }}
                            title={label}
                          >
                            <span className="menu-item-icon">
                              {a !== "none" ? <AgentIcon agent={a} size={16} /> : <Icon name="minus" />}
                            </span>
                            <span className="menu-item-label">{label}</span>
                            {isSelected && (
                              <span className="menu-item-check"><Icon name="check" /></span>
                            )}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}
              </div>
            </div>

            {settings.agent !== "none" && (
              <div className="extraction-control-group">
                <span className="extraction-control-label">模型与推理</span>
                <RuntimePopover
                  agent={settings.agent}
                  runtime={runtime}
                  value={settings.runtime}
                  direction="down"
                  onChange={handleRuntimeChange}
                />
              </div>
            )}
          </div>

          {settings.agent === "none" && (
            <div className="muted small">
              已停用后台提取，当前会话仍可通过提取技能更新摘要。
            </div>
          )}

          {(error || (settings.agent !== "none" && runtime.error)) && (
            <p role="alert" className="danger" style={{ margin: 0 }}>
              {error || runtime.error}
            </p>
          )}
        </div>
      )}

      <div className="extraction-subtools">
        <div className="extraction-subtool-card">
          <div className="extraction-subtool-info">
            <span className="extraction-subtool-title">
              <Icon name="terminal" />
              <span>Context 更新诊断</span>
            </span>
            <span className="muted small">记录更新时间、目标与失败阶段，保留 14 天</span>
          </div>
          <button
            type="button"
            className="btn small ghost"
            onClick={openDiagnosticsLogs}
          >
            打开日志目录
          </button>
        </div>
      </div>
    </section>
  );
}
