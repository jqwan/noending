import React, { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_SYNCED, type Route } from "../app/routes";
import { IntelligenceOnly } from "../app/experience";
import SidebarLogo from "../components/SidebarLogo";
import type { Project, WorkstreamCardData } from "../types";

/**
 * Sidebar（整体设计方案 §5-§11）：Brand→Home、Search、WORKSPACE 一级导航、
 * RECENT（最近 6 个 open Workstream）、PROJECTS（独立展示）、底部固定 Settings。
 * 自己负责自己的数据；AppShell 只传 route/navigate。
 */
export default function Sidebar({ route, navigate, onSearch }: {
  route: Route;
  navigate: (r: Route) => void;
  onSearch: () => void;
}) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [recent, setRecent] = useState<WorkstreamCardData[]>([]);

  const refresh = useCallback(() => {
    api.listProjects().then(setProjects).catch(console.error);
    api
      .listWorkstreamCards()
      .then((cards) =>
        setRecent(
          cards
            .filter((c) => c.lifecycle === "active" && c.visibility === "normal")
            .sort((a, b) =>
              (b.last_activity_at ?? b.updated_at).localeCompare(a.last_activity_at ?? a.updated_at),
            )
            .slice(0, 6),
        ),
      )
      .catch(console.error);
  }, []);

  useEffect(refresh, [refresh]);
  useEffect(() => onEvent(EVT_SYNCED, refresh), [refresh]);

  const workspaceActive = (v: "workstreams" | "sessions" | "assistant") => {
    if (route.view === v) return "active";
    // Workstream Detail → Workstreams 保持弱高亮（§10）
    if (v === "workstreams" && route.view === "workstream") return "weak";
    if (v === "sessions" && route.view === "session") return "active";
    return "";
  };

  return (
    <div className="sidebar">
      {/* Brand → Home（§6）：Home 是产品起点，不设一级菜单项 */}
      <button className="brand" onClick={() => navigate({ view: "home" })} title="首页">
        <SidebarLogo size={22} />
        <span className="brand-name">NoEnding</span>
      </button>
      <div className="brand-tagline">对话会结束，上下文不会。</div>

      <div className="sidebar-scroll">
        <button className="nav-item" onClick={onSearch}>
          搜索
          <span style={{ flex: 1 }} />
          <span className="kbd">⌘K</span>
        </button>

        <div className="nav-section">工作区</div>
        <button className={`nav-item ${workspaceActive("workstreams")}`}
          onClick={() => navigate({ view: "workstreams" })}>
          Workstreams
        </button>
        <button className={`nav-item ${workspaceActive("sessions")}`}
          onClick={() => navigate({ view: "sessions" })}>
          Sessions
        </button>
        <IntelligenceOnly>
          <button className={`nav-item ${workspaceActive("assistant")}`}
            onClick={() => navigate({ view: "assistant" })}>
            Assistant
          </button>
        </IntelligenceOnly>

        <div className="nav-section">最近</div>
        {recent.length === 0 && <div className="nav-item muted small">暂无</div>}
        {recent.map((w) => (
          <button key={w.id}
            className={`nav-item ${route.view === "workstream" && route.workstreamId === w.id ? "active" : ""}`}
            title={w.title}
            onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 13 }}>
              {w.title}
            </span>
          </button>
        ))}

        {/* v0.2：Project 由工作目录派生，侧栏没有创建入口（方案 §22、§42.3-M22）。 */}
        <div className="nav-section">Projects</div>
        {projects.length === 0 && (
          <div className="nav-item muted small">打开 Session 或选目录后自动出现</div>
        )}
        {projects.map((p) => (
          <button key={p.id}
            className={`nav-item ${route.view === "project" && route.projectId === p.id ? "active" : ""}`}
            title={p.name}
            onClick={() => navigate({ view: "project", projectId: p.id })}>
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{p.name}</span>
          </button>
        ))}
      </div>

      <div className="sidebar-footer">
        <button
          className={`nav-item ${route.view === "settings" ? "active" : ""}`}
          onClick={() => navigate({ view: "settings", section: "general" })}
        >
          设置
        </button>
      </div>

    </div>
  );
}
