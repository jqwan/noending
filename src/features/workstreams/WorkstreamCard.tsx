import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import type { Route } from "../../app/routes";
import { AGENT_LABELS, type Agent, type WorkstreamCardData } from "../../types";

// 词表：active → 进行中，completed → 已完成。v0.2 折叠了 abandoned。
const LIFECYCLE_LABELS: Record<string, string> = {
  active: "进行中",
  completed: "已完成",
};

/** 卡片摘要：优先 Agent 的 current_state，退回用户写的描述与目标。 */
export function cardSummaryLine(card: WorkstreamCardData): string {
  return card.current_state || card.description || card.goal || "";
}

/** 检索字段必须与 placeholder 声明的一致：命中一个页面上看不见的字段，
 *  等于给用户一个无法解释的结果。 */
export function cardSearchFields(card: WorkstreamCardData): (string | null | undefined)[] {
  return [card.title, card.description, card.project_name, card.current_state, card.goal];
}

export function searchFieldHint(): string {
  return "搜索任务…（标题、描述、项目、Context 摘要）";
}

/**
 * Workstream 卡片：卡片体 → Detail；新建进入预选当前任务的新会话页面。
 * 卡片只保留新建入口，继续会话在会话页头部（Agent 图标按钮直开桌面应用）。
 */
export default function WorkstreamCard({ card, mode, navigate, defaultAgent }: {
  card: WorkstreamCardData;
  mode: "compact" | "full";
  navigate: (r: Route) => void;
  defaultAgent: Agent | null;
}) {
  const openDetail = () => navigate({ view: "workstream", workstreamId: card.id });

  const body = cardSummaryLine(card);
  const archived = card.visibility === "archived";

  return (
    <article className={`ws-card task-card ${mode}`} onClick={openDetail}>
      <header className="ws-card-head">
        {/* 单行截断（.ws-card-title）与两行 clamp（.ws-card-body）都靠 title
            把完整内容留给用户，否则长标题在窄窗口里就永久丢了。 */}
        <h3 className="ws-card-title"><button className="card-title-link" title={card.title} onClick={e => { e.stopPropagation(); openDetail(); }}>{card.title}</button></h3>
        <div className="ws-card-side">
          {(
            <span className="badge" title="任务状态">{LIFECYCLE_LABELS[card.lifecycle] ?? card.lifecycle}</span>
          )}
          {/* visibility=archived 就是回收站：它和 lifecycle 正交，
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
          <button
            className={`btn small ws-btn ${card.latest_session ? "ghost icon-button" : ""}`}
            aria-label="新建会话"
            title={defaultAgent ? `用 ${AGENT_LABELS[defaultAgent]} 新建会话` : "新建会话"}
            onClick={() => navigate({ view: "new-session", workstreamId: card.id })}
          >
            <Icon name="plus" />{!card.latest_session && "新建会话"}
          </button>
        </div>}
      </footer>
    </article>
  );
}
