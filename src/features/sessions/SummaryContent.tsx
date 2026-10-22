import { useEffect, useId, useRef, useState } from "react";
import { api } from "../../api";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { copyToClipboard } from "../../components/common";
import Icon from "../../components/Icon";
import { showToast } from "../../components/Toast";
import { MD_COMPONENTS } from "./SessionMessage";
import type { SummaryDocument, SummaryItem, SummaryLockResult } from "../../types";

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
  const key = JSON.stringify(summary?.items.map(({title,description,details}) => ({title,description,details})) ?? null);
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
  };
}

type SummaryExpansion = ReturnType<typeof useSummaryExpansion>;

/** Place these controls beside the summary section's existing header actions. */
export function SummaryControls({ expansion }: { expansion: SummaryExpansion }) {
  if (!expansion.hasDetails) return null;
  return <button
    type="button"
    className="btn ghost icon-button small"
    title={expansion.allExpanded ? "全部收起" : "全部展开"}
    aria-label={expansion.allExpanded ? "全部收起" : "全部展开"}
    onClick={expansion.toggleAll}
  ><Icon name={expansion.allExpanded ? "collapseAll" : "expandAll"} /></button>;
}

export default function SummaryContent({ children: summary, expansion, target, onChange }: {
  children: SummaryDocument;
  expansion: SummaryExpansion;
  target?: { kind: "session" | "task" | "project"; id: string; revision: number };
  onChange?: (result: SummaryLockResult) => void;
}) {
  const prefix = useId();
  const [locking, setLocking] = useState(false);
  const lockBusy = useRef(false);
  const targetKey = target ? `${target.kind}:${target.id}` : "";
  const currentTarget = useRef(targetKey);
  currentTarget.current = targetKey;
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const toggleTitleLock = async (index: number) => {
    if (!target || !onChange || lockBusy.current) return;
    lockBusy.current = true;
    setLocking(true);
    try {
      const result = await api.setSummaryTitleLock(target.kind,target.id,target.revision,index+1,!summary.items[index].title_locked);
      if (!mounted.current || currentTarget.current !== targetKey) return;
      onChange(result);
      window.dispatchEvent(new Event("noending:sync"));
    } catch (error) {
      if (mounted.current && currentTarget.current === targetKey) {
        showToast(`标题锁定操作失败：${String(error)}`);
        window.dispatchEvent(new Event("noending:sync"));
      }
    } finally {
      lockBusy.current = false;
      if (mounted.current) setLocking(false);
    }
  };
  const { expanded, toggle } = expansion;
  return <div className="summary-items">
    {summary.items.map((item, index) => {
      const titleId = `${prefix}-title-${index}`;
      const detailsId = `${prefix}-details-${index}`;
      const open = expanded.has(index);
      return <section className="summary-item" aria-labelledby={titleId} key={index}>
        <div className="summary-item-header">
          <h3 id={titleId}>{item.title}</h3>
          <div className="summary-item-actions">
            {target && onChange && <button
              type="button"
              className={`btn ghost icon-button small${item.title_locked ? " on" : ""}`}
              title={item.title_locked ? "解锁标题（描述和细则仍可更新）" : "锁定标题（描述和细则仍可更新）"}
              aria-label={`${item.title_locked ? "解锁" : "锁定"}标题：${item.title}`}
              aria-pressed={item.title_locked}
              disabled={locking}
              onClick={() => toggleTitleLock(index)}
            ><Icon name={item.title_locked ? "lock" : "unlock"} /></button>}
            {item.details.length > 0 && <button
              type="button"
              className="btn ghost icon-button small"
              title={`${open ? "收起" : "展开"}细则（${item.details.length} 条）`}
              aria-expanded={open}
              aria-controls={detailsId}
              aria-label={`${open ? "收起" : "展开"}${item.title}的细则`}
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
  </div>;
}
