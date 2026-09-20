import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { timeAgo } from "../../components/common";
import { RuntimeIntentBadges } from "../settings/AgentRuntimeSettings";
import type { AssistantScope, Route } from "../../app/routes";
import { AGENT_LABELS } from "../../types";
import type {
  Agent, AgentRuntimeSettings, Project, SyncRun, WorkstreamCardData,
} from "../../types";

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
  /** Which Agent answers — the Assistant's only runtime choice. Model /
   *  provider / effort come from 设置 → Agent like every consumer. */
  agent: string;
}

interface ActionProposal {
  action: string;
  agent?: string;
  workstream_ids: string[];
  cwd?: string | null;
  session_id?: string;
  extra_workstream_ids: string[];
}

const AGENT_CHOICES: string[] = [...(Object.keys(AGENT_LABELS) as Agent[]), "none"];

const CHOICE_LABELS: Record<string, string> = {
  ...AGENT_LABELS,
  none: "仅检索（不调用模型）",
};

/** override 摘要：`Runtime: Agent default` 表示 NoEnding 一个参数都不传。 */
function runtimeSummary(st: AgentRuntimeSettings | null): string {
  if (!st) return "…";
  const o = st.overrides;
  const passed = (["model", "provider", "effort"] as const)
    .filter((f) => o[f] !== null)
    .map((f) => `${f}=${o[f]}`);
  return passed.length ? `Runtime：${passed.join(" · ")}` : "Runtime：Agent 默认值";
}

const SUGGESTIONS = [
  "最近 NoEnding 项目主要解决了什么？",
  "Context Integrity 还有哪些 Open Questions？",
  "找一下讨论 Windows launcher 的 Session。",
  "把这个决定加入 Context。",
];

/** Selector 值编码：四种 scope 都能表示，不再是 workstream / workspace 二选一。 */
const scopeValue = (s: AssistantScope): string =>
  s.type === "workspace" ? "workspace" : `${s.type}:${s.id}`;

const scopeFromValue = (v: string): AssistantScope => {
  if (v === "workspace") return { type: "workspace" };
  const i = v.indexOf(":");
  const type = v.slice(0, i);
  const id = v.slice(i + 1);
  if (type === "project") return { type: "project", id };
  if (type === "session") return { type: "session", id };
  return { type: "workstream", id };
};

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
  const [runtime, setRuntime] = useState<AgentRuntimeSettings | null>(null);
  const [cfgOpen, setCfgOpen] = useState(false);
  const [runs, setRuns] = useState<SyncRun[]>([]);
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [currentScope, setCurrentScope] = useState<AssistantScope>(scope ?? { type: "workspace" });
  const logRef = useRef<HTMLDivElement>(null);

  // Scope 跟随 Route：从 Project Detail 进入带 scope，回 Sidebar 再进
  // Assistant 时恢复 Workspace —— 不能只在首次 mount 读取。
  useEffect(() => {
    setCurrentScope(scope ?? { type: "workspace" });
  }, [scope]);

  useEffect(() => {
    api.assistantConfigGet().then(setCfg).catch(console.error);
    api.listSyncRuns(12).then(setRuns).catch(console.error);
    api.listWorkstreamCards().then((cs) => setWorkstreams(cs.filter((c) => c.lifecycle === "active" && c.visibility === "normal"))).catch(console.error);
    api.listProjects().then(setProjects).catch(console.error);
  }, []);
  // Runtime 是只读视图：真正的编辑发生在「设置 → Agent」，这里只显示
  // Assistant 所选 Agent 当前的 override。
  const loadRuntime = useCallback((agent: string) => {
    if (agent === "none") {
      setRuntime(null);
      return;
    }
    api.getAgentRuntimeSettings(agent as Agent).then(setRuntime).catch((e) => {
      setRuntime(null);
      console.error(e);
    });
  }, []);
  useEffect(() => {
    if (cfg) loadRuntime(cfg.agent);
  }, [cfg, loadRuntime]);
  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [messages]);

  const scopeLabel = (s: AssistantScope): string => {
    switch (s.type) {
      case "workspace": return "Workspace";
      case "project": {
        const p = projects.find((x) => x.id === s.id);
        return p ? `Project · ${p.name}` : `Project ${s.id.slice(0, 8)}`;
      }
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

  const chooseAgent = async (agent: string) => {
    if (!cfg) return;
    const next = { agent };
    setCfg(next);
    await api.assistantConfigSet(next.agent);
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
            Assistant 经由你已登录的 Agent CLI 无头运行（<span className="mono">{cfg ? `${CHOICE_LABELS[cfg.agent] ?? cfg.agent} · ${cfg.agent === "none" ? "仅检索" : runtimeSummary(runtime)}` : "…"}</span>）。
          </p>
        </div>
        <div className="actions">
          <button className="btn ghost" onClick={() => setCfgOpen(true)}>Agent 设置</button>
        </div>
      </div>

      <div className="scope-row">
        <span className="muted small">范围</span>
        <select value={scopeValue(currentScope)}
          onChange={(e) => setCurrentScope(scopeFromValue(e.target.value))}>
          <option value="workspace">整个工作区</option>
          {projects.map((p) => (
            <option key={p.id} value={`project:${p.id}`}>Project · {p.name}</option>
          ))}
          {workstreams.map((w) => (
            <option key={w.id} value={`workstream:${w.id}`}>Workstream · {w.title}</option>
          ))}
          {currentScope.type === "workstream" &&
            !workstreams.some((w) => w.id === currentScope.id) && (
              <option value={`workstream:${currentScope.id}`}>
                Workstream {currentScope.id.slice(0, 8)}
              </option>
            )}
          {currentScope.type === "session" && (
            <option value={`session:${currentScope.id}`}>
              Session {currentScope.id.slice(0, 8)}
            </option>
          )}
        </select>
      </div>

      <div ref={logRef} className="chat-log" style={{ flex: 1, overflowY: "auto", minHeight: 260 }}>
        {messages.length === 0 && (
          <div className="assistant-empty">
            <div className="spark">✦</div>
            <h2>你现在在推进什么？</h2>
            <p className="hint">可以问 Workstreams、Sessions 与 Context。</p>
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
                <div className="muted small" style={{ marginTop: 6 }}>Runtime：{m.runtime}</div>
              )}
            </div>
          </div>
        ))}
        {busy && <div className="chat-msg assistant"><div className="bubble">思考中…（经由 Agent CLI，可能需要数十秒）</div></div>}
      </div>

      <div className="row" style={{ marginTop: 10 }}>
        <input type="text" placeholder="问 NoEnding…（问上下文、启动 Session、查摄入历史）" value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && send()} />
        <button className="btn primary" onClick={() => send()} disabled={busy}>发送</button>
      </div>

      <details className="details-feed">
        <summary>最近同步 <span className="muted">（后台，共 {runs.length} 条）</span></summary>
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
            <h2>Assistant 使用的 Agent</h2>
            <p className="muted small">
              Assistant 经你已登录的 Agent CLI 无头运行，无需单独 API Key。模型 / Provider / Effort 属于
              Runtime 配置，由「设置 → Agent」统一管理（与新建 Session、继续 Session、后台同步同一套 override）；
              这里只显示、不编辑。
            </p>
            {AGENT_CHOICES.map((k) => (
              <label key={k} className="small" style={{ display: "flex", gap: 8, alignItems: "center", padding: "4px 0" }}>
                <input type="radio" name="agent" style={{ width: "auto" }} checked={cfg.agent === k} onChange={() => chooseAgent(k)} />
                {CHOICE_LABELS[k]}
              </label>
            ))}
            <hr className="divider" />
            {cfg.agent === "none" ? (
              <p className="muted small">当前不调用模型，仅返回检索结果。</p>
            ) : runtime ? (
              <>
                <div className="row" style={{ gap: 6, flexWrap: "wrap" }}>
                  <span className="muted small">Runtime（{CHOICE_LABELS[cfg.agent]}）</span>
                  <RuntimeIntentBadges agent={cfg.agent as Agent} runtime={runtime.overrides} />
                </div>
                {!runtime.detected && (
                  <div className="badge warn" style={{ marginTop: 8 }}>
                    未检测到 {CHOICE_LABELS[cfg.agent]} CLI —— Assistant 会退回仅检索。
                  </div>
                )}
              </>
            ) : (
              <p className="muted small">Runtime 配置读取中…</p>
            )}
            <div className="row" style={{ justifyContent: "space-between", marginTop: 14 }}>
              <button className="btn" onClick={() => { setCfgOpen(false); navigate({ view: "settings", section: "agents" }); }}>
                前往设置 → Agent
              </button>
              <button className="btn primary" onClick={() => setCfgOpen(false)}>完成</button>
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
    ? `启动新建的 Session · ${CHOICE_LABELS[action.agent ?? ""] ?? action.agent ?? "?"}`
    : `继续 Session · ${action.session_id?.slice(0, 8) ?? "?"}`;
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
