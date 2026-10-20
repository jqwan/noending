import type { Route } from "../app/routes";
import Icon from "../components/Icon";
import { AGENT_LABELS } from "../types";
import { useAgentStatus } from "../features/sessions/continueDesktop";
import { useSidebarData } from "./useSidebarData";
import SidebarSessionRow from "./SidebarSessionRow";
import SidebarGroupHeader from "./SidebarGroupHeader";
import SidebarGroupMenu from "./SidebarGroupMenu";
import SidebarFilterMenu from "./SidebarFilterMenu";
import { MAX_GROUP_SESSIONS_DEFAULT } from "./sidebarModel";

export * from "./sidebarModel";

/**
 * 会话侧边栏：新建、会话看板、最近会话与保留终端的统一列表。
 * 工作区导航由最左侧固定图标栏提供；切换工作区时保留本栏的数据与分组状态。
 */
export default function Sidebar({
  route,
  navigate,
  collapsed = false,
  hidden = false,
}: {
  route: Route;
  navigate: (r: Route) => void;
  collapsed?: boolean;
  hidden?: boolean;
}) {
  const agentStatus = useAgentStatus();
  const {
    terminalsLoading,
    groupBy,
    setGroupBy,
    collapsedGroups,
    expandedGroupSessions,
    setExpandedGroupSessions,
    flatVisibleCount,
    setFlatVisibleCount,
    filters,
    toggleFilter,
    clearFilters,
    runningOnly,
    setRunningOnly,
    showSessionIcons,
    setShowSessionIcons,
    sortBy,
    setSortBy,
    pinnedCollapsed,
    setPinnedCollapsed,
    isSessionPinned,
    togglePin,
    pinnedSessions,
    resumeStates,
    sessionGroups,
    allGroupsCollapsed,
    toggleGroup,
    toggleAllGroups,
  } = useSidebarData({ collapsed, hidden });

  const collapseKey = (groupId: string) => `${groupBy}:${groupId}`;

  return (
    <aside className={`sidebar${collapsed ? " collapsed" : ""}`} hidden={collapsed || hidden} aria-label="会话侧边栏">
      <div className="sidebar-top">
        <button
          type="button"
          className={`sidebar-top-btn ${route.view === "new-session" ? "active" : ""}`}
          onClick={() => navigate({ view: "new-session" })}
        >
          <span className="sidebar-header-icon">
            <Icon name="chatPlus" />
          </span>
          <span className="sidebar-header-title">新建</span>
        </button>
      </div>
      <div className="sidebar-scroll">
        {pinnedSessions.length > 0 && (
          <div className="sidebar-pinned-section">
            <button
              type="button"
              className={`sidebar-recent-group-header sidebar-pinned-header ${pinnedCollapsed ? "collapsed" : ""}`}
              onClick={() => setPinnedCollapsed((prev) => !prev)}
              aria-expanded={!pinnedCollapsed}
              title={`置顶 (${pinnedSessions.length})`}
            >
              <span className="sidebar-header-icon pinned-group-icon">
                <Icon name="pin" />
              </span>
              <span className="sidebar-header-title truncate">置顶</span>
              <span className="sidebar-recent-count">{pinnedSessions.length}</span>
            </button>
            {!pinnedCollapsed && (
              <div className="sidebar-recent-group-items">
                {pinnedSessions.map((s) => (
                  <SidebarSessionRow
                    key={`pinned:${s.id}`}
                    session={s}
                    resumeState={resumeStates[s.sessionId ?? ""]}
                    route={route}
                    navigate={navigate}
                    agentStatus={agentStatus}
                    loading={terminalsLoading}
                    isPinned={true}
                    onTogglePin={() => togglePin(s.id, s.sessionId)}
                    showIcon={showSessionIcons}
                  />
                ))}
              </div>
            )}
          </div>
        )}

        <div className="nav-section sidebar-section-header">
          <SidebarGroupMenu groupBy={groupBy} onSelectGroupBy={setGroupBy} />

          <div className="sidebar-header-actions">
            <button
              type="button"
              className={`sidebar-filter-trigger sidebar-running-filter${runningOnly ? " active" : ""}`}
              onClick={() => setRunningOnly((prev) => !prev)}
              aria-label="仅显示运行中会话"
              aria-pressed={runningOnly}
              title={runningOnly ? "关闭运行中筛选（不影响置顶区）" : "仅显示有运行中终端的会话（不影响置顶区）"}
            >
              <Icon name="terminal" />
            </button>

            <button
              type="button"
              className="sidebar-collapse-all-btn"
              onClick={toggleAllGroups}
              disabled={groupBy === "none" || sessionGroups.length === 0}
              aria-label={allGroupsCollapsed ? "展开全部" : "收起全部"}
              title={groupBy === "none" ? "未分组" : allGroupsCollapsed ? "展开全部" : "收起全部"}
            >
              <Icon name={allGroupsCollapsed ? "expandAll" : "collapseAll"} />
            </button>

            <SidebarFilterMenu
              filters={filters}
              onToggleFilter={toggleFilter}
              onClearFilters={clearFilters}
              sortBy={sortBy}
              onSelectSortBy={setSortBy}
              showSessionIcons={showSessionIcons}
              onToggleShowSessionIcons={() => setShowSessionIcons((prev) => !prev)}
            />
          </div>
        </div>

        {sessionGroups.length === 0 ? (
          <div className="sidebar-recent-empty">
            {runningOnly ? "暂无运行中会话" : filters.length > 0 ? "无匹配会话" : pinnedSessions.length > 0 ? "暂无未置顶会话" : "暂无会话"}
          </div>
        ) : groupBy === "none" ? (
          <div className="sidebar-flat-session-items">
            {sessionGroups[0].sessions.slice(0, flatVisibleCount).map((s) => (
              <SidebarSessionRow
                key={s.id}
                session={s}
                resumeState={resumeStates[s.sessionId ?? ""]}
                route={route}
                navigate={navigate}
                agentStatus={agentStatus}
                loading={terminalsLoading}
                isPinned={isSessionPinned(s)}
                onTogglePin={() => togglePin(s.id, s.sessionId)}
                showIcon={showSessionIcons}
              />
            ))}
            {sessionGroups[0].sessions.length > flatVisibleCount && (
              <button className="btn small ghost" onClick={() => setFlatVisibleCount((count) => count + 100)}>
                显示更多（剩余 {sessionGroups[0].sessions.length - flatVisibleCount}）
              </button>
            )}
          </div>
        ) : (
          sessionGroups.map((group) => {
            const isCollapsed = Boolean(collapsedGroups[collapseKey(group.id)]);
            const isUnassignedGroup = group.id === "__unassigned_project__";
            const isKnownAgent = groupBy === "agent" && group.id in AGENT_LABELS;
            const iconName = isUnassignedGroup
              ? isCollapsed
                ? "chevronRight"
                : "chevronDown"
              : groupBy === "project"
              ? isCollapsed
                ? "project"
                : "projectOpen"
              : "agent";

            return (
              <div className="sidebar-recent-group" key={group.id}>
                <SidebarGroupHeader
                  group={group}
                  isCollapsed={isCollapsed}
                  isUnassignedGroup={isUnassignedGroup}
                  isKnownAgent={isKnownAgent}
                  iconName={iconName}
                  onToggle={() => toggleGroup(group.id)}
                />
                {!isCollapsed && (() => {
                  const isExpanded = Boolean(expandedGroupSessions[group.id]);
                  const visibleSessions = isExpanded
                    ? group.sessions
                    : group.sessions.slice(0, MAX_GROUP_SESSIONS_DEFAULT);
                  const hasMore = group.sessions.length > MAX_GROUP_SESSIONS_DEFAULT;

                  return (
                    <div className={`sidebar-recent-group-items${showSessionIcons ? " has-icons" : ""}`}>
                      {visibleSessions.map((s) => (
                        <SidebarSessionRow
                          key={s.id}
                          session={s}
                          resumeState={resumeStates[s.sessionId ?? ""]}
                          route={route}
                          navigate={navigate}
                          agentStatus={agentStatus}
                          loading={terminalsLoading}
                          isPinned={isSessionPinned(s)}
                          onTogglePin={() => togglePin(s.id, s.sessionId)}
                          showIcon={showSessionIcons}
                        />
                      ))}
                      {hasMore && !isExpanded && (
                        <button
                          type="button"
                          className="sidebar-group-expand-btn"
                          onClick={() =>
                            setExpandedGroupSessions((prev) => ({
                              ...prev,
                              [group.id]: true,
                            }))
                          }
                        >
                          展开显示
                        </button>
                      )}
                    </div>
                  );
                })()}
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
}
