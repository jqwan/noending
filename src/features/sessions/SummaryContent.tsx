import { useId, useLayoutEffect, useRef, useState } from "react";
import SummaryItemEditor from "./SummaryItemEditor";
import type { SummaryEditor } from "./useSummaryEditor";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { copyToClipboard } from "../../components/common";
import Icon from "../../components/Icon";
import { showToast } from "../../components/Toast";
import { MD_COMPONENTS } from "./SessionMessage";
import type { SummaryDocument, SummaryItem } from "../../types";

function itemMarkdown(item: SummaryItem): string {
  return [
    `### ${item.title}`,
    item.description,
    item.details.map(detail => `- ${detail.replace(/\n/g, "\n  ")}`).join("\n"),
  ].filter(Boolean).join("\n\n");
}

export function summaryMarkdown(summary: SummaryDocument): string {
  return summary.items.map(itemMarkdown).join("\n\n");
}

function Text({ children }: { children: string }) {
  return <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{children}</ReactMarkdown>;
}

function contentKey(summary: SummaryDocument | null | undefined) {
  return JSON.stringify(summary?.items.map(({title,description,details}) => ({title,description,details})) ?? null);
}

export function useSummaryExpansion(summary: SummaryDocument | null | undefined) {
  const key = contentKey(summary);
  const [state, setState] = useState(() => ({ key, expanded: new Set<number>() }));
  // Positions belong to one content version; identical reads keep their state.
  const current = state.key === key ? state : { key, expanded: new Set<number>() };
  if (current !== state) setState(current);
  const expandable = summary?.items.flatMap((item, index) => item.details.length > 0 ? [index] : []) ?? [];
  const allExpanded = expandable.length > 0 && expandable.every(index => current.expanded.has(index));
  return {
    expanded: current.expanded,
    hasDetails: expandable.length > 0,
    allExpanded,
    toggle(index: number) {
      setState(previous => {
        const expanded = new Set(previous.key === key ? previous.expanded : []);
        if (expanded.has(index)) expanded.delete(index);
        else expanded.add(index);
        return { key, expanded };
      });
    },
    toggleAll() {
      setState({ key, expanded: allExpanded ? new Set() : new Set(expandable) });
    },
    preserve(next: SummaryDocument, removedIndex?: number) {
      const expanded = [...current.expanded]
        .filter(index => index !== removedIndex)
        .map(index => removedIndex !== undefined && index > removedIndex ? index - 1 : index)
        .filter(index => index < next.items.length);
      setState({key:contentKey(next),expanded:new Set(expanded)});
    },
  };
}

type SummaryExpansion = ReturnType<typeof useSummaryExpansion>;

/** Place these controls beside the summary section's existing header actions. */
export function SummaryControls({ expansion, editor }: { expansion: SummaryExpansion; editor?: SummaryEditor }) {
  return <>
    {editor && <button type="button" className="btn ghost icon-button small" title="新增事项" aria-label="新增事项"
      disabled={!editor.available || editor.editing} onClick={() => editor.start(null)}><Icon name="plus" /></button>}
    {expansion.hasDetails && <button
      type="button" className="btn ghost icon-button small"
      title={expansion.allExpanded ? "全部收起" : "全部展开"}
      aria-label={expansion.allExpanded ? "全部收起" : "全部展开"}
      disabled={editor?.editing} onClick={expansion.toggleAll}
    ><Icon name={expansion.allExpanded ? "collapseAll" : "expandAll"} /></button>}
  </>;
}

export default function SummaryContent({ children, expansion, editor }: {
  children: SummaryDocument;
  expansion: SummaryExpansion;
  editor?: SummaryEditor;
}) {
  const prefix = useId();
  const summary = editor?.draft?.base ?? children;
  const editButtons = useRef(new Map<number, HTMLButtonElement>());
  const lastEdited = useRef<number | null>(null);
  useLayoutEffect(() => {
    if (editor?.draft) lastEdited.current = editor.draft.index ?? children.items.length;
    else if (lastEdited.current !== null) {
      const index = Math.min(lastEdited.current, children.items.length - 1);
      editButtons.current.get(index)?.focus({preventScroll:true});
      lastEdited.current = null;
    }
  }, [editor?.draft, children.items.length]);
  const { expanded, toggle } = expansion;
  return <div className="summary-items">
    {summary.items.map((item, index) => {
      if (editor?.draft?.index === index) return <section className="summary-item is-editing" key={index}><SummaryItemEditor editor={editor} /></section>;
      const titleId = `${prefix}-title-${index}`;
      const detailsId = `${prefix}-details-${index}`;
      const open = expanded.has(index);
      return <section className="summary-item" aria-labelledby={titleId} key={index}>
        <div className="summary-item-header">
          <h3 id={titleId}>{item.title}</h3>
          <div className="summary-item-actions">
            {editor && <>
              <button ref={element => { if (element) editButtons.current.set(index,element); else editButtons.current.delete(index); }}
                type="button" className="btn ghost icon-button small" title="编辑事项" aria-label={`编辑事项：${item.title}`}
                disabled={!editor.available || editor.editing} onClick={() => editor.start(index)}><Icon name="edit" /></button>
              <button
                type="button" className={`btn ghost icon-button small${item.title_locked ? " on" : ""}`}
                title={item.title_locked ? "解锁标题（描述和细则仍可更新）" : "锁定标题（描述和细则仍可更新）"}
                aria-label={`${item.title_locked ? "解锁" : "锁定"}标题：${item.title}`}
                aria-pressed={item.title_locked} disabled={!editor.available || editor.editing}
                onClick={() => void editor.toggleTitleLock(index)}
              ><Icon name={item.title_locked ? "lock" : "unlock"} /></button>
            </>}
            {item.details.length > 0 && <button
              type="button"
              className="btn ghost icon-button small"
              title={`${open ? "收起" : "展开"}细则（${item.details.length} 条）`}
              aria-expanded={open}
              aria-controls={detailsId}
              aria-label={`${open ? "收起" : "展开"}${item.title}的细则`}
              disabled={editor?.editing}
              onClick={() => toggle(index)}
            ><Icon name={open ? "chevronDown" : "chevronRight"} /></button>}
            <button
              type="button"
              className="btn ghost icon-button small"
              title="复制事项"
              aria-label={`复制事项：${item.title}`}
              onClick={async () => showToast(await copyToClipboard(itemMarkdown(item)) ? "已复制事项" : "复制失败")}
            ><Icon name="copy" /></button>
          </div>
        </div>
        {item.description && <div className="summary-item-description"><Text>{item.description}</Text></div>}
        {item.details.length > 0 &&
          <ul className="summary-item-details" id={detailsId} hidden={!open}>
            {item.details.map((detail, detailIndex) => <li key={detailIndex}><Text>{detail}</Text></li>)}
          </ul>
        }
      </section>;
    })}
    {editor?.draft?.index === null && <section className="summary-item is-editing"><SummaryItemEditor editor={editor} /></section>}
  </div>;
}
