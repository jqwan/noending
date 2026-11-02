import { DetailLayout } from "../../components/DetailLayout";
import { Field } from "../../components/common";
import { DetailFields, DetailList, DetailListRow, DetailSection } from "../../components/DetailSection";
import Notice from "../../components/Notice";
import { useEffect, useState } from "react";
import { api } from "../../api";
import { onEvent, EVT_SYNCED, EVT_TERMINALS, type Route } from "../../app/routes";
import Icon from "../../components/Icon";
import type { Agent, AgentStatusEntry, SessionListItem } from "../../types";
import { AGENT_LABELS } from "../../types";
import SessionMiniList from "../sessions/SessionMiniList";
import { CatalogStatus } from "./RuntimeFields";
import { useAgentRuntime } from "./useAgentRuntime";
import { extractVersion } from "./AgentsView";

export default function AgentConfiguration({
  agent,
  entry,
  reload,
  navigate,
  packagesSection,
  sourcesSection,
}: {
  agent: Agent;
  entry?: AgentStatusEntry;
  reload: () => Promise<void>;
  navigate: (route: Route) => void;
  packagesSection?: React.ReactNode;
  sourcesSection?: React.ReactNode;
}) {
  const runtime = useAgentRuntime(agent, Boolean(entry?.terminal_cli));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
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
        <Notice role="alert"  >
          {error || runtime.error}
        </Notice>
      )}

      <DetailLayout panel={<><DetailSection sectionId="installation" label="安装与启动" title={<>
              <Icon name="settings" />
              <span>安装与启动</span>
            </>}>

            <DetailFields>
              {entry?.terminal_cli ? <>
                <Field label="CLI" action={<button
                  className="btn ghost icon-button small" disabled={busy}
                  title="重新检测 CLI" aria-label="重新检测"
                  onClick={() => void action(async () => {
                    runtime.setLoaded(await api.redetectAgent(agent));
                    runtime.invalidate();
                    await reload();
                  })}
                ><Icon name="refresh" className={busy ? "spinning" : ""} /></button>}>
                  {entry.detected ? "CLI 已检测到" : "未检测到 CLI"}
                  {entry.version ? ` · ${extractVersion(entry.version) ? `v${extractVersion(entry.version)}` : entry.version}` : ""}
                </Field>
                {entry.executable && <Field label="可执行文件路径" copyValue={entry.executable}>
                  <span className="mono" title={entry.executable}>{entry.executable}</span>
                </Field>}
              </> : <Field label="CLI"><span className="muted">该 Agent 不支持新建终端会话，可同步历史会话。</span></Field>}
              {entry?.desktop_app && <Field label="桌面应用">
                {entry.desktop_app} · {entry.desktop_app_present ? "已检测到桌面应用" : "未检测到桌面应用"}
              </Field>}
            </DetailFields>
          </DetailSection></>}>{sourcesSection}
{packagesSection}
{entry?.terminal_cli && (
            <DetailSection sectionId="models" label="模型与思考等级" title={<>
                  <Icon name="sliders" />
                  <span>模型与思考等级</span>
                </>} actions={<>
                  <button
                    className="btn ghost icon-button small"
                    disabled={runtime.loading}
                    onClick={() => void runtime.refresh(true)}
                    title="刷新模型列表"
                    aria-label="刷新模型列表"
                  >
                    <Icon name="refresh" className={runtime.loading ? "spinning" : ""} />
                  </button>
                </>}>

              <p className="muted small" style={{ marginTop: 0 }}>
                展示当前 Agent 支持的模型与思考强度。新建会话时可在发送前直接选择。
              </p>
              {runtime.options && <CatalogStatus options={runtime.options} />}
              {runtime.options && (
                <div className="agent-facts-group">
                  {runtime.options.models.length > 0 ? (
                    (() => {
                      const hasPerModelEfforts = runtime.options.models.some((x) => x.supported_efforts.length > 0);
                      return (
                        <DetailList label="模型列表">
                          {runtime.options.models.map((m) => {
                            const efforts = hasPerModelEfforts
                              ? m.supported_efforts
                              : (runtime.options?.effort_levels ?? []);
                            return (
                              <DetailListRow key={`${m.provider || ""}/${m.id}`}
                                title={m.display_name ?? m.id}
                                meta={m.provider && <span className="badge">{m.provider}</span>}
                                description={<>思考档位：{efforts.length > 0 ? efforts.join(" / ") : (hasPerModelEfforts ? "不支持" : "由 Agent 自行决定")}</>}
                              />
                            );
                          })}
                        </DetailList>
                      );
                    })()
                  ) : (
                    <p className="muted small" style={{ margin: "4px 0 0" }}>
                      暂未发现模型，点击右上角刷新按钮从 CLI 获取。
                    </p>
                  )}
                </div>
              )}
            </DetailSection>
          )}
<DetailSection sectionId="sessions" label="会话" title={<>
                <Icon name="chat" />
                <span>会话（{sessions.length}）</span>
              </>} actions={<>{Boolean(entry?.terminal_cli && entry?.detected) && (
                <button
                  className="btn ghost icon-button small"
                  title={`使用 ${AGENT_LABELS[agent]} 新建会话`}
                  aria-label="新建会话"
                  onClick={() => navigate({ view: "new-session", agent })}
                >
                  <Icon name="plus" />
                </button>
              )}</>}>


            <SessionMiniList
              sessions={sessions}
              emptyText="该 Agent 下还没有会话。"
              navigate={navigate}
            />
          </DetailSection></DetailLayout>
    </div>
  );
}
