import React, { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import type { Project, Workstream } from "../../types";

/**
 * New Workstream 的简单 Modal（Title / Description / Project 可选）。
 * Home 与 Workstreams 页共用；创建成功后进入 Workstream Detail（§28），
 * 让用户自然补充 Context 或直接 Start Session。
 */
export default function NewWorkstreamModal({ onClose, onCreated }: {
  onClose: () => void;
  onCreated?: (w: Workstream) => void;
}) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [title, setTitle] = useState("");
  const [desc, setDesc] = useState("");
  const [project, setProject] = useState("none");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.listProjects().then(setProjects).catch(console.error);
  }, []);

  const create = async () => {
    if (!title.trim() || busy) return;
    setBusy(true);
    try {
      const w = await api.createWorkstream(project === "none" ? null : project, title, desc);
      onCreated?.(w);
      onClose();
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="Create Workstream" onClose={onClose}>
      <label className="field"><span>标题</span>
        <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus
          placeholder="例如：Context Sync / 行程设计 / 预算"
          onKeyDown={(e) => e.key === "Enter" && create()} /></label>
      <label className="field"><span>描述（可选）</span><textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
      <label className="field"><span>Project（可选）</span>
        <select value={project} onChange={(e) => setProject(e.target.value)}>
          <option value="none">不归属（Workstream 可以独立存在）</option>
          {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select></label>
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" onClick={onClose}>Cancel</button>
        <button className="btn primary" disabled={busy} onClick={create}>Create</button>
      </div>
    </Modal>
  );
}
