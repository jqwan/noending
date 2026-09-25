import Icon from "../../components/Icon";
import { useState } from "react";
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
    ? "搜索任务…（标题、描述、项目、Context 摘要）"
    : "搜索任务…（标题、描述、项目）";
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
  const archived = card.visibility === "archived";

  return (
    <>
      <article className={`ws-card task-card ${mode}`} onClick={openDetail}>
        <header className="ws-card-head">
          {/* 单行截断（.ws-card-title）与两行 clamp（.ws-card-body）都靠 title
              把完整内容留给用户，否则长标题在窄窗口里就永久丢了。 */}
          <h3 className="ws-card-title"><button className="card-title-link" title={card.title} onClick={e => { e.stopPropagation(); openDetail(); }}>{card.title}</button></h3>
          <div className="ws-card-side">
            {(
              <span className="badge" title="任务状态">{LIFECYCLE_LABELS[card.lifecycle] ?? card.lifecycle}</span>
            )}
            {/* visibility=archived 就是回收站（方案 §1.13）：它和 lifecycle 正交，
                所以这里单独一个徽标，而不是把 lifecycle 改成第三种值。 */}
            {card.visibility === "archived" && (
              <span className="badge warn" title="在回收站里：工作路径、会话归属与 Context 都原样保留。进详情页可以恢复或永久删除。">回收站</span>
            )}
          </div>
        </header>

        {card.project_name && <div className="task-project" title={card.project_name}><Icon name="folder" /><span className="truncate">{card.project_name}</span></div>}
        {body && <p className="ws-card-body" title={body}>{body}</p>}

        <footer className="ws-card-meta">
          <span>
            {card.session_count === 0
              ? "还没有会话"
              : `${card.session_count} 个会话 · ${timeAgo(card.last_activity_at)}`}
          </span>
          {!archived && <div className="ws-card-actions" onClick={(e) => e.stopPropagation()}>
            {/* 与 Home 同一个判断：卡片不自己宣称「没有 Agent」。`defaultAgent`
                由 useWorkstreamCards 异步解析（返回形状按 §8.1.1 冻结，没有
                "解析完成"这一位），所以在解析期间 disabled + 「未检测到」的
                tooltip 会说假话。是否真的没有 Agent 一律交给 NewSessionModal
                自己判定——它同时是唯一的启动路径。 */}
            <button
              className={`btn small ws-btn ${card.latest_session ? "ghost icon-button" : ""}`}
              aria-label="新建会话"
              title={defaultAgent ? `用 ${AGENT_LABELS[defaultAgent]} 新建会话` : "新建会话"}
              onClick={() => setNewSessionOpen(true)}
            >
              <Icon name="plus" />{!card.latest_session && "新建会话"}
            </button>
            {card.latest_session && (
              <button
                className="btn small ws-btn resume-primary"
                title={`继续最近的 ${AGENT_LABELS[card.latest_session.agent]} 会话`}
                onClick={() => setResumeOpen(true)}
              >
                <AgentIcon agent={card.latest_session.agent} />
                继续
              </button>
            )}
          </div>}
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
