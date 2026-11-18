import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import type { Route, ViewAction } from "../../app/routes";
import BoardToolbar from "../../components/BoardToolbar";
import { timeAgo } from "../../components/common";
import CustomSelect from "../../components/CustomSelect";
import EmptyState from "../../components/EmptyState";
import Icon, { type IconName } from "../../components/Icon";
import Notice from "../../components/Notice";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";
import { useViewScroll } from "../../hooks/useViewState";
import PageHeader from "../../layout/PageHeader";
import { AGENT_LABELS, type Agent, type AgentStatusEntry, type AssistantCard } from "../../types";
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
  const [agentFilter, setAgentFilter] = useState("all");
  const [loading, setLoading] = useState(cards === null);
  const [failure, setFailure] = useState("");
  const [createOpen, setCreateOpen] = useState(action === "new");
  const [startingId, setStartingId] = useState<string | null>(null);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [agentStatus, setAgentStatus] = useState<Record<Agent, AgentStatusEntry> | null>(null);

  const showSkeleton = useLoadingDelay(loading && cards === null);
  const scrollRef = useViewScroll("assistants.scroll", cards !== null);
  const requestSeq = useRef(0);

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedQuery(query);
    }, 200);
    return () => clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    let cancelled = false;
    api.getAgentStatus().then((status) => {
      if (!cancelled && status) setAgentStatus(status);
    }).catch(console.error);
    return () => { cancelled = true; };
  }, []);

  const agentFilterOptions = useMemo(() => {
    const allAgents = Object.keys(AGENT_LABELS) as Agent[];
    const cliAgents = agentStatus
      ? allAgents.filter((a) => Boolean(agentStatus[a]?.terminal_cli))
      : [];
    return [
      { value: "all", label: "全部运行工具" },
      ...cliAgents.map((a) => ({
        value: a,
        label: AGENT_LABELS[a] ?? a,
      })),
    ];
  }, [agentStatus]);

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
    setLoading(true);
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
        if (!debouncedQuery.trim() && agentFilter === "all") {
          if (scope === "archived") assistantCardsCache.archived = res.items;
          else if (scope === "all") assistantCardsCache.all = res.items;
          else assistantCardsCache.unarchived = res.items;
        }
      }
    } catch (err) {
      if (seq === requestSeq.current) {
        setFailure(String(err));
      }
    } finally {
      if (seq === requestSeq.current) {
        setLoading(false);
      }
    }
  }, [scope, debouncedQuery, agentFilter]);

  useEffect(() => {
    void loadCards();
  }, [loadCards]);

  const handleLoadMore = async () => {
    if (!nextCursor || loadingMore || !hasMore) return;
    setLoadingMore(true);
    try {
      const res = await api.listAssistantProfiles({
        scope,
        query: query.trim() || undefined,
        cursor: nextCursor,
        limit: 50,
      });
      setCards((prev) => (prev ? [...prev, ...res.items] : res.items));
      setNextCursor(res.next_cursor ?? null);
      setHasMore(!res.end && Boolean(res.next_cursor));
    } catch (err) {
      setFailure(`加载更多失败：${String(err)}`);
    } finally {
      setLoadingMore(false);
    }
  };

  const filteredCards = useMemo(() => {
    if (!cards) return [];
    if (agentFilter === "all") return cards;
    return cards.filter((card) => card.agent === agentFilter);
  }, [cards, agentFilter]);

  const handleStartSession = async (card: AssistantCard, e: React.MouseEvent) => {
    e.stopPropagation();
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
          <div className="toolbar ws-controls">
            <div className="ws-control">
              <span className="muted small">运行工具</span>
              <CustomSelect
                value={agentFilter}
                onChange={setAgentFilter}
                ariaLabel="选择运行工具筛选"
                options={agentFilterOptions}
              />
            </div>
          </div>
        </BoardToolbar>

        {failure && (
          <Notice tone="error">
            {failure}
          </Notice>
        )}

        {showSkeleton && (
          <div className="board-grid assistant-grid">
            <div className="board-card skeleton" />
            <div className="board-card skeleton" />
            <div className="board-card skeleton" />
          </div>
        )}

        {!showSkeleton && filteredCards.length === 0 && (
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
                  className="btn solid"
                  onClick={() => setCreateOpen(true)}
                >
                  <Icon name="plus" /> 新建智能体
                </button>
              ) : undefined
            }
          />
        )}

        {!showSkeleton && filteredCards.length > 0 && (
          <div className="board-grid assistant-grid">
            {filteredCards.map((card) => {
              const isStarting = startingId === card.id;
              const isArchived = card.visibility === "archived";
              return (
                <div
                  key={card.id}
                  className={`board-card assistant-card${isArchived ? " archived" : ""}`}
                  aria-label={`智能体：${card.name}`}
                  onClick={() =>
                    navigate({
                      view: "assistant",
                      assistantId: card.id,
                      initialTitle: card.name,
                    })
                  }
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
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
                        {isArchived && <span className="tag small">已归档</span>}
                      </div>
                      <span className="muted small assistant-desc">
                        {card.description || "暂无职责描述"}
                      </span>
                    </div>
                  </div>

                  <div className="assistant-card-meta">
                    <span className="badge-agent">
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
                        className="btn small solid"
                        disabled={isStarting}
                        onClick={(e) => handleStartSession(card, e)}
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
