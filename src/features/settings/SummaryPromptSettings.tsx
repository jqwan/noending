import "./promptTemplates.css";
import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal, copyToClipboard } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import type { SummaryPromptKind, SummaryPromptSettings as PromptSettings, SummaryPromptTemplates } from "../../types";

const labels: Record<SummaryPromptKind, string> = { session: "会话", task: "任务", project: "项目" };

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
            调整摘要写作要求，预览完整提示词。
          </p>
        </div>
        <button className="btn small" onClick={() => setOpen(true)}>查看与编辑</button>
      </div>
      {open && <TemplateEditor onClose={() => setOpen(false)} />}
    </section>
  );
}

export function TemplateEditor({ onClose }: { onClose: () => void }) {
  const [settings, setSettings] = useState<PromptSettings | null>(null);
  const [drafts, setDrafts] = useState<SummaryPromptTemplates | null>(null);
  const [kind, setKind] = useState<SummaryPromptKind>("session");
  const [previewing, setPreviewing] = useState(false);
  const [previews, setPreviews] = useState<Partial<Record<SummaryPromptKind, string>>>({});
  const [previewLoading, setPreviewLoading] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const active = useRef(true);
  const previewRef = useRef<HTMLPreElement>(null);

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
    if (!previews[kind]) {
      setPreviewLoading(true);
    }
    setError("");
    api.previewSummaryPrompt(kind, { ...settings.templates, [kind]: drafts[kind] }).then(text => {
      if (!cancelled) {
        setPreviews(prev => ({ ...prev, [kind]: text }));
        setPreviewLoading(false);
      }
    }).catch(e => {
      if (!cancelled) {
        setError(String(e));
        setPreviewLoading(false);
      }
    });

    // 后台预加载其余类型预览，消除切 Tab 时的白屏与尺寸跳动
    const otherKinds = (Object.keys(labels) as SummaryPromptKind[]).filter(k => k !== kind);
    for (const other of otherKinds) {
      if (!previews[other]) {
        const otherTemplates = { ...settings.templates, [other]: drafts[other] };
        api.previewSummaryPrompt(other, otherTemplates).then(text => {
          if (!cancelled) {
            setPreviews(prev => ({ ...prev, [other]: text }));
          }
        }).catch(() => {});
      }
    }

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
  const currentPreview = previews[kind] || "";
  return <Modal title="摘要提取提示词" wide onClose={onClose}>
    {!settings || !drafts ? <>
      {error ? <div role="alert"><p>{error}</p><button className="btn small" onClick={() => setLoadAttempt(n => n + 1)}>重试</button></div> : <p className="muted" role="status">正在读取模板…</p>}
    </> : <>
      <div className="row" role="tablist" aria-label="摘要类型" style={{ gap: 8 }}>
        {(Object.keys(labels) as SummaryPromptKind[]).map(type => <button key={type} role="tab" aria-selected={kind === type} className={`btn small ${kind === type ? "primary" : "ghost"}`} disabled={busy} onClick={() => { setKind(type); setError(""); }}>{labels[type]}摘要</button>)}
      </div>
      <p className="muted small" style={{ margin: "4px 0 12px" }}>
        编辑摘要写作要求。会话模板同时用于任务、项目提取中的会话摘要步骤。完整提示词中的查询流程、回复协议和结果格式由系统提供。
      </p>
      <div className="row" style={{ gap: 8, marginBottom: 10, alignItems: "center" }}>
        <button className={`btn small ${!previewing ? "primary" : "ghost"}`} aria-pressed={!previewing} onClick={() => setPreviewing(false)}>编辑模板</button>
        <button className={`btn small ${previewing ? "primary" : "ghost"}`} aria-pressed={previewing} onClick={() => setPreviewing(true)}>预览完整提示词</button>
        <span className="muted small" style={{ marginLeft: "auto" }}>{changed ? "有未保存修改" : settings.templates[kind] === settings.defaults[kind] ? "默认模板" : "自定义模板"}</span>
      </div>
      <div className="summary-prompt-content-box">
        {previewing ? <>
          <div className="summary-prompt-content-header">
            <span>预览使用当前类型的编辑内容，其他要求使用已保存模板；文件路径以占位符展示。</span>
          </div>
          <pre
            key={kind}
            ref={previewRef}
            className={`summary-prompt-preview${previewLoading && !currentPreview ? " loading" : ""}`}
            aria-label={`${labels[kind]}摘要完整提示词`}
          >
            {currentPreview || (error ? "预览失败" : "正在生成预览…")}
          </pre>
        </> : <>
          <div className="summary-prompt-content-header">
            <span>{labels[kind]}摘要要求模板</span>
          </div>
          <textarea
            key={kind}
            aria-label={`${labels[kind]}摘要要求模板`}
            className="summary-prompt-editor"
            value={drafts[kind]}
            disabled={busy}
            onChange={e => { setDrafts({ ...drafts, [kind]: e.target.value }); setError(""); }}
          />
        </>}
      </div>
      {error && <p role="alert" className="danger" style={{ margin: "8px 0 0" }}>{error}</p>}
      <div className="row summary-prompt-actions" style={{ marginTop: 16, paddingTop: 14, borderTop: "1px solid var(--border-subtle)" }}>
        <button className="btn small ghost" disabled={busy || drafts[kind] === settings.defaults[kind]} onClick={() => { setDrafts({ ...drafts, [kind]: settings.defaults[kind] }); setError(""); }}>恢复默认</button>
        <button className="btn small ghost" disabled={busy || !changed} onClick={() => { setDrafts({ ...drafts, [kind]: settings.templates[kind] }); setError(""); }}>撤销修改</button>
        {previewing && <button className="btn small ghost" disabled={!currentPreview} onClick={async () => showToast(await copyToClipboard(currentPreview) ? "已复制提示词" : "复制失败")}>复制提示词</button>}
        <button className="btn small primary" disabled={busy || !changed || !drafts[kind].trim()} onClick={save}>{busy ? "保存中…" : "保存模板"}</button>
      </div>
    </>}
  </Modal>;
}
