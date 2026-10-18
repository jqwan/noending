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

  return <Modal title="托管到 NoEnding" onClose={() => { if (!busy) onClose(); }}>
    <p className="small muted" style={{ margin: "0 0 14px" }}>将 {name} 托管到 NoEnding 统一管理，之后可在用户级或项目中灵活启停；现有 Agent 入口将自动指向新位置。</p>
    <dl className="extension-properties">
      <dt>源目录</dt><dd className="mono">{preview?.source_path ?? (loading ? "读取中…" : "—")}</dd>
      <dt>加载入口</dt><dd>{preview ? !preview.entry_count ? "无加载入口" : preview.physical_entries ? `${preview.entry_count} 个（其中 ${preview.physical_entries} 个物理入口）` : `${preview.entry_count} 个链接入口` : loading ? "读取中…" : "—"}</dd>
    </dl>
    <div className="extension-dialog-option">
      <label className={`extension-check${physical || deleteSource ? " danger" : ""}${physical ? " disabled" : ""}`}>
        <input
          type="checkbox"
          checked={physical ? true : deleteSource}
          disabled={busy || physical}
          onChange={e => setDeleteSource(e.target.checked)}
        />
        <span>托管后删除源目录</span>
      </label>
      <p className={`small ${physical || deleteSource ? "extension-danger-note" : "extension-safe-note"}`}>
        {physical
          ? "源目录位于 Agent 配置目录，托管后会删除源目录并由 NoEnding 管理。"
          : deleteSource
            ? "源目录将移入 NoEnding，删除外部原目录。"
            : "源目录将复制到 NoEnding，保留外部原目录。"}
      </p>
    </div>
    {error && <div role="alert" className="extension-error">{error}</div>}
    <div className="row extension-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn primary" disabled={busy || loading || !preview} onClick={() => void submit()}>{busy ? "托管中…" : "托管"}</button>
    </div>
  </Modal>;
}
