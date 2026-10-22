import { useId, useLayoutEffect, useRef, useState } from "react";
import type { TextareaHTMLAttributes } from "react";
import Icon from "../../components/Icon";
import { copyToClipboard } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { SummaryEditor } from "./useSummaryEditor";

function GrowingTextarea(props: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  const element = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const field = element.current;
    if (!field) return;
    const resize = () => {
      field.style.height = "auto";
      field.style.height = `${field.scrollHeight}px`;
    };
    resize();
    if (typeof ResizeObserver === "undefined") return;
    let width = field.clientWidth;
    const observer = new ResizeObserver(() => {
      if (field.clientWidth !== width) { width = field.clientWidth; resize(); }
    });
    observer.observe(field);
    return () => observer.disconnect();
  }, [props.value]);
  return <textarea {...props} ref={element} rows={1} />;
}

export default function SummaryItemEditor({ editor }: { editor: SummaryEditor }) {
  const id = useId();
  const [confirmRemove, setConfirmRemove] = useState(false);
  const draft = editor.draft!;
  const { item } = draft;
  const form = useRef<HTMLFormElement>(null);
  const detailCount = useRef(item.details.length);
  useLayoutEffect(() => {
    if (item.details.length > detailCount.current) {
      const fields = form.current?.querySelectorAll<HTMLTextAreaElement>("[data-summary-detail]");
      fields?.[fields.length - 1]?.focus({preventScroll:true});
    }
    detailCount.current = item.details.length;
  }, [item.details.length]);
  const change = (patch: Partial<typeof item>) => editor.change({ ...item, ...patch });
  return <form ref={form} className="summary-item-editor" aria-label={draft.index === null ? "新增事项" : `编辑事项：${draft.base.items[draft.index].title}`}
    aria-busy={editor.saving}
    onSubmit={event => { event.preventDefault(); void editor.save(); }}
    onKeyDown={event => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); void editor.save(); }
      if (event.key === "Escape" && !editor.saving) { event.preventDefault(); event.stopPropagation(); editor.cancel(); }
    }}>
    <label className="sr-only" htmlFor={`${id}-title`}>事项标题</label>
    <input id={`${id}-title`} className="summary-edit-title" value={item.title} placeholder="事项标题"
      autoFocus={!item.title_locked} readOnly={item.title_locked} disabled={editor.saving}
      aria-describedby={item.title_locked ? `${id}-lock` : undefined}
      onChange={event => change({ title: event.target.value })} />
    {item.title_locked && <span id={`${id}-lock`} className="muted small summary-edit-lock"><Icon name="lock" />标题已锁定，描述和细则仍可编辑</span>}
    <label className="sr-only" htmlFor={`${id}-description`}>事项描述</label>
    <GrowingTextarea id={`${id}-description`} className="summary-edit-description" value={item.description} placeholder="添加描述（可选）"
      autoFocus={item.title_locked} disabled={editor.saving} onChange={event => change({ description: event.target.value })} />
    {item.details.length > 0 && <ol className="summary-edit-details">
      {item.details.map((detail, index) => <li key={index}>
        <GrowingTextarea data-summary-detail aria-label={`细则 ${index + 1}`} value={detail} placeholder="细则内容" disabled={editor.saving}
          onChange={event => change({ details: item.details.map((value, at) => at === index ? event.target.value : value) })} />
        <button type="button" className="btn ghost icon-button small" aria-label={`移除细则 ${index + 1}`} title="移除细则" disabled={editor.saving}
          onClick={() => change({ details: item.details.filter((_, at) => at !== index) })}><Icon name="close" /></button>
      </li>)}
    </ol>}
    {(editor.error || editor.stale) && <div role="alert" className="summary-edit-error">
      <span>{editor.stale ? "摘要已在其他地方更新，草稿已保留。可复制草稿，取消编辑后核对最新内容。" : editor.error}</span>
      <button type="button" className="btn ghost small" onClick={async () => {
        const text = [item.title, item.description, ...item.details.map(detail => `- ${detail}`)].filter(Boolean).join("\n\n");
        showToast(await copyToClipboard(text) ? "已复制草稿" : "复制失败");
      }}>复制草稿</button>
    </div>}
    <div className="summary-edit-footer">
      {confirmRemove ? <>
        <span className="small">{draft.base.items.length === 1 ? "删除后摘要将为空。" : "删除这个事项？"}</span>
        <div className="summary-edit-save-actions">
          <button type="button" className="btn ghost small" disabled={editor.saving} onClick={() => setConfirmRemove(false)}>保留事项</button>
          <button type="button" className="btn danger small" disabled={!editor.available || editor.stale} onClick={() => void editor.save(true)}>{editor.saving ? "删除中…" : "确认删除"}</button>
        </div>
      </> : <>
        <div className="summary-edit-extra-actions">
          <button type="button" className="btn ghost small" disabled={editor.saving} onClick={() => change({ details: [...item.details, ""] })}><Icon name="plus" />添加细则</button>
          {draft.index !== null && <button type="button" className="btn ghost icon-button small danger" aria-label="删除事项"
            title={item.title_locked ? "请先解锁标题再删除事项" : "删除事项"} disabled={editor.saving || item.title_locked || editor.stale}
            onClick={() => setConfirmRemove(true)}><Icon name="trash" /></button>}
        </div>
        <div className="summary-edit-save-actions">
          <button type="button" className="btn ghost small" disabled={editor.saving} onClick={editor.cancel}>取消</button>
          <button type="submit" className="btn primary small" disabled={!editor.available || editor.stale}>{editor.saving ? "保存中…" : "保存"}</button>
        </div>
      </>}
    </div>
  </form>;
}
