import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import type { Route } from "../../app/routes";
import { AGENT_LABELS, type Agent, type WorkstreamCardData } from "../../types";

/** 卡片概览：显示任务描述（用户描述），不显示任务摘要。 */
export function cardSummaryLine(card: WorkstreamCardData): string {
  return card.description || "";
}

/**
 * Workstream 卡片：卡片体 → Detail；新建进入预选当前任务的新会话页面。
 * 卡片提供归档、取消归档和永久删除；未归档任务可新建会话。
 */
export default function WorkstreamCard({ card, mode, navigate, defaultAgent, onArchive, onRestore, onDelete, busy = false }: {
  card: WorkstreamCardData;
  mode: "compact" | "full";
  navigate: (r: Route) => void;
  defaultAgent: Agent | null;
  onArchive?: (card: WorkstreamCardData) => void;
  onRestore?: (card: WorkstreamCardData) => void;
  onDelete?: (card: WorkstreamCardData) => void;
  busy?: boolean;
}) {
  const openDetail = () => navigate({ view: "workstream", workstreamId: card.id, initialTitle: card.title });

  const body = cardSummaryLine(card);
  const archived = card.visibility === "archived";

  return (
    <article className={`ws-card task-card ${mode}`} onClick={openDetail}>
      <header className="ws-card-head">
        {/* 单行截断（.ws-card-title）与两行 clamp（.ws-card-body）都靠 title
            把完整内容留给用户，否则长标题在窄窗口里就永久丢了。 */}
        <h3 className="ws-card-title">
          <button className="card-title-link" title={card.title} onClick={e => { e.stopPropagation(); openDetail(); }}>
            <Icon name="tasks" />
            <span>{card.title}</span>
          </button>
        </h3>
        <div className="ws-card-side">
          {archived && <span className="badge">已归档</span>}
        </div>
      </header>

      <div className="ws-card-body">
        <div className="board-stats">
          {card.projects.length > 0 && (
            <span
              className="task-project"
              title={card.projects.map((p) => p.name).join("、")}
            >
              <Icon name="project" />
              {card.projects.length}<span>个项目</span>
              <span className="sr-only">{card.projects.length} 个项目</span>
            </span>
          )}
          <span title="会话">
            <Icon name="chat" />
            {card.session_count}<span>会话</span>
          </span>
        </div>
        {body && <p className="ws-card-desc" title={body}>{body}</p>}
      </div>

      <footer className="ws-card-meta">
        <span className="muted small">
          {timeAgo(card.last_activity_at ?? card.updated_at)}
        </span>
        <div className="ws-card-actions" onClick={(e) => e.stopPropagation()}>
          {!archived && card.projects.length > 0 && <button
            className="btn small ws-btn ghost icon-button"
            aria-label="新建会话"
            title={defaultAgent ? `用 ${AGENT_LABELS[defaultAgent]} 新建会话` : "新建会话"}
            onClick={() => navigate({ view: "new-session", workstreamId: card.id })}
          >
            <Icon name="plus" />
          </button>}
          <button className="btn small ghost icon-button" disabled={busy}
            aria-label={archived ? `取消归档${card.title}` : `归档${card.title}`}
            title={archived ? "取消归档" : "归档"}
            onClick={() => archived ? onRestore?.(card) : onArchive?.(card)}>
            <Icon name={archived ? "unarchive" : "archive"} />
          </button>
          {archived && <button className="btn small ghost icon-button danger" disabled={busy}
            aria-label={`永久删除${card.title}`} title="永久删除" onClick={() => onDelete?.(card)}>
            <Icon name="trash" />
          </button>}
        </div>
      </footer>
    </article>
  );
}
