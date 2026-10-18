import { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { ExtensionSkillMigrationPreview, ExtensionSkillMigrationResult } from "../../types";
import "./extensions.css";

/** Register a discovered Skill by adopting its source into the NoEnding library. */
export default function SkillRegisterDialog({ id, name, onClose, onDone }: {
  id: string; name: string; onClose: () => void;
  onDone: (result: ExtensionSkillMigrationResult) => void;
}) {
  const [preview, setPreview] = useState<ExtensionSkillMigrationPreview | null>(null);
  const [deleteSource, setDeleteSource] = useState(true);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const physical = (preview?.physical_entries ?? 0) > 0;

  useEffect(() => {
    let live = true;
    void api.extensionSkillMigrationPreview(id).then(value => {
      if (live) setPreview(value);
    }).catch(e => { if (live) setError(String(e)); })
      .finally(() => { if (live) setLoading(false); });
    return () => { live = false; };
  }, [id]);

  const submit = async () => {
    if (busy || !preview) return;
    setBusy(true); setError("");
    try {
      const result = await api.migrateExtensionSkill(id, deleteSource || physical);
      showToast(result.message);
      onDone(result);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return <Modal title="转移到 NoEnding" onClose={() => { if (!busy) onClose(); }}>
    <p className="small muted">转移后会把 {name} 的源目录收进 NoEnding 源库，现有加载入口改为指向新位置，之后统一在这里启停和管理。</p>
    <dl className="extension-properties">
      <dt>源目录</dt><dd className="mono">{preview?.source_path ?? (loading ? "读取中…" : "—")}</dd>
      <dt>目标目录</dt><dd className="mono">{preview?.target_path ?? (loading ? "读取中…" : "—")}</dd>
      <dt>加载入口</dt><dd>{preview ? !preview.entry_count ? "无加载入口" : preview.physical_entries ? `${preview.entry_count} 个（其中 ${preview.physical_entries} 个物理入口）` : `${preview.entry_count} 个链接入口` : loading ? "读取中…" : "—"}</dd>
    </dl>
    {physical
      ? <>
        <label className="extension-check"><input type="checkbox" checked disabled />转移后删除源目录</label>
        <p className="small muted">源目录位于 Agent 配置目录，转移后会删除源目录并由 NoEnding 管理。</p>
      </>
      : <label className="extension-check"><input type="checkbox" checked={deleteSource} disabled={busy} onChange={e => setDeleteSource(e.target.checked)} />转移后删除源目录</label>}
    {error && <div role="alert" className="extension-error">{error}</div>}
    <div className="row extension-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn primary" disabled={busy || loading || !preview} onClick={() => void submit()}>{busy ? "转移中…" : "转移"}</button>
    </div>
  </Modal>;
}
