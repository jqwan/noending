import React, { useState } from "react";
import { api } from "../../api";
import AgentIcon from "../../components/AgentIcon";
import SidebarLogo from "../../components/SidebarLogo";
import type { Route } from "../../app/routes";
import { ContinueSection, RecentSessions } from "./ContinueSection";
import { useWorkstreamCards } from "../workstreams/useWorkstreamCards";
import NewWorkstreamModal from "../workstreams/NewWorkstreamModal";
import { announceLaunch } from "../launcher/LaunchResultModal";

/**
 * Home = Continue where you left off（整体设计方案 §12）。
 * 唯一任务：让用户用最短路径回到最近推进的 Workstream。
 */
export default function HomeView({ navigate }: { navigate: (r: Route) => void }) {
  const { cards, defaultAgent, refresh } = useWorkstreamCards();
  const [creatingWs, setCreatingWs] = useState(false);

  if (cards === null) return <div className="main narrow">加载中…</div>;

  const hasWorkstreams = cards.some(
    (c) => c.lifecycle === "open" && c.visibility === "normal",
  );

  // Workstream-centered, not Workstream-required：空状态保留直接 New Session。
  const plainNewSession = async () => {
    if (!defaultAgent) return;
    try {
      announceLaunch("启动", await api.launchNewSession(defaultAgent, []));
    } catch (e) {
      console.error(e);
    }
  };

  if (!hasWorkstreams) {
    return (
      <div className="main narrow home">
        <div className="hero">
          <SidebarLogo size={64} animated />
          <div className="tagline">Conversations end. Context doesn't.</div>
          <p className="page-sub" style={{ textAlign: "center", marginBottom: 18 }}>
            开始一件可以跨 Session、跨 Agent 继续推进的事。
          </p>
          <div className="actions-row">
            <button className="btn primary" onClick={() => setCreatingWs(true)}>+ New Workstream</button>
          </div>
          <div className="muted small" style={{ margin: "10px 0" }}>or</div>
          <div className="actions-row">
            <button className="btn ws-btn" disabled={!defaultAgent}
              title={defaultAgent ? undefined : "未检测到可用的 Agent CLI"}
              onClick={plainNewSession}>
              {defaultAgent ? (
                <>
                  <AgentIcon agent={defaultAgent} />
                  New Session
                </>
              ) : (
                "New Session"
              )}
            </button>
            {!defaultAgent && (
              <button className="link small"
                onClick={() => navigate({ view: "settings", section: "agents" })}>
                Configure Agents
              </button>
            )}
          </div>
        </div>

        {creatingWs && <NewWorkstreamModal onClose={() => setCreatingWs(false)} onCreated={refresh} />}
      </div>
    );
  }

  return (
    <div className="main narrow">
      <div className="home-greeting">
        <h1>Good to see you again.</h1>
        <p className="sub">Continue where you left off.</p>
      </div>

      <ContinueSection navigate={navigate} defaultAgent={defaultAgent} cards={cards} />
      <RecentSessions navigate={navigate} />

      <div className="home-foot">
        <button className="btn small ghost" onClick={() => setCreatingWs(true)}>+ New Workstream</button>
      </div>

      {creatingWs && <NewWorkstreamModal onClose={() => setCreatingWs(false)} onCreated={refresh} />}
    </div>
  );
}
