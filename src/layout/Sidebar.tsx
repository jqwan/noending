import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_SYNCED, type Route } from "../app/routes";
import { IntelligenceOnly } from "../app/experience";
import Icon from "../components/Icon";
import SidebarLogo from "../components/SidebarLogo";
import type { WorkstreamCardData } from "../types";

/**
 * Sidebar（Projects Experience v0.2 §1-§2、§21-§23）：Brand→Home、Search、
 * 工作区一级导航（Workstreams / Projects / Sessions / Assistant）、
 * 最近（最近 6 个 open Workstream，§21 保留）、底部固定 Settings。
 * Project 不再逐个铺在导航上——它们整体进入 Projects Board（§1）；
 * 自己负责自己的数据；AppShell 只传 route/navigate。
 */
export default function Sidebar({ route, navigate, onSearch, collapsed = false }: {
  route: Route;
  navigate: (r: Route) => void;
  onSearch: () => void;
  collapsed?: boolean;
}) {
  const [pinned, setPinned] = useState<string[]>(() => {
    try {
      const value: unknown = JSON.parse(localStorage.getItem("noending.pinnedTasks") ?? "[]");
      return Array.isArray(value) ? value.filter((id): id is string => typeof id === "string") : [];
    } catch { return []; }
  });
  const togglePin = (id: string) => setPinned(current => {
    const next = current.includes(id) ? current.filter(value => value !== id) : [...current, id];
    localStorage.setItem("noending.pinnedTasks", JSON.stringify(next));
    return next;
  });
  const [recent, setRecent] = useState<WorkstreamCardData[]>([]);

  const refresh = useCallback(() => {
    api
      .listWorkstreamCards()
      .then((cards) =>
        setRecent(
          cards
            .filter((c) => c.lifecycle === "active" && c.visibility === "normal")
            .sort((a, b) =>
              (b.last_activity_at ?? b.updated_at).localeCompare(a.last_activity_at ?? a.updated_at),
            ),
        ),
      )
      .catch(console.error);
  }, []);

  useEffect(refresh, [refresh]);
  useEffect(() => onEvent(EVT_SYNCED, refresh), [refresh]);

  const workspaceActive = (v: "workstreams" | "projects" | "sessions" | "assistant") => {
    if (route.view === v) return "active";
    // Workstream Detail → Workstreams 保持弱高亮（§10）
    if (v === "workstreams" && route.view === "workstream") return "weak";
    if (v === "sessions" && route.view === "session") return "active";
    // §22 — Project Detail → Projects 弱高亮，与 Workstreams 同一模式
    if (v === "projects" && route.view === "project") return "weak";
    return "";
  };

  return (
    <div className={`sidebar${collapsed ? " collapsed" : ""}`}>
      <button className="brand" onClick={() => navigate({ view: "home" })} title="首页">
        <SidebarLogo size={22} />
        <span className="brand-name">NoEnding</span>
      </button>


      <div className="sidebar-scroll">
        <button className={`nav-item ${route.view === "home" ? "active" : ""}`} onClick={() => navigate({ view: "home" })}>
          <Icon name="home" />首页
        </button>
        <button className="nav-item" onClick={onSearch}>
          <Icon name="search" />搜索
          <span style={{ flex: 1 }} />
          <span className="kbd">{/Macintosh|Mac OS X/.test(navigator.userAgent) ? "⌘K" : "Ctrl K"}</span>
        </button>

        <div className="nav-section">工作区</div>
        <button className={`nav-item ${workspaceActive("workstreams")}`}
          onClick={() => navigate({ view: "workstreams" })}>
          <Icon name="tasks" />任务
        </button>
        {/* §2 — Projects 成为一等导航项：Workstream=我正在做什么，
            Project=我在哪里做，Session=我做过哪些执行 */}
        <button className={`nav-item ${workspaceActive("projects")}`}
          onClick={() => navigate({ view: "projects" })}>
          <Icon name="folder" />项目
        </button>
        <button className={`nav-item ${workspaceActive("sessions")}`}
          onClick={() => navigate({ view: "sessions" })}>
          <Icon name="chat" />会话
        </button>
        <IntelligenceOnly>
          <button className={`nav-item ${workspaceActive("assistant")}`}
            onClick={() => navigate({ view: "assistant" })}>
            <Icon name="spark" />Assistant
          </button>
        </IntelligenceOnly>

        {[{ label: "固定", rows: recent.filter(w => pinned.includes(w.id)) },
          { label: "最近", rows: recent.filter(w => !pinned.includes(w.id)).slice(0, 6) }].map(group => (
          <div key={group.label}>
            {(group.rows.length > 0 || group.label === "最近") && <div className="nav-section">{group.label}</div>}
            {group.rows.map(w => (
              <div className="sidebar-task" key={w.id}>
                <button className={`nav-item ${route.view === "workstream" && route.workstreamId === w.id ? "active" : ""}`}
                  title={w.title} onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
                  <span className="truncate">{w.title}</span>
                </button>
                <button className="pin-button" aria-label={`${pinned.includes(w.id) ? "取消固定" : "固定"}：${w.title}`}
                  title={pinned.includes(w.id) ? "取消固定" : "固定"} aria-pressed={pinned.includes(w.id)} onClick={() => togglePin(w.id)}>
                  <Icon name="pin" />
                </button>
              </div>
            ))}
          </div>
        ))}
      </div>

      <div className="sidebar-footer">
        <button
          className={`nav-item ${route.view === "settings" ? "active" : ""}`}
          onClick={() => navigate({ view: "settings", section: "general" })}
        >
          <Icon name="settings" />设置
        </button>
      </div>

    </div>
  );
}
