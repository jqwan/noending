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
  const [creatingProject, setCreatingProject] = useState(false);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [projectBusy, setProjectBusy] = useState(false);
  const [projectError, setProjectError] = useState("");

  const refresh = useCallback(() => {
    api.listProjects().then(setProjects).catch(console.error);
    api
      .listWorkstreamCards()
      .then((cards) =>
        setRecent(
          cards
            .filter((c) => c.lifecycle === "open" && c.visibility === "normal")
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

  const createProject = async () => {
    if (!name.trim() || projectBusy) return;
    setProjectBusy(true);
    setProjectError("");
    try {
      await api.createProject(name, desc);
      setCreatingProject(false);
      setName("");
      setDesc("");
      refresh();
    } catch (e) {
      // 失败时保留弹窗和已输入的内容，把原因写在脸上；静默返回会让用户
      // 以为这个 Project 已经建好了（§25 错误状态）。
      console.error(e);
      setProjectError(String(e));
    } finally {
      setProjectBusy(false);
    }
  };

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

        <div className="nav-section">
          Projects
          <button className="nav-section-add" title="新建 Project" onClick={() => setCreatingProject(true)}>+</button>
        </div>
        {projects.length === 0 && <div className="nav-item muted small">尚未创建</div>}
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

      {creatingProject && (
        <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setCreatingProject(false)}>
          <div className="modal">
            <h2>新建 Project</h2>
            <p className="muted small" style={{ marginTop: -6 }}>
              Project 只是可选的组织层，Workstream 可以独立存在。
            </p>
            <label className="field"><span>名称</span>
              <input type="text" value={name} onChange={(e) => setName(e.target.value)} autoFocus
                placeholder="例如：Agent Workspace / Japan Trip" /></label>
            <label className="field"><span>描述（可选）</span>
              <textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
            {projectError && (
              <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>
                {projectError}
              </div>
            )}
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button className="btn" onClick={() => setCreatingProject(false)} disabled={projectBusy}>取消</button>
              <button className="btn primary" disabled={projectBusy || !name.trim()} onClick={createProject}>
                {projectBusy ? "创建中…" : "创建"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
