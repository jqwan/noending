import React, { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { Route } from "../app/routes";
import type { SearchHit, Session, Workstream } from "../types";

interface PaletteItem {
  key: string;
  kind: string;
  label: string;
  hint?: string;
  route: Route;
}

/** 固定命令（实施方案 §53）：导航 + New 动作，不与实体搜索混淆。
 *  New 动作以 route.action 携带意图，目标页已挂载时同样会打开 Modal。 */
const FIXED_COMMANDS: PaletteItem[] = [
  { key: "cmd-home", kind: "命令", label: "Go to Home", hint: "继续最近的工作", route: { view: "home" } },
  { key: "cmd-workstreams", kind: "命令", label: "Go to Workstreams", route: { view: "workstreams" } },
  { key: "cmd-sessions", kind: "命令", label: "Go to Sessions", route: { view: "sessions" } },
  { key: "cmd-assistant", kind: "命令", label: "Go to Assistant", route: { view: "assistant" } },
  { key: "cmd-projects", kind: "命令", label: "Go to Projects", route: { view: "projects" } },
  { key: "cmd-settings", kind: "命令", label: "Go to Settings", route: { view: "settings", section: "general" } },
  { key: "cmd-new-ws", kind: "命令", label: "New Workstream", route: { view: "workstreams", action: "new" }, hint: "创建" },
  { key: "cmd-new-session", kind: "命令", label: "New Session", route: { view: "sessions", action: "new" }, hint: "默认 Agent" },
];

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
    // 固定命令：输入为空时全部可见；输入后按子串过滤
    for (const c of FIXED_COMMANDS) {
      if (!ql || c.label.toLowerCase().includes(ql)) out.push(c);
    }
    if (!ql || "workstreams".includes(ql)) {
      out.push({ key: "nav-workstreams", kind: "页面", label: "Workstreams", hint: "看板", route: { view: "workstreams" } });
    }
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
    // New 命令：意图随 route.action 到达页面，由页面打开 Modal
    // （保持可取消、可选项；已在目标页时同样生效）
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
