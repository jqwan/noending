import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { Route, ViewAction } from "../../app/routes";
import BoardLoading from "../../components/BoardLoading";
import BoardToolbar from "../../components/BoardToolbar";
import { timeAgo, useRefreshSignal } from "../../components/common";
import EmptyState from "../../components/EmptyState";
import Icon, { type IconName } from "../../components/Icon";
import Notice from "../../components/Notice";
import { useViewScroll } from "../../hooks/useViewState";
import PageHeader from "../../layout/PageHeader";
import type { AssistantCard } from "../../types";
import { assistantCardsCache, clearAssistantCardsCache } from "./assistantsCache";
import CreateAssistantModal from "./CreateAssistantModal";
import "./assistants.css";

export default function AssistantsView({
  action,
  scope = "unarchived",
  navigate,
  actionSeq,
}: {
  action?: ViewAction;
  scope?: "unarchived" | "archived" | "all";
  navigate: (route: Route) => void;
  actionSeq?: number;
}) {
  const [cards, setCards] = useState<AssistantCard[] | null>(() => {
    if (scope === "archived") return assistantCardsCache.archived;
    if (scope === "all") return assistantCardsCache.all;
    return assistantCardsCache.unarchived;
  });
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [failure, setFailure] = useState("");
  const [createOpen, setCreateOpen] = useState(action === "new");
  const [startingId, setStartingId] = useState<string | null>(null);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);

  const scrollRef = useViewScroll("assistants.scroll", cards !== null);
  const requestSeq = useRef(0);

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedQuery(query);
    }, 200);
    return () => clearTimeout(timer);
  }, [query]);

  // 响应快捷动作（如从 Palette 导航至新建）
  const lastActionSeq = useRef(actionSeq);
  useEffect(() => {
    if (actionSeq !== undefined && actionSeq !== lastActionSeq.current) {
      lastActionSeq.current = actionSeq;
      if (action === "new") {
        setCreateOpen(true);
      }
    }
  }, [action, actionSeq]);

  const loadCards = useCallback(async () => {
    const seq = ++requestSeq.current;
    setFailure("");
    try {
      const res = await api.listAssistantProfiles({
        scope,
        query: debouncedQuery.trim() || undefined,
        limit: 50,
      });
      if (seq === requestSeq.current) {
        setCards(res.items);
        setNextCursor(res.next_cursor ?? null);
        setHasMore(!res.end && Boolean(res.next_cursor));
        if (!debouncedQuery.trim()) {
          if (scope === "archived") assistantCardsCache.archived = res.items;
          else if (scope === "all") assistantCardsCache.all = res.items;
          else assistantCardsCache.unarchived = res.items;
        }
      }
    } catch (err) {
      if (seq === requestSeq.current) {
        setFailure(String(err));
      }
    }
  }, [scope, debouncedQuery]);

  useEffect(() => {
    void loadCards();
  }, [loadCards]);

  useRefreshSignal(loadCards);

  const handleLoadMore = async () => {
    if (!nextCursor || loadingMore || !hasMore) return;
    const seq = requestSeq.current;
    setLoadingMore(true);
    try {
      const res = await api.listAssistantProfiles({
        scope,
        query: debouncedQuery.trim() || undefined,
        cursor: nextCursor,
        limit: 50,
      });
      if (seq === requestSeq.current) {
        setCards((prev) => (prev ? [...prev, ...res.items] : res.items));
        setNextCursor(res.next_cursor ?? null);
        setHasMore(!res.end && Boolean(res.next_cursor));
      }
    } catch (err) {
      if (seq === requestSeq.current) {
        setFailure(`加载更多失败：${String(err)}`);
      }
    } finally {
      if (seq === requestSeq.current) {
        setLoadingMore(false);
      }
    }
  };

  const handleStartSession = async (card: AssistantCard) => {
    if (startingId) return;
    setStartingId(card.id);
    try {
      const launch = await api.startAssistantSession(card.id);
      if (launch.terminal_id) {
        navigate({
          view: "terminal",
          terminalId: launch.terminal_id,
          initialTitle: card.name,
          initialAgent: card.agent,
        });
      }
    } catch (err) {
      setFailure(`启动会话失败：${String(err)}`);
    } finally {
      setStartingId(null);
    }
  };

  const getAvatarIcon = (token: string): IconName => {
    if (token === "agent" || token === "chat" || token === "tasks" || token === "desktop") {
      return token;
    }
    return "spark";
  };

  return (
    <div className="main board-page assistants-page">
      <PageHeader
        title="智能体"
        icon={<Icon name="spark" />}
        navigation={
          <div className="settings-seg icon-seg archive-scope" role="group" aria-label="智能体列表范围">
            <button
              className={scope === "unarchived" ? "on" : ""}
              aria-pressed={scope === "unarchived"}
              aria-label="正常使用"
              title="正常使用中的智能体"
              onClick={() => navigate({ view: "assistants", scope: "unarchived" })}
            >
              <Icon name="spark" />
            </button>
            <button
              className={scope === "archived" ? "on" : ""}
              aria-pressed={scope === "archived"}
              aria-label="已归档"
              title="已归档的智能体"
              onClick={() => navigate({ view: "assistants", scope: "archived" })}
            >
              <Icon name="archive" />
            </button>
          </div>
        }
        actions={
          <button
            className="btn ghost icon-button"
            aria-label="新建智能体"
            title="新建智能体"
            onClick={() => setCreateOpen(true)}
          >
            <Icon name="plus" />
          </button>
        }
      />

      <div className="board-scroll" ref={scrollRef}>
        <BoardToolbar
          searchLabel="搜索智能体"
          placeholder="搜索智能体…（名称或职责）"
          query={query}
          onQueryChange={setQuery}
        >
          {null}
        </BoardToolbar>

        {failure && (
          <Notice tone="error">
            {failure}
          </Notice>
        )}

        <BoardLoading loading={cards === null && !failure} viewMode="cards" />

        {cards !== null && !failure && cards.length === 0 && (
          <EmptyState
            title={query ? "未找到匹配的智能体" : scope === "archived" ? "暂无已归档智能体" : "暂无智能体"}
            hint={
              query
                ? "请尝试不同的搜索词或清除筛选"
                : scope === "archived"
                ? "归档后的角色将在此收纳保存"
                : "创建专属智能体角色，分派项目任务与管理推进"
            }
            actions={
              !query && scope !== "archived" ? (
                <button
                  className="btn primary"
                  onClick={() => setCreateOpen(true)}
                >
                  <Icon name="plus" /> 新建智能体
                </button>
              ) : undefined
            }
          />
        )}

        {cards !== null && !failure && cards.length > 0 && (
          <div className="board-grid assistant-grid">
            {cards.map((card) => {
              const isStarting = startingId === card.id;
              const isArchived = card.visibility === "archived";
              return (
                <div
                  key={card.id}
                  className={`board-card assistant-card${isArchived ? " archived" : ""}`}
                  aria-label={`智能体：${card.name}`}
                  role="button"
                  tabIndex={0}
                  onClick={() =>
                    navigate({
                      view: "assistant",
                      assistantId: card.id,
                      initialTitle: card.name,
                    })
                  }
                  onKeyDown={(e) => {
                    if (e.target !== e.currentTarget) return;
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      navigate({
                        view: "assistant",
                        assistantId: card.id,
                        initialTitle: card.name,
                      });
                    }
                  }}
                >
                  <div className="assistant-card-header">
                    <div className="assistant-avatar">
                      <Icon name={getAvatarIcon(card.avatar)} />
                    </div>
                    <div className="assistant-card-info">
                      <div className="assistant-card-title-row">
                        <span className="assistant-name">{card.name}</span>
                        {isArchived && <span className="badge small">已归档</span>}
                      </div>
                      <span className="muted small assistant-desc">
                        {card.description || "暂无职责描述"}
                      </span>
                    </div>
                  </div>

                  <div className="assistant-card-meta">
                    <span className="badge">
                      <Icon name="agent" /> {card.agent}
                    </span>
                    <span className="muted small">
                      {card.session_count} 次会话 · {card.task_count} 项负责任务
                    </span>
                  </div>

                  <div className="assistant-card-footer">
                    <span className="muted x-small">
                      {card.last_activity_at
                        ? `活跃于 ${timeAgo(card.last_activity_at)}`
                        : "尚未有会话活动"}
                    </span>
                    {!isArchived && (
                      <button
                        type="button"
                        className="btn small primary"
                        disabled={isStarting}
                        onClick={(e) => {
                          e.stopPropagation();
                          void handleStartSession(card);
                        }}
                        onKeyDown={(e) => e.stopPropagation()}
                        title="在智能体专属空间拉起会话"
                      >
                        <Icon name="chat" /> {isStarting ? "启动中…" : "开始交流"}
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {hasMore && (
          <div className="load-more-row" style={{ display: "flex", justifyContent: "center", padding: "var(--space-md) 0" }}>
            <button
              className="btn ghost"
              disabled={loadingMore}
              onClick={handleLoadMore}
            >
              {loadingMore ? "加载中…" : "加载更多"}
            </button>
          </div>
        )}
      </div>

      <CreateAssistantModal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        onCreated={(id, name) => {
          setCreateOpen(false);
          clearAssistantCardsCache();
          navigate({
            view: "assistant",
            assistantId: id,
            initialTitle: name,
          });
        }}
      />
    </div>
  );
}
