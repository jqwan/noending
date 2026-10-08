import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import type { Route } from "../../app/routes";
import { AGENT_LABELS, type Agent, type WorkstreamCardData } from "../../types";

/** 卡片摘要：优先 Agent 的 current_state，退回用户写的描述与目标。 */
export function cardSummaryLine(card: WorkstreamCardData): string {
  return card.current_state || card.description || card.goal || "";
}

/** 检索字段必须与 placeholder 声明的一致：命中一个页面上看不见的字段，
 *  等于给用户一个无法解释的结果。 */
export function cardSearchFields(card: WorkstreamCardData): (string | null | undefined)[] {
  return [card.title, card.description, ...card.projects.map((p) => p.name), card.current_state, card.goal];
}

export function searchFieldHint(): string {
  return "搜索任务…（标题、描述、项目、Context 摘要）";
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
          {archived && <span className="badge">已归档</span>}
        </div>
      </header>

      {card.projects.map((p) => <div key={p.id} className="task-project" title={p.name}><Icon name="folder" /><span className="truncate">{p.name}</span></div>)}
      {body && <p className="ws-card-body" title={body}>{body}</p>}

      <footer className="ws-card-meta">
        <span>
          {card.session_count === 0
            ? "还没有会话"
            : `${card.session_count} 个会话 · ${timeAgo(card.last_activity_at)}`}
        </span>
        <div className="ws-card-actions" onClick={(e) => e.stopPropagation()}>
          {!archived && <button
            className={`btn small ws-btn ${card.latest_session ? "ghost icon-button" : ""}`}
            aria-label="新建会话"
            title={defaultAgent ? `用 ${AGENT_LABELS[defaultAgent]} 新建会话` : "新建会话"}
            onClick={() => navigate({ view: "new-session", workstreamId: card.id })}
          >
            <Icon name="plus" />{!card.latest_session && "新建会话"}
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
