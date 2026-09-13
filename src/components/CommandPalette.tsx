import React, { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { Route } from "../App";
import type { SearchHit, Session, Workstream } from "../types";

interface PaletteItem {
  key: string;
  kind: string;
  label: string;
  hint?: string;
  route: Route;
}

export default function CommandPalette({ onClose, navigate }: {
  onClose: () => void;
  navigate: (r: Route) => void;
}) {
  const [q, setQ] = useState("");
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    api.listWorkstreams().then((ws) => setWorkstreams(ws.filter((w) => w.visibility === "normal"))).catch(() => {});
    api.listAllSessions().then((s) => setSessions(s.slice(0, 25))).catch(() => {});
  }, []);

  useEffect(() => {
    if (q.trim().length < 2) { setHits([]); return; }
    const t = setTimeout(() => {
      api.search(q, 8).then(setHits).catch(() => {});
    }, 180);
    return () => clearTimeout(t);
  }, [q]);

  const items = useMemo<PaletteItem[]>(() => {
    const out: PaletteItem[] = [];
    const ql = q.trim().toLowerCase();
    for (const w of workstreams) {
      if (!ql || w.title.toLowerCase().includes(ql)) {
        out.push({ key: `w-${w.id}`, kind: "Workstream", label: w.title, hint: "打开", route: { view: "workstream", workstreamId: w.id } });
      }
    }
    for (const s of sessions) {
      if (!ql || (s.title ?? "").toLowerCase().includes(ql) || (s.cwd ?? "").toLowerCase().includes(ql)) {
        out.push({ key: `s-${s.id}`, kind: "Session", label: s.title ?? s.agent_session_id, hint: s.cwd ?? undefined, route: { view: "session", sessionId: s.id } });
      }
    }
    for (const h of hits) {
      const route: Route =
        h.kind === "workstream" ? { view: "workstream", workstreamId: h.ref_id }
        : h.kind === "item" ? { view: "workstream", workstreamId: h.parent_id }
        : h.kind === "event" ? { view: "session", sessionId: h.parent_id }
        : h.kind === "project" ? { view: "project", projectId: h.ref_id }
        : { view: "sessions" };
      out.push({ key: `h-${h.kind}-${h.ref_id}`, kind: h.kind === "event" ? "消息" : h.kind === "item" ? "Context" : h.kind, label: h.title.slice(0, 80), hint: "全文匹配", route });
    }
    // dedupe by key, cap
    const seen = new Set<string>();
    return out.filter((i) => !seen.has(i.key) && seen.add(i.key)).slice(0, 12);
  }, [q, workstreams, sessions, hits]);

  useEffect(() => { setSelected(0); }, [q]);

  const go = (item: PaletteItem | undefined) => {
    if (!item) return;
    navigate(item.route);
    onClose();
  };

  return (
    <div className="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-label="搜索与跳转">
        <input
          ref={inputRef}
          type="text"
          placeholder="搜索 Workstream、Session、Context…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") { e.preventDefault(); setSelected((s) => Math.min(s + 1, items.length - 1)); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setSelected((s) => Math.max(s - 1, 0)); }
            else if (e.key === "Enter") { go(items[selected]); }
            else if (e.key === "Escape") { onClose(); }
          }}
        />
        <div className="palette-list">
          {items.map((item, i) => (
            <div
              key={item.key}
              className={`palette-item ${i === selected ? "selected" : ""}`}
              onMouseEnter={() => setSelected(i)}
              onClick={() => go(item)}
            >
              <span className="badge kind">{item.kind}</span>
              <span className="label">{item.label}</span>
              {item.hint && <span className="hint">{item.hint}</span>}
            </div>
          ))}
          {items.length === 0 && (
            <div className="palette-empty">没有匹配结果。输入至少两个字符以全文检索 Context 与消息。</div>
          )}
        </div>
      </div>
    </div>
  );
}
