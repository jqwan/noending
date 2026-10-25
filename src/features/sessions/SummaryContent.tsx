import { useId, useState, type ReactNode } from "react";
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

export function useSummaryExpansion(summary: SummaryDocument | null | undefined) {
  const [expanded, setExpanded] = useState(new Set<string>());
  const expandable = summary?.items.filter(item => item.details.length > 0).map(item => item.id) ?? [];
  const allExpanded = expandable.length > 0 && expandable.every(id => expanded.has(id));
  return {
    expanded,
    hasDetails: expandable.length > 0,
    allExpanded,
    toggle(id: string) {
      setExpanded(previous => {
        const next = new Set(previous);
        if (next.has(id)) next.delete(id); else next.add(id);
        return next;
      });
    },
    toggleAll() { setExpanded(allExpanded ? new Set() : new Set(expandable)); },

  };
}

type SummaryExpansion = ReturnType<typeof useSummaryExpansion>;

/** Place these controls beside the summary section's existing header actions. */
export function SummaryControls({ expansion, disabled = false }: { expansion: SummaryExpansion; disabled?: boolean }) {
  return <>
    {expansion.hasDetails && <button
      type="button" className="btn ghost icon-button small"
      title={expansion.allExpanded ? "全部收起" : "全部展开"}
      aria-label={expansion.allExpanded ? "全部收起" : "全部展开"}
      disabled={disabled} onClick={expansion.toggleAll}
    ><Icon name={expansion.allExpanded ? "collapseAll" : "expandAll"} /></button>}
  </>;
}

export default function SummaryContent({ children, expansion, actions, renderEditor, editing = false }: {
  children: SummaryDocument;
  expansion: SummaryExpansion;
  actions?: (item: SummaryItem) => ReactNode;
  renderEditor?: (item: SummaryItem) => ReactNode;
  editing?: boolean;
}) {
  const prefix = useId();
  const { expanded, toggle } = expansion;
  return <div className="summary-items">
    {children.items.map((item, index) => {
      const customEditor = renderEditor?.(item);
      if (customEditor) return <section className="summary-item is-editing" key={item.id}>{customEditor}</section>;
      const titleId = `${prefix}-title-${index}`;
      const detailsId = `${prefix}-details-${index}`;
      const open = expanded.has(item.id);
      return <section className="summary-item" aria-labelledby={titleId} key={item.id}>
        <div className="summary-item-header">
          <h3 id={titleId}>{item.title}</h3>
          <div className="summary-item-actions">
            {actions?.(item)}
            {item.details.length > 0 && <button
              type="button"
              className="btn ghost icon-button small"
              title={`${open ? "收起" : "展开"}细则（${item.details.length} 条）`}
              aria-expanded={open}
              aria-controls={detailsId}
              aria-label={`${open ? "收起" : "展开"}${item.title}的细则`}
              disabled={editing}
              onClick={() => toggle(item.id)}
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
  </div>;
}
