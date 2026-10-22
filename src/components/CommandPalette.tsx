import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { submitsOnEnter } from "./common";
import type { Route } from "../app/routes";
import type { NavigationItem, NavigationResults } from "../types";

const KIND_LABELS: Record<NavigationItem["kind"], string> = {
  project: "项目",
  workstream: "任务",
  session: "会话",
  agent: "Agent",
  skill: "技能",
};

function destination(item: NavigationItem): Route | null {
  switch (item.kind) {
    case "project": return { view: "project", projectId: item.ref_id, initialTitle: item.title };
    case "workstream": return { view: "workstream", workstreamId: item.ref_id, initialTitle: item.title };
    case "session": return { view: "session", sessionId: item.ref_id, initialTitle: item.title, initialAgent: item.agent ?? undefined };
    case "agent": return item.agent ? { view: "agents", agent: item.agent } : null;
    case "skill": return { view: "skill", resourceId: item.ref_id, initialTitle: item.title };
  }
}

/** Find an existing object and open its detail. Content search is a separate surface. */
export default function CommandPalette({ onClose, navigate }: {
  onClose: () => void;
  navigate: (r: Route) => void;
}) {
  const [q, setQ] = useState("");
  const query = q.trim();
  const [snapshot, setSnapshot] = useState<{ query: string; result?: NavigationResults; failed?: boolean }>({ query: "" });
  const [selected, setSelected] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const result = snapshot.query === query ? snapshot.result : undefined;
  const failed = snapshot.query === query && snapshot.failed;
  const items = result?.items ?? [];

  useEffect(() => { inputRef.current?.focus(); }, []);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest" });
  }, [selected, result]);

  useEffect(() => {
    let active = true;
    setSelected(0);
    setSnapshot({ query });
    const timer = setTimeout(() => {
      api.lookupNavigation(query).then(result => {
        if (active) setSnapshot({ query, result });
      }).catch(() => {
        if (active) setSnapshot({ query, failed: true });
      });
    }, query ? 180 : 0);
    return () => { active = false; clearTimeout(timer); };
  }, [query, attempt]);

  const go = (item: NavigationItem | undefined) => {
    if (!item) return;
    const route = destination(item);
    if (!route) return;
    navigate(route);
    onClose();
  };

  return (
    <div className="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-label="查找" aria-modal="true">
        <input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-autocomplete="list"
          aria-controls="navigation-results"
          aria-expanded="true"
          aria-activedescendant={items[selected] ? `navigation-result-${selected}` : undefined}
          aria-label="查找项目、任务、会话、Agent 或技能"
          placeholder="查找项目、任务、会话、Agent 或技能…"
          value={q}
          onChange={(e) => { setQ(e.target.value); setSelected(0); }}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
            if (e.key === "ArrowDown") { e.preventDefault(); setSelected(s => Math.max(0, Math.min(s + 1, items.length - 1))); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setSelected(s => Math.max(s - 1, 0)); }
            else if (submitsOnEnter(e)) { e.preventDefault(); go(items[selected]); }
            else if (e.key === "Escape") { onClose(); }
          }}
        />
        <div ref={listRef} id="navigation-results" className="palette-list" role="listbox" aria-label="查找结果" aria-busy={!result && !failed}>
          {items.map((item, i) => (
            <div
              key={`${item.kind}:${item.ref_id}`}
              className={`palette-item ${i === selected ? "selected" : ""}`}
              role="option"
              id={`navigation-result-${i}`}
              aria-selected={i === selected}
              onMouseEnter={() => setSelected(i)}
              onClick={() => go(item)}
            >
              <span className="badge kind">{KIND_LABELS[item.kind]}</span>
              <span className="label" title={item.title}>{item.title}</span>
              {item.archived && <span className="badge warn">已归档</span>}
              {item.hint && <span className="hint" title={item.hint}>{item.hint}</span>}
            </div>
          ))}
        </div>
        {!result && !failed && <div className="palette-empty" role="status">正在查找…</div>}
        {failed && <div className="palette-empty" role="alert">查找失败。<button className="btn small ghost" onClick={() => setAttempt(a => a + 1)}>重试</button></div>}
        {result && items.length === 0 && <div className="palette-empty">没有匹配的项目、任务、会话、Agent 或技能。</div>}
        {result && items.length > 0 && !query && <div className="palette-empty">最近会话与 Agent；输入名称或目录查找其他对象。</div>}
        {result?.has_more && <div className="palette-empty">结果较多，请补充名称或目录缩小范围。</div>}
      </div>
    </div>
  );
}
