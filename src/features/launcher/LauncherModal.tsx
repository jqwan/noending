import React, { useEffect, useState } from "react";
import { api } from "../../api";
import type { Agent, SessionContextBundle } from "../../types";
import { AGENT_LABELS } from "../../types";
import { Modal } from "../../components/common";
import type { LaunchResult } from "../../types";

export default function LauncherModal({
  workstreamIds = [],
  sessionId,
  mode,
  onClose,
}: {
  workstreamIds?: string[];
  sessionId?: string;
  mode: "new" | "resume";
  onClose: () => void;
}) {
  const [agent, setAgent] = useState<Agent>("claude_code");
  const [selected, setSelected] = useState<Set<string>>(new Set(workstreamIds));
  const [workstreams, setWorkstreams] = useState<{ id: string; title: string }[]>([]);
  const [cwd, setCwd] = useState("");
  const [preview, setPreview] = useState<SessionContextBundle | null>(null);
  const [result, setResult] = useState<LaunchResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api.listWorkstreams().then((ws) => setWorkstreams(ws.filter((w) => w.visibility === "normal").map((w) => ({ id: w.id, title: w.title })))).catch(console.error);
  }, []);

  const ids = Array.from(selected);
  useEffect(() => {
    if (ids.length === 0) { setPreview(null); return; }
    // Resume 预览必须携带 session_id：增量是针对该 Session 上次实际
    // 收到的上下文计算的。
    api.previewBundle(ids, mode, sessionId).then(setPreview).catch((e) => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [JSON.stringify(ids), mode, sessionId]);

  const launch = async () => {
    setBusy(true); setError("");
    try {
      const r = mode === "new"
        ? await api.launchNewSession(agent, ids, cwd || undefined)
        : await api.launchResumeSession(sessionId!, ids);
      setResult(r);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={mode === "new" ? "启动 New Session" : "Resume Session"} onClose={onClose}>
      {result ? (
        <>
          <div className="badge accent" style={{ marginBottom: 12 }}>已通过 {result.launched_via} 启动</div>
          <p className="small">{result.note}</p>
          <h3>执行的命令</h3>
          <div className="card mono small">{result.command_line}</div>
          <h3>注入的上下文（约 {result.bundle.approx_tokens} tokens）</h3>
          <div className="card mono small" style={{ whiteSpace: "pre-wrap", maxHeight: 240, overflow: "auto" }}>{result.bundle.markdown}</div>
          <div className="row" style={{ justifyContent: "flex-end", marginTop: 12 }}>
            <button className="btn primary" onClick={onClose}>完成</button>
          </div>
        </>
      ) : (
        <>
          {mode === "new" && (
            <>
              <h3>Agent</h3>
              <div className="row" style={{ marginBottom: 14 }}>
                {Object.entries(AGENT_LABELS).map(([k, v]) => (
                  <button key={k}
                    className={`btn ${agent === k ? "primary" : ""}`}
                    onClick={() => setAgent(k as Agent)}>{v}</button>
                ))}
              </div>
              <label className="field"><span>工作目录（可选，Codex 建议填写）</span>
                <input type="text" value={cwd} onChange={(e) => setCwd(e.target.value)} placeholder="~/projects/…" /></label>
            </>
          )}

          <h3>{mode === "new" ? "Contexts（Workstream 多选，可不选）" : "附加 Workstreams（临时加入，可选）"}</h3>
          {workstreams.map((w) => (
            <label key={w.id} className="small" style={{ display: "flex", gap: 8, alignItems: "center", padding: "3px 0" }}>
              <input type="checkbox" style={{ width: "auto" }}
                checked={selected.has(w.id)}
                onChange={(e) => setSelected((s) => { const n = new Set(s); e.target.checked ? n.add(w.id) : n.delete(w.id); return n; })} />
              {w.title}
            </label>
          ))}
          {selected.size === 0 && (
            <div className="muted small" style={{ margin: "4px 0 0" }}>
              {mode === "new"
                ? "不携带 Workstream Context 直接开始；Session 之后会通过 Sync 自动关联到已有 Workstream。"
                : "该 Session 未关联 Workstream：将同步其自身消息后直接恢复，不注入上下文。"}
            </div>
          )}

          <h3>Context Bundle 预览 {preview && <span className="muted">（约 {preview.approx_tokens} tokens）</span>}</h3>
          <div className="card mono small" style={{ whiteSpace: "pre-wrap", maxHeight: 200, overflow: "auto" }}>
            {preview ? preview.markdown : "选择 Workstream 后生成预览"}
          </div>

          {error && <div className="badge warn" style={{ marginTop: 10 }}>{error}</div>}

          <div className="row" style={{ justifyContent: "flex-end", marginTop: 16 }}>
            <button className="btn" onClick={onClose}>取消</button>
            <button className="btn accent" disabled={busy} onClick={launch}>
              {busy ? "启动中…" : mode === "new" ? "Start Session" : "带着最新上下文继续"}
            </button>
          </div>
        </>
      )}
    </Modal>
  );
}
