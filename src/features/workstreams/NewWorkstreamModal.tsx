import React, { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import type { Project, Workstream } from "../../types";

/**
 * 新建 Workstream 的简单 Modal（标题 / 描述 / 工作目录 / Project 均可选）。
 * Home 与 Workstreams 页共用；创建成功后进入 Workstream Detail（§28），
 * 让用户直接补充标题与描述，或在那里新建第一个 Session。
 * 从 Project Detail 打开时传入 initialProjectId 继承当前 Project
 * （上下文操作语义）；用户仍可手动切换为「不归属」。
 */
export default function NewWorkstreamModal({ onClose, onCreated, initialProjectId }: {
  onClose: () => void;
  onCreated?: (w: Workstream) => void;
  initialProjectId?: string;
}) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [title, setTitle] = useState("");
  const [desc, setDesc] = useState("");
  const [project, setProject] = useState(initialProjectId ?? "none");
  const [defaultCwd, setDefaultCwd] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api.listProjects().then(setProjects).catch(console.error);
  }, []);

  const create = async () => {
    if (!title.trim() || busy) return;
    setBusy(true);
    setError("");
    try {
      const w = await api.createWorkstream(
        project === "none" ? null : project,
        title,
        desc,
        defaultCwd,
      );
      onCreated?.(w);
      onClose();
    } catch (e) {
      // 失败时留在弹窗里、把原因说出来：静默关闭会让用户以为已经建好了。
      console.error(e);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="新建 Workstream" onClose={onClose}>
      <label className="field"><span>标题</span>
        <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus
          placeholder="例如：接口设计 / 行程规划 / 预算整理"
          onKeyDown={(e) => e.key === "Enter" && create()} /></label>
      <label className="field"><span>描述（可选）</span><textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
      <label className="field"><span>工作目录（可选）</span>
        <input type="text" value={defaultCwd} onChange={(e) => setDefaultCwd(e.target.value)}
          className="mono"
          placeholder="/path/to/project — 该 Workstream 的新建 Session 默认在此目录启动" /></label>
      <label className="field"><span>Project（可选）</span>
        <select value={project} onChange={(e) => setProject(e.target.value)}>
          <option value="none">不归属（Workstream 可以独立存在）</option>
          {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select></label>
      {error && (
        <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>{error}</div>
      )}
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" onClick={onClose} disabled={busy}>取消</button>
        <button className="btn primary" disabled={busy || !title.trim()} onClick={create}>
          {busy ? "创建中…" : "创建"}
        </button>
      </div>
    </Modal>
  );
}
