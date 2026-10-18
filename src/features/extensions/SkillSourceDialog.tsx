import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";

export default function SkillSourceDialog({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const [source, setSource] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const submit = async () => {
    if (!source.trim() || busy) return;
    setBusy(true); setError("");
    try {
      const result = await api.registerExtensionSkillSource(source.trim());
      showToast(result.message);
      onDone();
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <Modal title="托管 Skill 源" onClose={() => { if (!busy) onClose(); }}>
    <div className="extension-form">
      <p className="small muted">选择包含 SKILL.md 的目录。托管后源目录会移入 NoEnding 源库，不新增 Agent 入口；之后可在看板或项目详情启用。</p>
      <label>源 Skill 目录<div className="row"><input type="text" aria-label="源 Skill 目录" value={source} disabled={busy} onChange={e => setSource(e.target.value)} /><button className="btn" disabled={busy} onClick={async () => { const picked = await open({ directory: true, multiple: false, title: "选择包含 SKILL.md 的目录" }); if (typeof picked === "string") setSource(picked); }}>选择目录</button></div></label>
      {error && <div role="alert" className="extension-error">{error}</div>}
      <div className="row extension-dialog-actions"><button className="btn" disabled={busy} onClick={onClose}>取消</button><button className="btn primary" disabled={busy || !source.trim()} onClick={() => void submit()}>{busy ? "处理中…" : "托管源"}</button></div>
    </div>
  </Modal>;
}
