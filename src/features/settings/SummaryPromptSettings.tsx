import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal, copyToClipboard } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import type { SummaryPromptKind, SummaryPromptSettings as PromptSettings, SummaryPromptTemplates } from "../../types";

const labels: Record<SummaryPromptKind, string> = { session: "会话", task: "任务", project: "项目", preferences: "偏好" };

export default function SummaryPromptSettings() {
  const [open, setOpen] = useState(false);
  return (
    <section>
      <div className="settings-section-header">
        <div>
          <h3 style={{ display: "flex", alignItems: "center", gap: 8, margin: 0 }}>
            <Icon name="edit" />
            <span>摘要提取提示词</span>
          </h3>
          <p className="muted small" style={{ margin: "4px 0 0" }}>
            调整摘要写作与偏好提取要求，预览完整提示词。
          </p>
        </div>
        <button className="btn small" onClick={() => setOpen(true)}>查看与编辑</button>
      </div>
      {open && <TemplateEditor onClose={() => setOpen(false)} />}
    </section>
  );
}

function TemplateEditor({ onClose }: { onClose: () => void }) {
  const [settings, setSettings] = useState<PromptSettings | null>(null);
  const [drafts, setDrafts] = useState<SummaryPromptTemplates | null>(null);
  const [kind, setKind] = useState<SummaryPromptKind>("session");
  const [previewing, setPreviewing] = useState(false);
  const [preview, setPreview] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const active = useRef(true);

  useEffect(() => {
    active.current = true;
    let cancelled = false;
    setError("");
    api.getSummaryPromptSettings().then(next => {
      if (cancelled) return;
      setSettings(next);
      setDrafts(next.templates);
    }).catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; active.current = false; };
  }, [loadAttempt]);

  useEffect(() => {
    if (!previewing || !drafts || !settings) return;
    let cancelled = false;
    setPreview("");
    setError("");
    api.previewSummaryPrompt(kind, { ...settings.templates, [kind]: drafts[kind] }).then(text => {
      if (!cancelled) setPreview(text);
    }).catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [previewing, kind, drafts, settings]);

  const save = async () => {
    if (!drafts || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      const next = await api.setSummaryPromptTemplate(kind, drafts[kind]);
      if (!active.current) return;
      setSettings(next);
      setDrafts(current => current && { ...current, [kind]: next.templates[kind] });
      showToast("已保存，下次摘要提取生效");
    } catch (e) {
      if (active.current) setError(String(e));
    } finally {
      busyRef.current = false;
      if (active.current) setBusy(false);
    }
  };

  const changed = settings && drafts && drafts[kind] !== settings.templates[kind];
  return <Modal title="摘要提取提示词" wide onClose={onClose}>
    {!settings || !drafts ? <>
      {error ? <div role="alert"><p>{error}</p><button className="btn small" onClick={() => setLoadAttempt(n => n + 1)}>重试</button></div> : <p className="muted" role="status">正在读取模板…</p>}
    </> : <>
      <div className="row" role="tablist" aria-label="摘要类型" style={{ gap: 8 }}>
        {(Object.keys(labels) as SummaryPromptKind[]).map(type => <button key={type} role="tab" aria-selected={kind === type} className={`btn small ${kind === type ? "primary" : "ghost"}`} disabled={busy} onClick={() => { setKind(type); setError(""); }}>{labels[type]}{type === "preferences" ? "提取" : "摘要"}</button>)}
      </div>
      <p className="muted small">
        编辑摘要写作或偏好提取要求。偏好模板用于所有摘要提取；会话模板同时用于任务、项目提取中的会话摘要步骤。完整提示词中的查询流程、回复协议和结果格式由系统提供。
      </p>
      <div className="row" style={{ gap: 8, marginBottom: 12 }}>
        <button className={`btn small ${!previewing ? "primary" : "ghost"}`} aria-pressed={!previewing} onClick={() => setPreviewing(false)}>编辑模板</button>
        <button className={`btn small ${previewing ? "primary" : "ghost"}`} aria-pressed={previewing} onClick={() => setPreviewing(true)}>预览完整提示词</button>
        <span className="muted small">{changed ? "有未保存修改" : settings.templates[kind] === settings.defaults[kind] ? "默认模板" : "自定义模板"}</span>
      </div>
      {previewing ? <>
        <p className="muted small">预览使用当前类型的编辑内容，其他要求使用已保存模板；文件路径以占位符展示。</p>
        <pre className="summary-prompt-preview" aria-label={`${labels[kind]}${kind === "preferences" ? "提取" : "摘要"}完整提示词`}>{preview || (error ? "预览失败" : "正在生成预览…")}</pre>
      </> : <label className="field">
        <span>{labels[kind]}{kind === "preferences" ? "提取" : "摘要"}要求模板</span>
        <textarea className="summary-prompt-editor" value={drafts[kind]} disabled={busy} onChange={e => { setDrafts({ ...drafts, [kind]: e.target.value }); setError(""); }} />
      </label>}
      {error && <p role="alert" className="danger">{error}</p>}
      <div className="row summary-prompt-actions" style={{ marginTop: 16, paddingTop: 14, borderTop: "1px solid var(--border-subtle)" }}>
        <button className="btn small ghost" disabled={busy || drafts[kind] === settings.defaults[kind]} onClick={() => { setDrafts({ ...drafts, [kind]: settings.defaults[kind] }); setError(""); }}>恢复默认</button>
        <button className="btn small ghost" disabled={busy || !changed} onClick={() => { setDrafts({ ...drafts, [kind]: settings.templates[kind] }); setError(""); }}>撤销修改</button>
        {previewing && <button className="btn small ghost" disabled={!preview} onClick={async () => showToast(await copyToClipboard(preview) ? "已复制提示词" : "复制失败")}>复制提示词</button>}
        <button className="btn small primary" disabled={busy || !changed || !drafts[kind].trim()} onClick={save}>{busy ? "保存中…" : "保存模板"}</button>
      </div>
    </>}
  </Modal>;
}
