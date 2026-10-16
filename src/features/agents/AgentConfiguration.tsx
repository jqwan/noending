import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import { onEvent, EVT_SYNCED, EVT_TERMINALS, type Route } from "../../app/routes";
import Icon from "../../components/Icon";
import type { Agent, AgentStatusEntry, SessionListItem, TerminalSummary } from "../../types";
import { AGENT_LABELS } from "../../types";
import SessionMiniList from "../sessions/SessionMiniList";
import { CatalogStatus, NATIVE_RUNTIME } from "./RuntimeFields";
import { useAgentRuntime } from "./useAgentRuntime";
import { extractVersion } from "./AgentsView";

export default function AgentConfiguration({
  agent,
  entry,
  reload,
  navigate,
  sourcesSection,
}: {
  agent: Agent;
  entry?: AgentStatusEntry;
  reload: () => Promise<void>;
  navigate: (route: Route) => void;
  sourcesSection?: React.ReactNode;
}) {
  const runtime = useAgentRuntime(agent, Boolean(entry?.terminal_cli));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);
  const [sessions, setSessions] = useState<SessionListItem[]>([]);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      api.listSessions(undefined, agent).then((v) => {
        if (!cancelled) setSessions(v);
      }).catch(() => {});
    };
    load();
    const offSync = onEvent(EVT_SYNCED, load);
    const offTerm = onEvent(EVT_TERMINALS, load);
    return () => {
      cancelled = true;
      offSync();
      offTerm();
    };
  }, [agent]);

  const sortedSessions = useMemo(() => {
    return [...sessions].sort((a, b) =>
      (b.last_activity_at ?? b.started_at ?? "").localeCompare(
        a.last_activity_at ?? a.started_at ?? "",
      ),
    );
  }, [sessions]);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      api.terminalList().then((v) => {
        if (!cancelled) setTerminals(v.filter((t) => t.agent === agent));
      }).catch((e) => {
        if (!cancelled) setError(String(e));
      });
    };
    load();
    const off = onEvent(EVT_TERMINALS, load);
    return () => {
      cancelled = true;
      off();
    };
  }, [agent]);

  const action = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="agent-detail-wrapper">
      {(error || runtime.error) && (
        <div role="alert" className="badge warn" style={{ marginBottom: 14 }}>
          {error || runtime.error}
        </div>
      )}

      <div className="agent-detail-layout">
        <div className="agent-detail-main">
          {sourcesSection}

          {terminals.length > 0 && (
            <section className="card agent-settings-section">
              <h2>
                <Icon name="terminal" />
                <span>终端会话</span>
              </h2>
              <div className="agent-terminal-list">
                {terminals.map((t) => (
                  <div className="agent-terminal-status" key={t.terminal_id}>
                    <div className="agent-terminal-info">
                      <span className={`dot ${t.live ? "running" : "exited"}`} />
                      <span>
                        {t.session_id ? "已绑定" : "等待会话身份或消息落盘"} · {t.live ? "运行中" : "已退出"}
                      </span>
                    </div>
                    <button
                      className="btn small ghost"
                      onClick={() => navigate({ view: "terminal", terminalId: t.terminal_id })}
                    >
                      {t.session_title ?? "查看终端"}
                    </button>
                  </div>
                ))}
              </div>
            </section>
          )}

          <section className="card agent-settings-section">
            <div className="row between" style={{ alignItems: "center" }}>
              <h2>
                <Icon name="chat" />
                <span>会话 ({sessions.length})</span>
              </h2>
              {Boolean(entry?.terminal_cli && entry?.detected) && (
                <button
                  className="btn ghost icon-button"
                  title={`使用 ${AGENT_LABELS[agent]} 新建会话`}
                  aria-label="新建会话"
                  onClick={() => navigate({ view: "new-session", agent })}
                >
                  <Icon name="plus" />
                </button>
              )}
            </div>

            <SessionMiniList
              sessions={sortedSessions}
              emptyText="该 Agent 下还没有会话。"
              navigate={navigate}
            />
          </section>
        </div>

        <aside className="agent-detail-aside">
          <section className="card agent-settings-section">
            <h2>
              <Icon name="settings" />
              <span>安装与启动</span>
            </h2>
            {entry?.terminal_cli ? (
              <div className="agent-aside-facts">
                <div className="row between" style={{ alignItems: "center" }}>
                  <span className="small">
                    {entry.detected ? "CLI 已检测到" : "未检测到 CLI"}
                    {entry.version ? ` · ${extractVersion(entry.version) ? `v${extractVersion(entry.version)}` : entry.version}` : ""}
                  </span>
                  <button
                    className="btn ghost icon-button small"
                    disabled={busy}
                    title="重新检测 CLI"
                    aria-label="重新检测"
                    onClick={() => void action(async () => {
                      runtime.setLoaded(await api.redetectAgent(agent));
                      runtime.invalidate();
                      await reload();
                    })}
                  >
                    <Icon name="refresh" className={busy ? "spinning" : ""} />
                  </button>
                </div>
                {entry.executable && (
                  <div className="agent-executable-block">
                    <span className="section-label" style={{ margin: "8px 0 2px", fontSize: 11 }}>可执行文件路径</span>
                    <p className="mono small agent-executable" title={entry.executable}>{entry.executable}</p>
                  </div>
                )}
              </div>
            ) : (
              <p className="muted">该 Agent 不支持新建终端会话，可同步历史会话。</p>
            )}

            {entry?.desktop_app && (
              <div className="agent-desktop-status" style={{ marginTop: 12, paddingTop: 10, borderTop: "1px solid var(--border-subtle)" }}>
                <span className="section-label" style={{ margin: "0 0 4px", fontSize: 11 }}>桌面应用</span>
                <p className="small" style={{ margin: 0 }}>
                  {entry.desktop_app} · {entry.desktop_app_present ? "已检测到桌面应用" : "未检测到桌面应用"}
                </p>
              </div>
            )}
          </section>

          {entry?.terminal_cli && (
            <section className="card agent-settings-section">
              <div className="row between" style={{ alignItems: "center", marginBottom: 6 }}>
                <h2>
                  <Icon name="sliders" />
                  <span>模型与思考等级</span>
                </h2>
                <div className="row" style={{ gap: 8, alignItems: "center" }}>
                  {runtime.options && <CatalogStatus options={runtime.options} />}
                  <button
                    className="btn ghost icon-button small"
                    disabled={runtime.loading}
                    onClick={() => void runtime.refresh(true)}
                    title="刷新模型列表"
                    aria-label="刷新模型列表"
                  >
                    <Icon name="refresh" className={runtime.loading ? "spinning" : ""} />
                  </button>
                </div>
              </div>
              <p className="muted small" style={{ marginTop: 0 }}>
                展示当前 Agent 支持的模型与思考强度。新建会话时可在发送前直接选择。
              </p>
              {!runtime.options && runtime.error && (
                <button
                  className="btn small"
                  disabled={busy}
                  onClick={() => void action(async () => {
                    runtime.setLoaded(await api.setAgentRuntimeOverrides(agent, NATIVE_RUNTIME));
                  })}
                >
                  恢复 Agent 默认参数
                </button>
              )}
              {runtime.options && (
                <div className="agent-facts-group">
                  {runtime.options.models.length > 0 ? (
                    (() => {
                      const hasPerModelEfforts = runtime.options.models.some((x) => x.supported_efforts.length > 0);
                      return (
                        <div className="agent-models-list">
                          {runtime.options.models.map((m) => {
                            const efforts = hasPerModelEfforts
                              ? m.supported_efforts
                              : (runtime.options?.effort_levels ?? []);
                            return (
                              <div key={`${m.provider || ""}/${m.id}`} className="agent-model-row">
                                <div className="agent-model-info">
                                  <span className="agent-model-id">{m.display_name ?? m.id}</span>
                                  {m.provider && (
                                    <span className="badge small">{m.provider}</span>
                                  )}
                                </div>
                                <span className="muted small">
                                  思考档位：{efforts.length > 0 ? efforts.join(" / ") : (hasPerModelEfforts ? "不支持" : "由 Agent 自行决定")}
                                </span>
                              </div>
                            );
                          })}
                        </div>
                      );
                    })()
                  ) : (
                    <p className="muted small" style={{ margin: "4px 0 0" }}>
                      暂未发现模型，点击右上角刷新按钮从 CLI 获取。
                    </p>
                  )}
                </div>
              )}
            </section>
          )}
        </aside>
      </div>
    </div>
  );
}
