import React, { useState } from "react";
import AgentIcon from "../../components/AgentIcon";
import SidebarLogo from "../../components/SidebarLogo";
import type { Route } from "../../app/routes";
import { ContinueSection, RecentSessions } from "./ContinueSection";
import { useWorkstreamCards } from "../workstreams/useWorkstreamCards";
import NewWorkstreamModal from "../workstreams/NewWorkstreamModal";
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
  const { cards, defaultAgent, refresh } = useWorkstreamCards();
  const [creatingWs, setCreatingWs] = useState(false);
  const [creatingSession, setCreatingSession] = useState(false);

  if (cards === null) return <div className="main narrow">加载中…</div>;

  const activeCards = cards.filter(
    (c) => c.lifecycle === "open" && c.visibility === "normal",
  );
  // 卡片全是归档：和「真的什么都没有」是两种边界，得告诉用户东西去哪了。
  const archivedOnly = activeCards.length === 0 && cards.length > 0;

  const modals = (
    <>
      {creatingWs && (
        <NewWorkstreamModal onClose={() => setCreatingWs(false)} onCreated={refresh} />
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
          <SidebarLogo size={64} animated />
          <div className="tagline">对话会结束，上下文不会。</div>
          <p className="page-sub" style={{ textAlign: "center", marginBottom: 18 }}>
            开始一件可以跨 Session、跨 Agent 继续推进的事。
          </p>
          <div className="actions-row">
            <button className="btn primary" onClick={() => setCreatingWs(true)}>+ 新建 Workstream</button>
          </div>
          <div className="muted small" style={{ margin: "10px 0" }}>或</div>
          <div className="actions-row">
            {/* 打开的是全局新建 Session 面板：Agent 未检测到时由面板给出
                「未检测到可用的 Agent CLI」与不可点的启动按钮，这里不再自己
                判断（避免在 Agent 还在解析时把按钮误标成「未检测」）。 */}
            <button className="btn ws-btn" onClick={() => setCreatingSession(true)}>
              {defaultAgent ? (
                <>
                  <AgentIcon agent={defaultAgent} />
                  新建 Session
                </>
              ) : (
                "新建 Session"
              )}
            </button>
            {!defaultAgent && (
              <button className="link small"
                onClick={() => navigate({ view: "settings", section: "agents" })}>
                配置 Agent
              </button>
            )}
          </div>
          {archivedOnly && (
            <p className="muted small" style={{ marginTop: 18 }}>
              已归档的 Workstream 不会出现在首页。{" "}
              <button className="link small"
                onClick={() => navigate({ view: "workstreams" })}>
                Workstreams →
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
      <div className="home-greeting">
        <h1>欢迎回来</h1>
        <p className="sub">继续上次的工作</p>
      </div>

      <ContinueSection navigate={navigate} defaultAgent={defaultAgent} cards={cards} />
      <RecentSessions navigate={navigate} onNewSession={() => setCreatingSession(true)} />

      <div className="home-foot">
        <button className="btn small ghost" onClick={() => setCreatingWs(true)}>+ 新建 Workstream</button>
      </div>

      {modals}
    </div>
  );
}
