import { useEffect, useState } from "react";
import { api } from "../../api";
import { displayPath } from "../../pathDisplay";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { SkillRegistrationPreview, SkillRegistrationResult } from "../../types";
import "./skills.css";

/** Manage a discovered Skill by moving its source or referencing it in place. */
export default function SkillRegisterDialog({ id, name, onClose, onDone }: {
  id: string; name: string; onClose: () => void;
  onDone: (result: SkillRegistrationResult) => void;
}) {
  const [preview, setPreview] = useState<SkillRegistrationPreview | null>(null);
  const [migrateSource, setMigrateSource] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const physical = (preview?.physical_entries ?? 0) > 0;

  useEffect(() => {
    let live = true;
    void api.skillRegistrationPreview(id).then(value => {
      if (live) setPreview(value);
    }).catch(e => { if (live) setError(String(e)); })
      .finally(() => { if (live) setLoading(false); });
    return () => { live = false; };
  }, [id]);

  const submit = async () => {
    if (busy || !preview) return;
    setBusy(true); setError("");
    try {
      const result = await api.registerDiscoveredSkill(id, migrateSource);
      showToast(result.message);
      onDone(result);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return <Modal title="托管到 NoEnding" onClose={() => { if (!busy) onClose(); }}>
    <p className="small muted" style={{ margin: "0 0 14px" }}>将 {name} 登记到 NoEnding，管理用户级和项目级加载入口。</p>
    <dl className="skill-properties">
      <dt>源目录</dt><dd className="mono">{preview ? displayPath(preview.source_path) : loading ? "读取中…" : "—"}</dd>
      <dt>加载入口</dt><dd>{preview ? !preview.entry_count ? "无加载入口" : preview.physical_entries ? `${preview.entry_count} 个（其中 ${preview.physical_entries} 个物理入口）` : `${preview.entry_count} 个链接入口` : loading ? "读取中…" : "—"}</dd>
    </dl>
    <div className="skill-dialog-option">
      <label className={`skill-check${migrateSource ? " danger" : ""}`}>
        <input type="checkbox" checked={migrateSource} disabled={busy}
          onChange={e => setMigrateSource(e.target.checked)} />
        <span>迁移源目录到 NoEnding</span>
      </label>
      <p className={`small ${migrateSource ? "skill-danger-note" : "skill-safe-note"}`}>
        {migrateSource
          ? "源目录将移入 NoEnding 技能库，原加载入口改为指向新位置的链接。"
          : "保留原目录，只登记源路径，不移动或复制文件，也不改动现有加载入口。"}
      </p>
      {!migrateSource && physical && <p className="small skill-safe-note">源目录位于 Agent 生效路径，对应作用域会保持启用，不能直接停用该物理入口。</p>}
    </div>
    {error && <div role="alert" className="skill-error">{error}</div>}
    <div className="row skill-dialog-actions">
      <button className="btn" disabled={busy} onClick={onClose}>取消</button>
      <button className="btn primary" disabled={busy || loading || !preview} onClick={() => void submit()}>{busy ? "托管中…" : "托管"}</button>
    </div>
  </Modal>;
}
