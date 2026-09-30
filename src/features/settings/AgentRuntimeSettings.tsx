import { useEffect, useState } from "react";
import { api } from "../../api";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, type Agent, type AgentRuntimeSettings } from "../../types";

/** 设置 → Agent → TUI/CLI 的一块：安装状态。模型 / 思考强度不再提供 override，
 *  一律沿用 Agent 默认值——NoEnding 不替用户做这类选择。 */
export default function AgentRuntimeRow({ agent }: { agent: Agent }) {
  const [st, setSt] = useState<AgentRuntimeSettings | null>(null);

  useEffect(() => {
    api.getAgentRuntimeSettings(agent).then(setSt).catch(console.error);
  }, [agent]);

  if (!st) {
    return (
      <div className="settings-agent-runtime muted small">
        {AGENT_LABELS[agent]} 加载中…
      </div>
    );
  }

  return (
    <div className="settings-agent-runtime">
      <div className="row-line">
        <div>
          <div className="settings-row-label row" style={{ gap: 7 }}>
            <AgentIcon agent={agent} />
            {AGENT_LABELS[agent]}
          </div>
          <div className="settings-row-hint mono">{st.executable ?? "未找到可执行文件"}</div>
        </div>
        <div className="row">
          {st.version && <span className="muted mono small">{st.version}</span>}
          <span className={`badge ${st.detected ? "success" : ""}`}>
            {st.detected ? "已检测" : "未检测"}
          </span>
        </div>
      </div>
    </div>
  );
}
