import React, { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Agent, type LaunchResult, type Workstream } from "../../types";
import LaunchResultModal from "../launcher/LaunchResultModal";

/**
 * 全局 New Session（实施方案 §33/§41）：无 Workstream 前置。
 * 默认 Workstream = None、Agent = Default Agent，用户可直接 Start。
 */
export default function NewSessionModal({ onClose }: { onClose: () => void }) {
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [wsId, setWsId] = useState("none");
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<LaunchResult | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    api.listWorkstreams().then((ws) => setWorkstreams(ws.filter((w) => w.visibility === "normal"))).catch(console.error);
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
  }, []);

  const start = async () => {
    if (!defaultAgent || busy) return;
    setBusy(true); setError("");
    try {
      setResult(await api.launchNewSession(defaultAgent, wsId === "none" ? [] : [wsId]));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (result) return <LaunchResultModal result={result} onClose={onClose} />;

  return (
    <Modal title="New Session" onClose={onClose}>
      <label className="field"><span>Workstream（可不选，Session 之后由 Sync 自动关联）</span>
        <select value={wsId} onChange={(e) => setWsId(e.target.value)}>
          <option value="none">None（直接开始）</option>
          {workstreams.map((w) => <option key={w.id} value={w.id}>{w.title}</option>)}
        </select></label>
      <div className="row-line" style={{ borderTop: 0 }}>
        <div>
          <div className="settings-row-label">Agent</div>
          <div className="settings-row-hint">使用 Settings 中的 Default Agent</div>
        </div>
        {defaultAgent && (
          <span className="badge accent ws-btn" style={{ gap: 6 }}>
            <AgentIcon agent={defaultAgent} />
            {AGENT_LABELS[defaultAgent]}
          </span>
        )}
      </div>
      {error && <div className="badge warn" style={{ marginTop: 8 }}>{error}</div>}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn" onClick={onClose}>取消</button>
        <button className="btn primary" disabled={busy || !defaultAgent} onClick={start}>
          {busy ? "启动中…" : "Start"}
        </button>
      </div>
    </Modal>
  );
}
