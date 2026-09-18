import React, { useState } from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import type { Route } from "../../app/routes";
import NewSessionModal from "../sessions/NewSessionModal";
import ResumeSessionModal from "../sessions/ResumeSessionModal";
import { AGENT_LABELS, type Agent, type WorkstreamCardData } from "../../types";

// §2.1 / §2.2 词表：open → 进行中，completed → 已完成，abandoned → 已放弃。
const LIFECYCLE_LABELS: Record<string, string> = {
  open: "进行中",
  completed: "已完成",
  abandoned: "已放弃",
};

/**
 * The one Workstream card shared by Home (compact) and Workstreams (full).
 * Behavior is identical in both modes:
 *   Body   → Workstream Detail
 *   新建 Session → 挂载全局 NewSessionModal（预置本 Workstream，可再改）
 *   继续        → 挂载全局 ResumeSessionModal
 *
 * 卡片自己不再调用 launcher：启动路径唯一
 * NewSessionModal → prepareNewSession → launchPrepared（§8.1.1、Preview-Launch
 * Identity）。两个 Modal 渲染在 `<article>` 之外，否则卡片整体的
 * 「点击进详情」会吃掉弹窗里的点击。
 */
export default function WorkstreamCard({ card, mode, navigate, defaultAgent }: {
  card: WorkstreamCardData;
  mode: "compact" | "full";
  navigate: (r: Route) => void;
  defaultAgent: Agent | null;
}) {
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [resumeOpen, setResumeOpen] = useState(false);

  const openDetail = () => navigate({ view: "workstream", workstreamId: card.id });

  // Current State → Description → Goal（§4 内容优先级，两行截断）
  const body = card.current_state || card.description || card.goal || "";

  return (
    <>
      <article className={`ws-card ${mode}`} onClick={openDetail}>
        <header className="ws-card-head">
          <h3 className="ws-card-title">{card.title}</h3>
          <div className="ws-card-side">
            {card.lifecycle !== "open" && (
              <span className="badge">{LIFECYCLE_LABELS[card.lifecycle] ?? card.lifecycle}</span>
            )}
            {/* Project 是可选组织层：未归属时不显示任何占位 */}
            {card.project_name && <span className="ws-card-project">{card.project_name}</span>}
          </div>
        </header>

        {body ? (
          <p className="ws-card-body">{body}</p>
        ) : (
          <p className="ws-card-body muted">还没有描述 — 在详情页补充。</p>
        )}

        <footer className="ws-card-meta">
          <span>
            {card.session_count === 0
              ? "还没有 Session"
              : `最近活动 ${timeAgo(card.last_activity_at)} · ${card.session_count} 个 Session`}
          </span>
          <div className="ws-card-actions" onClick={(e) => e.stopPropagation()}>
            {defaultAgent ? (
              <button
                className="btn small ws-btn"
                title={`用 ${AGENT_LABELS[defaultAgent]} 新建 Session`}
                onClick={() => setNewSessionOpen(true)}
              >
                <AgentIcon agent={defaultAgent} />
                新建 Session
              </button>
            ) : (
              <button
                className="btn small ws-btn"
                disabled
                title="未检测到可用的 Agent CLI — 到 设置 → Agent 配置"
              >
                新建 Session
              </button>
            )}
            {!defaultAgent && (
              <button
                className="link small"
                onClick={() => navigate({ view: "settings", section: "agents" })}
              >
                配置
              </button>
            )}
            {card.latest_session && (
              <button
                className={`btn small ws-btn ${mode === "compact" ? "resume-primary" : ""}`}
                title={`继续最近的 ${AGENT_LABELS[card.latest_session.agent]} Session`}
                onClick={() => setResumeOpen(true)}
              >
                <AgentIcon agent={card.latest_session.agent} />
                继续
              </button>
            )}
          </div>
        </footer>
      </article>

      {newSessionOpen && (
        <NewSessionModal workstreamId={card.id} onClose={() => setNewSessionOpen(false)} />
      )}
      {resumeOpen && card.latest_session && (
        <ResumeSessionModal
          sessionId={card.latest_session.id}
          onClose={() => setResumeOpen(false)}
        />
      )}
    </>
  );
}
