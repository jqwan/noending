import React, { useState } from "react";
import { timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import type { Route } from "../../app/routes";
import { useBaseExperience } from "../../app/experience";
import NewSessionModal from "../sessions/NewSessionModal";
import ResumeSessionModal from "../sessions/ResumeSessionModal";
import { AGENT_LABELS, type Agent, type WorkstreamCardData } from "../../types";

// 词表：active → 进行中，completed → 已完成。v0.2 折叠了 abandoned。
const LIFECYCLE_LABELS: Record<string, string> = {
  active: "进行中",
  completed: "已完成",
};

/**
 * Base Experience 下，Workstream 由用户显式组织的信息驱动（§14 的定义），
 * 不显示冻结期留下的 Agent 摘要（`current_state` / `goal`）。智能重新开启时
 * 摘要能力原样回来，所以这里是门控而不是删除。
 */
export function cardSummaryLine(card: WorkstreamCardData, intelligenceEnabled: boolean): string {
  return intelligenceEnabled
    ? card.current_state || card.description || card.goal || ""
    : card.description || "";
}

/**
 * 检索字段必须与 placeholder 声明的一致：命中一个页面上根本看不见的字段，
 * 等于给用户一个无法解释的结果（搜到了、却看不到命中的是什么）。
 */
export function cardSearchFields(
  card: WorkstreamCardData,
  intelligenceEnabled: boolean,
): (string | null | undefined)[] {
  const userFields = [card.title, card.description, card.project_name];
  return intelligenceEnabled
    ? [...userFields, card.current_state, card.goal]
    : userFields;
}

export function searchFieldHint(intelligenceEnabled: boolean): string {
  return intelligenceEnabled
    ? "搜索 Workstream…（标题、描述、Project、Context 摘要）"
    : "搜索 Workstream…（标题、描述、Project）";
}

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

  const { intelligenceEnabled } = useBaseExperience();
  const body = cardSummaryLine(card, intelligenceEnabled);

  return (
    <>
      <article className={`ws-card ${mode}`} onClick={openDetail}>
        <header className="ws-card-head">
          {/* 单行截断（.ws-card-title）与两行 clamp（.ws-card-body）都靠 title
              把完整内容留给用户，否则长标题在窄窗口里就永久丢了。 */}
          <h3 className="ws-card-title" title={card.title}>{card.title}</h3>
          <div className="ws-card-side">
            {card.lifecycle !== "active" && (
              <span className="badge" title="只是分类标签，不改变任何行为">{LIFECYCLE_LABELS[card.lifecycle] ?? card.lifecycle}</span>
            )}
            {/* visibility=archived 就是回收站（方案 §1.13）：它和 lifecycle 正交，
                所以这里单独一个徽标，而不是把 lifecycle 改成第三种值。 */}
            {card.visibility === "archived" && (
              <span className="badge warn" title="在回收站里：工作路径、Session 绑定与 Context 都原样保留。进详情页可以恢复或永久删除。">回收站</span>
            )}
            {/* Project 是主工作路径的派生投影（方案 §42.3-M19），不是用户挑的组织层：
                没有路径就没有 Project，此时不显示任何占位。
                .ws-card-side 是 flex:none，长 Project 名会把标题挤没，
                所以这里就地限宽并把全名留在 title 上。 */}
            {card.project_name && (
              <span
                className="ws-card-project"
                title={`由主工作路径派生的 Project（只读）：${card.project_name}`}
                style={{ maxWidth: 130, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
              >
                {card.project_name}
              </span>
            )}
          </div>
        </header>

        {body ? (
          <p className="ws-card-body" title={body}>{body}</p>
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
            {/* 与 Home 同一个判断：卡片不自己宣称「没有 Agent」。`defaultAgent`
                由 useWorkstreamCards 异步解析（返回形状按 §8.1.1 冻结，没有
                "解析完成"这一位），所以在解析期间 disabled + 「未检测到」的
                tooltip 会说假话。是否真的没有 Agent 一律交给 NewSessionModal
                自己判定——它同时是唯一的启动路径。 */}
            <button
              className="btn small ws-btn"
              title={defaultAgent ? `用 ${AGENT_LABELS[defaultAgent]} 新建 Session` : "新建 Session"}
              onClick={() => setNewSessionOpen(true)}
            >
              {defaultAgent ? <AgentIcon agent={defaultAgent} /> : null}
              新建 Session
            </button>
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
