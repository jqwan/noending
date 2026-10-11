import { useEffect, useState } from "react";
import { api } from "../../api";
import { onEvent, EVT_TERMINALS, type Route } from "../../app/routes";
import { showToast } from "../../components/Toast";
import type { Agent, AgentRuntimeOverrides, AgentStatusEntry, TerminalSummary } from "../../types";
import RuntimeFields, { CatalogStatus, NATIVE_RUNTIME } from "./RuntimeFields";
import { useAgentRuntime } from "./useAgentRuntime";

export default function AgentConfiguration({ agent, entry, reload, navigate }: {
  agent: Agent; entry?: AgentStatusEntry; reload: () => Promise<void>; navigate: (route: Route) => void;
}) {
  const runtime = useAgentRuntime(agent, Boolean(entry?.terminal_cli));
  const [draft, setDraft] = useState<AgentRuntimeOverrides>(NATIVE_RUNTIME);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);
  useEffect(() => {
    if (runtime.settings) setDraft(runtime.settings.overrides);
  }, [runtime.settings?.overrides.model, runtime.settings?.overrides.provider, runtime.settings?.overrides.effort]);
  useEffect(() => {
    let cancelled = false;
    const load = () => { api.terminalList().then((v) => { if (!cancelled) setTerminals(v.filter((t) => t.agent === agent)); }).catch((e) => { if (!cancelled) setError(String(e)); }); };
    load();
    const off = onEvent(EVT_TERMINALS, load);
    return () => { cancelled = true; off(); };
  }, [agent]);
  const action = async (fn: () => Promise<unknown>) => {
    setBusy(true); setError("");
    try { await fn(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const dirty = runtime.settings && (["model", "provider", "effort"] as const).some((field) => draft[field] !== runtime.settings!.overrides[field]);
  return <>
    {(error || runtime.error) && <div role="alert" className="badge warn">{error || runtime.error}</div>}
    <section className="card agent-settings-section">
      <h2>安装与启动</h2>
      {entry?.terminal_cli ? <>
        <div className="row between">
          <span>{entry.detected ? "CLI 已检测到" : "未检测到 CLI"}{entry.version ? ` · ${entry.version}` : ""}</span>
          <button className="btn small" disabled={busy} onClick={() => void action(async () => { runtime.setLoaded(await api.redetectAgent(agent)); runtime.invalidate(); await reload(); })}>重新检测</button>
        </div>
        {entry.executable && <p className="mono small agent-executable">{entry.executable}</p>}
        <label className="agent-toggle"><input type="checkbox" checked={entry.launch_enabled} disabled={busy} onChange={(e) => { const enabled = e.target.checked; void action(async () => { await api.setAgentLaunchEnabled(agent, enabled); await reload(); }); }} />用于新建会话</label>
        <p className="muted small">自动检测安装情况；关闭此选项后仍可继续已有会话及同步历史。</p>
      </> : <p className="muted">该代理不支持新建终端会话，可同步历史会话。</p>}
      {entry?.desktop_app && <p className="small">{entry.desktop_app} · {entry.desktop_app_present ? "已检测到桌面应用" : "未检测到桌面应用"}</p>}
    </section>
    {entry?.terminal_cli && <section className="card agent-settings-section">
      <div className="row between"><h2>默认运行参数</h2><button className="btn small" disabled={runtime.loading} onClick={() => void runtime.refresh(true)}>{runtime.loading ? "获取中…" : "刷新模型列表"}</button></div>
      <p className="muted small">留空由 Agent 自己决定。保存的参数用于新建、继续会话，以及使用该代理的助手与智能整理；新建会话可单次覆盖。</p>
      {!runtime.options && runtime.error && <button className="btn" disabled={busy} onClick={() => void action(async () => { runtime.setLoaded(await api.setAgentRuntimeOverrides(agent, NATIVE_RUNTIME)); })}>恢复 Agent 默认参数</button>}
      {runtime.options && <>
        <RuntimeFields value={draft} options={runtime.options} onChange={setDraft} onDiscover={() => void runtime.refresh()} disabled={busy} />
        <CatalogStatus options={runtime.options} />
        <div className="row" style={{ gap: 8 }}>
          <button className="btn primary" disabled={busy || !dirty} onClick={() => void action(async () => { runtime.setLoaded(await api.setAgentRuntimeOverrides(agent, draft)); showToast("已保存运行参数"); })}>保存参数</button>
          <button className="btn" disabled={busy} onClick={() => setDraft(NATIVE_RUNTIME)}>使用 Agent 默认</button>
        </div>
      </>}
    </section>}
    {terminals.length > 0 && <section className="card agent-settings-section"><h2>终端会话绑定</h2>
      {terminals.map((t) => <div className="agent-terminal-status" key={t.terminal_id}>
        <span>{t.session_id ? "已绑定" : "等待会话身份或消息落盘"} · {t.live ? "运行中" : "已退出"}</span>
        <button className="btn small ghost" onClick={() => navigate({ view: "terminal", terminalId: t.terminal_id })}>{t.session_title ?? "查看终端"}</button>
      </div>)}
    </section>}
  </>;
}
