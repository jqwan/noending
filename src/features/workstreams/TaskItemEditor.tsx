import { useId, useLayoutEffect, useRef, useState } from "react";
import type { TextareaHTMLAttributes } from "react";
import Icon from "../../components/Icon";
import { copyToClipboard } from "../../components/common";
import { showToast } from "../../components/Toast";
import type { SummaryDocument, SummaryItem } from "../../types";

export interface ItemEditorState {
  draft: { index: number | null; base: SummaryDocument; item: SummaryItem } | null;
  saving: boolean;
  available: boolean;
  stale: boolean;
  error: string;
  save(remove?: boolean): unknown;
  cancel(): void;
  change(item: SummaryItem): void;
}

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

export default function TaskItemEditor({ editor, showDetails = true }: {
  editor: ItemEditorState;
  showDetails?: boolean;
}) {
  const id = useId();
  const [confirmRemove, setConfirmRemove] = useState(false);
  const draft = editor.draft!;
  const { item } = draft;
  const form = useRef<HTMLFormElement>(null);
  const detailCount = useRef(item.details.length);
  useLayoutEffect(() => {
    if (item.details.length > detailCount.current) {
      const fields = form.current?.querySelectorAll<HTMLTextAreaElement>("[data-task-detail]");
      fields?.[fields.length - 1]?.focus({preventScroll:true});
    }
    detailCount.current = item.details.length;
  }, [item.details.length]);
  const change = (patch: Partial<typeof item>) => editor.change({ ...item, ...patch });
  return <form ref={form} className="task-item-editor" aria-label={draft.index === null ? "新增事项" : `编辑事项：${draft.base.items[draft.index].title}`}
    aria-busy={editor.saving}
    onSubmit={event => { event.preventDefault(); void editor.save(); }}
    onKeyDown={event => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); void editor.save(); }
      if (event.key === "Escape" && !editor.saving) { event.preventDefault(); event.stopPropagation(); editor.cancel(); }
    }}>
    <label className="sr-only" htmlFor={`${id}-title`}>事项标题</label>
    <input id={`${id}-title`} className="task-edit-title" value={item.title} placeholder="事项标题"
      autoFocus disabled={editor.saving}
      onChange={event => change({ title: event.target.value })} />
    <label className="sr-only" htmlFor={`${id}-description`}>事项描述</label>
    <GrowingTextarea id={`${id}-description`} className="task-edit-description" value={item.description} placeholder="添加描述（可选）"
      disabled={editor.saving} onChange={event => change({ description: event.target.value })} />
    {item.details.length > 0 && <ol className="task-edit-details">
      {item.details.map((detail, index) => <li key={index}>
        <GrowingTextarea data-task-detail aria-label={`细则 ${index + 1}`} value={detail} placeholder="细则内容" disabled={editor.saving}
          onChange={event => change({ details: item.details.map((value, at) => at === index ? event.target.value : value) })} />
        <button type="button" className="btn ghost icon-button small" aria-label={`移除细则 ${index + 1}`} title="移除细则" disabled={editor.saving}
          onClick={() => change({ details: item.details.filter((_, at) => at !== index) })}><Icon name="close" /></button>
      </li>)}
    </ol>}
    {(editor.error || editor.stale) && <div role="alert" className="task-edit-error">
      <span>{editor.stale ? "任务事项已在其他地方修改或删除。草稿已保留，请复制后取消编辑，核对最新内容。" : editor.error}</span>
      <button type="button" className="btn ghost small" onClick={async () => {
        const text = [item.title, item.description, ...item.details.map(detail => `- ${detail}`)].filter(Boolean).join("\n\n");
        showToast(await copyToClipboard(text) ? "已复制草稿" : "复制失败");
      }}>复制草稿</button>
    </div>}
    <div className="task-edit-footer">
      {confirmRemove ? <>
        <span className="small">{"永久删除这个任务事项？"}</span>
        <div className="task-edit-save-actions">
          <button type="button" className="btn ghost small" disabled={editor.saving} onClick={() => setConfirmRemove(false)}>保留事项</button>
          <button type="button" className="btn danger small" disabled={!editor.available || editor.stale} onClick={() => void editor.save(true)}>{editor.saving ? "删除中…" : "确认删除"}</button>
        </div>
      </> : <>
        <div className="task-edit-extra-actions">
          {showDetails && <button type="button" className="btn ghost small" disabled={editor.saving} onClick={() => change({ details: [...item.details, ""] })}><Icon name="plus" />添加细则</button>}
          {draft.index !== null && <button type="button" className="btn ghost icon-button small danger" aria-label="删除事项"
            title="删除事项" disabled={editor.saving || editor.stale}
            onClick={() => setConfirmRemove(true)}><Icon name="trash" /></button>}
        </div>
        <div className="task-edit-save-actions">
          <button type="button" className="btn ghost small" disabled={editor.saving} onClick={editor.cancel}>取消</button>
          <button type="submit" className="btn primary small" disabled={!editor.available || editor.stale}>{editor.saving ? "保存中…" : "保存"}</button>
        </div>
      </>}
    </div>
  </form>;
}
