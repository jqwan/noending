import { useState } from "react";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import PageHeader from "../../layout/PageHeader";
import SidebarLogo from "../../components/SidebarLogo";
import type { Route } from "../../app/routes";
import { ContinueSection, RecentSessions } from "./ContinueSection";
import { useWorkstreamCards } from "../workstreams/useWorkstreamCards";
import WorkstreamFormModal from "../workstreams/WorkstreamFormModal";
import NewSessionModal from "../sessions/NewSessionModal";

/**
 * Home = Continue where you left off（整体设计方案 §12）。
 * 唯一任务：让用户用最短路径回到最近推进的 Workstream。
 *
 * Base Experience（方案 §11.5）：Home 不消费 reviewSummaries，也不挂载任何智能
 * 段落（Context Updates / Review / Conflict 一律不出现在主路径，§24）。这里的
 * 「新建 Session」不自己启动进程，而是打开全局唯一的启动路径
 * NewSessionModal → prepareNewSession → launchPrepared（§8.1.1、Preview-Launch
 * Identity）。Home 也从不推进 ReviewState（Home Attention Integrity）。
 */
export default function HomeView({ navigate }: { navigate: (r: Route) => void }) {
  const { cards, defaultAgent, refresh, loadError } = useWorkstreamCards();
  const [creatingWs, setCreatingWs] = useState(false);
  const [creatingSession, setCreatingSession] = useState(false);

  if (cards === null) return <div className="main narrow" role="status">{loadError ? <>加载失败 <button className="btn small" onClick={refresh}>重试</button></> : "加载中…"}</div>;

  const activeCards = cards.filter(
    (c) => c.lifecycle === "active" && c.visibility === "normal",
  );
  // 卡片一个都不剩：和「真的什么都没有」是两种边界，得告诉用户东西去哪了。
  // 成因有两种，不能混着说——归档是进了回收站，完成只是不再活跃。
  const everythingHidden = activeCards.length === 0 && cards.length > 0;
  const archivedCount = cards.filter((c) => c.visibility === "archived").length;
  const hiddenReason =
    archivedCount === 0
      ? "已完成的任务不会出现在首页。"
      : cards.length - archivedCount === 0
      ? "已归档的任务不会出现在首页。"
        : "已归档或已完成的任务不会出现在首页。";

  const modals = (
    <>
      {creatingWs && (
        <WorkstreamFormModal onClose={() => setCreatingWs(false)} onCreated={refresh} />
      )}
      {creatingSession && (
        <NewSessionModal onClose={() => setCreatingSession(false)} />
      )}
    </>
  );

  if (activeCards.length === 0) {
    return (
      <div className="main narrow home">
        <div className="hero">
          <SidebarLogo size={40} />
          <h1>开始新任务</h1>
          <div className="actions-row">
            <button className="btn primary" onClick={() => setCreatingWs(true)}>+ 新建任务</button>
          </div>
          <div className="muted small" style={{ margin: "10px 0" }}>或</div>
          <div className="actions-row">
            {/* 打开的是全局新建 Session 面板：Agent 未检测到时由面板给出
                「未检测到可用的 Agent CLI」与不可点的启动按钮，这里不再自己
                判断（避免在 Agent 还在解析时把按钮误标成「未检测」）。
                也正因为如此，这里不再额外挂一个 `!defaultAgent` 的「配置 Agent」
                链接：defaultAgent 从 hook 里异步解析，链接会在解析期间闪一下，
                而它承诺的「没有 Agent」那时还只是未知（§25 状态必须是真的）。 */}
            <button className="btn ws-btn" onClick={() => setCreatingSession(true)}>
              {defaultAgent ? (
                <>
                  <AgentIcon agent={defaultAgent} />
                  新建会话
                </>
              ) : (
                "新建会话"
              )}
            </button>
          </div>
          {everythingHidden && (
            <p className="muted small" style={{ marginTop: 18 }}>
              {hiddenReason}{" "}
              <button className="link small"
                onClick={() => navigate({ view: "workstreams" })}>
                任务 →
              </button>
            </p>
          )}
        </div>

        {modals}
      </div>
    );
  }

  return (
    <div className="main narrow">
      <PageHeader
        title="继续工作"
        actions={
          <button className="btn ghost icon-button" aria-label="新建任务" title="新建任务" onClick={() => setCreatingWs(true)}>
            <Icon name="plus" />
          </button>
        }
      />

      <ContinueSection navigate={navigate} defaultAgent={defaultAgent} cards={cards} />
      <RecentSessions navigate={navigate} onNewSession={() => setCreatingSession(true)} />



      {modals}
    </div>
  );
}
