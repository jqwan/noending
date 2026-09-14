import React, { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { timeAgo } from "../../components/common";
import type { AssistantScope, Route } from "../../app/routes";
import type { SyncRun, WorkstreamCardData } from "../../types";

interface AssistantMessage {
  id: string;
  session_id: string;
  role: "user" | "assistant";
  content: string;
  action_json: string | null;
  runtime: string | null;
  created_at: string;
}

interface AssistantConfig {
  agent: string;
  model: string;
  provider: string;
  effort: string;
}

interface ActionProposal {
  action: string;
  agent?: string;
  workstream_ids: string[];
  cwd?: string | null;
  session_id?: string;
  extra_workstream_ids: string[];
}

const AGENT_PRESETS: Record<string, { model: string; provider: string; effort: string; label: string }> = {
  codex: { model: "gpt-5.6-luna", provider: "openai-codex", effort: "low", label: "Codex · gpt-5.6-luna (low)" },
  pi: { model: "qwen/qwen3.8-27b", provider: "lmstudio", effort: "low", label: "Pi · 本地 Qwen (LM Studio)" },
  claude_code: { model: "sonnet", provider: "", effort: "", label: "Claude Code · sonnet" },
  none: { model: "", provider: "", effort: "", label: "仅检索（不调用模型）" },
};

const SUGGESTIONS = [
  "最近 NoEnding 项目主要解决了什么？",
  "Context Integrity 还有哪些 Open Questions？",
  "找一下讨论 Windows launcher 的 Session。",
  "把这个决定加入 Context。",
];

/**
 * Assistant = Workspace Interface（整体设计方案 §46-§50），不是第四个 Agent。
 * Scope 只做 prompt 侧注入（§38），不引入新协议。
 */
export default function AssistantView({ scope, navigate }: {
  scope?: AssistantScope;
  navigate: (r: Route) => void;
}) {
  const [messages, setMessages] = useState<AssistantMessage[]>([]);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [cfg, setCfg] = useState<AssistantConfig | null>(null);
  const [cfgOpen, setCfgOpen] = useState(false);
  const [runs, setRuns] = useState<SyncRun[]>([]);
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [currentScope, setCurrentScope] = useState<AssistantScope>(scope ?? { type: "workspace" });
  const logRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(() => {
    api.listSyncRuns(12).then(setRuns).catch(console.error);
    if (sessionId) {
      api.assistantMessages(sessionId).then(setMessages).catch(console.error);
    }
  }, [sessionId]);
  useEffect(() => {
    api.assistantConfigGet().then(setCfg).catch(console.error);
    api.listSyncRuns(12).then(setRuns).catch(console.error);
    api.listWorkstreamCards().then((cs) => setWorkstreams(cs.filter((c) => c.lifecycle === "open" && c.visibility === "normal"))).catch(console.error);
  }, []);
  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [messages]);

  const scopeLabel = (s: AssistantScope): string => {
    switch (s.type) {
      case "workspace": return "Workspace";
      case "project": return `Project ${s.id.slice(0, 8)}`;
      case "workstream": {
        const t = workstreams.find((w) => w.id === s.id)?.title;
        return t ? `Workstream · ${t}` : `Workstream ${s.id.slice(0, 8)}`;
      }
      case "session": return `Session ${s.id.slice(0, 8)}`;
    }
  };

  const send = async (raw?: string) => {
    const base = (raw ?? input).trim();
    if (!base || busy) return;
    setInput("");
    setBusy(true);
    // Scope 以元数据前缀注入 prompt（§38），不改变协议
    const scoped = currentScope.type === "workspace"
      ? base
      : `[Scope: ${scopeLabel(currentScope)}]\n${base}`;
    setMessages((m) => [...m, {
      id: `tmp-${Date.now()}`, session_id: sessionId ?? "", role: "user",
      content: base, action_json: null, runtime: null,
      created_at: new Date().toISOString(),
    }]);
    try {
      const reply = await api.assistantSend(sessionId, scoped);
      setSessionId(reply.session_id);
      const msgs = await api.assistantMessages(reply.session_id);
      setMessages(msgs);
      api.listSyncRuns(12).then(setRuns).catch(() => {});
    } catch (e) {
      setMessages((m) => [...m, {
        id: `err-${Date.now()}`, session_id: sessionId ?? "", role: "assistant",
        content: `调用失败：${e}`, action_json: null, runtime: null,
        created_at: new Date().toISOString(),
      }]);
    } finally {
      setBusy(false);
    }
  };

  const applyPreset = async (agent: string) => {
    if (!cfg) return;
    const p = AGENT_PRESETS[agent];
    const next = { agent, model: p.model, provider: p.provider, effort: p.effort };
    setCfg(next);
    await api.assistantConfigSet(next.agent, next.model, next.provider, next.effort);
  };

  const saveCfg = async () => {
    if (!cfg) return;
    await api.assistantConfigSet(cfg.agent, cfg.model, cfg.provider, cfg.effort);
    setCfgOpen(false);
  };

  const executeAction = async (actionJson: string, msgId: string) => {
    try {
      const r = await api.assistantExecuteAction(actionJson);
      setMessages((m) => m.map((msg) => msg.id === msgId
        ? { ...msg, content: `${msg.content}\n\n✅ 已执行：${r.note ?? r.launched_via ?? ""}`, action_json: null }
        : msg));
    } catch (e) {
      alert(`执行失败：${e}`);
    }
  };

  return (
    <div className="main" style={{ display: "flex", flexDirection: "column" }}>
      <div className="page-head">
        <div>
          <h1>Assistant</h1>
          <p className="page-sub">
            Workspace Assistant，经由你已登录的 Agent CLI 无头运行（<span className="mono">{cfg ? `${cfg.agent}${cfg.model ? " · " + cfg.model : ""}` : "…"}</span>）。
          </p>
        </div>
        <div className="actions">
          <button className="btn ghost" onClick={() => setCfgOpen(true)}>模型设置</button>
        </div>
      </div>

      <div className="scope-row">
        <span className="muted small">Scope:</span>
        <select value={currentScope.type === "workstream" ? currentScope.id : "workspace"}
          onChange={(e) => {
            const v = e.target.value;
            setCurrentScope(v === "workspace" ? { type: "workspace" } : { type: "workstream", id: v });
          }}>
          <option value="workspace">Workspace</option>
          {workstreams.map((w) => (
            <option key={w.id} value={w.id}>Workstream · {w.title}</option>
          ))}
        </select>
      </div>

      <div ref={logRef} className="chat-log" style={{ flex: 1, overflowY: "auto", minHeight: 260 }}>
        {messages.length === 0 && (
          <div className="assistant-empty">
            <div className="spark">✦</div>
            <h2>What are you working on?</h2>
            <p className="hint">Ask about your Workstreams, Sessions and Context.</p>
            <div className="suggest-list">
              {SUGGESTIONS.map((s) => (
                <button key={s} className="suggest-chip" onClick={() => send(s)}>{s}</button>
              ))}
            </div>
          </div>
        )}
        {messages.map((m) => (
          <div key={m.id} className={`chat-msg ${m.role === "user" ? "user" : "assistant"}`}>
            <div className="bubble">
              {m.content}
              {m.action_json && (
                <div style={{ marginTop: 10 }}>
                  <ActionCard json={m.action_json} onExecute={() => executeAction(m.action_json!, m.id)} navigate={navigate} />
                </div>
              )}
              {m.runtime && m.role === "assistant" && (
                <div className="muted small" style={{ marginTop: 6 }}>runtime: {m.runtime}</div>
              )}
            </div>
          </div>
        ))}
        {busy && <div className="chat-msg assistant"><div className="bubble">思考中…（经由 Agent CLI，可能需要数十秒）</div></div>}
      </div>

      <div className="row" style={{ marginTop: 10 }}>
        <input type="text" placeholder="Ask NoEnding...（问上下文、启动 Session、查同步历史）" value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && send()} />
        <button className="btn primary" onClick={() => send()} disabled={busy}>Send</button>
      </div>

      <details className="details-feed">
        <summary>最近同步 <span className="muted">（Background Mode，共 {runs.length} 条）</span></summary>
        <div>
          {runs.map((r) => (
            <div key={r.id} className="feed-row">
              <span className="when">{timeAgo(r.created_at)}</span>
              <span style={{ flex: 1 }}>{r.summary}</span>
              <span className="badge">{r.runtime}</span>
            </div>
          ))}
          {runs.length === 0 && <div className="muted small" style={{ padding: "8px 0" }}>暂无同步记录。</div>}
        </div>
      </details>

      {cfgOpen && cfg && (
        <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setCfgOpen(false)}>
          <div className="modal">
            <h2>Assistant 运行模型</h2>
            <p className="muted small">Assistant 与后台同步共用此配置；调用走 Agent CLI 无头模式，使用你已登录的账号，无需单独 API Key。</p>
            {Object.entries(AGENT_PRESETS).map(([k, p]) => (
              <label key={k} className="small" style={{ display: "flex", gap: 8, alignItems: "center", padding: "4px 0" }}>
                <input type="radio" name="agent" style={{ width: "auto" }} checked={cfg.agent === k} onChange={() => applyPreset(k)} />
                {p.label}
              </label>
            ))}
            <hr className="divider" />
            <div className="grid2">
              <label className="field"><span>模型</span>
                <input type="text" value={cfg.model} onChange={(e) => setCfg({ ...cfg, model: e.target.value })} /></label>
              <label className="field"><span>Effort / Thinking</span>
                <input type="text" value={cfg.effort} onChange={(e) => setCfg({ ...cfg, effort: e.target.value })} /></label>
            </div>
            {cfg.agent === "pi" && (
              <label className="field"><span>Pi Provider</span>
                <input type="text" value={cfg.provider} onChange={(e) => setCfg({ ...cfg, provider: e.target.value })} /></label>
            )}
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button className="btn" onClick={() => setCfgOpen(false)}>取消</button>
              <button className="btn primary" onClick={saveCfg}>保存</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** Action Card（§50）：所有改变 Domain 的操作先摘要、确认后才执行。 */
function ActionCard({ json, onExecute, navigate }: {
  json: string;
  onExecute: () => void;
  navigate: (r: Route) => void;
}) {
  let action: ActionProposal | null = null;
  try { action = JSON.parse(json); } catch { /* ignore */ }
  if (!action) return null;
  const label = action.action === "launch_new_session"
    ? `启动 New Session · ${action.agent ?? "?"}`
    : `Resume Session · ${action.session_id?.slice(0, 8) ?? "?"}`;
  return (
    <div className="card" style={{ margin: 0 }}>
      <div className="row between">
        <div className="row">
          <span className="badge dark">建议动作</span>
          <strong className="small">{label}</strong>
          {action.workstream_ids.length > 0 && (
            <span className="muted small">{action.workstream_ids.length} 个 Workstream</span>
          )}
        </div>
        <div className="row">
          {action.action === "resume_session" && action.session_id && (
            <button className="link" onClick={() => navigate({ view: "session", sessionId: action.session_id! })}>查看</button>
          )}
          <button className="btn small accent" onClick={onExecute}>确认执行</button>
        </div>
      </div>
    </div>
  );
}
