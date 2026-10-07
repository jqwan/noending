import { useEffect, useState } from "react";
import Icon from "../../components/Icon";
import { api } from "../../api";
import type { Agent } from "../../types";
import type { Route, SessionEntry } from "../../app/routes";

/**
 * 会话页右上角的三段子页切换：概览（详情事实）/ 对话（整屏阅读）/ 终端（内嵌 TUI）。
 * 与会话看板的「会话列表/回收站」同一交互语言：settings-seg 分段控件、图标按钮、
 * navigate 驱动、无本地状态。
 *
 * 终端段只对有 TUI CLI 的 Agent 出现——这是 `get_agent_status` 里的静态适配器事实
 * （terminal_cli），不在前端硬编码名册；状态读不到时按无终端能力渲染，两个基础段
 * 仍然可用。回收站里或源不可用的会话，终端段置灰并说明原因（复用 Resume 的门槛语义）。
 */
export default function SessionSubpageTabs({ sessionId, entry, agent, terminalGate, navigate }: {
  sessionId: string;
  entry: SessionEntry | undefined;
  agent: Agent | null;
  /** 非 null：终端段置灰，值为原因；null = 可用。 */
  terminalGate: string | null;
  navigate: (r: Route) => void;
}) {
  const [hasTerminalCli, setHasTerminalCli] = useState(false);
  useEffect(() => {
    if (!agent) return;
    let live = true;
    api.getAgentStatus()
      .then((status) => {
        if (live) setHasTerminalCli(status[agent]?.terminal_cli === true);
      })
      .catch(() => {
        // 读不到能力事实时按「无终端」渲染；基础两段不受影响。
      });
    return () => {
      live = false;
    };
  }, [agent]);

  const nav = (next: SessionEntry | undefined) =>
    navigate({ view: "session", sessionId, entry: next });

  return (
    <div className="settings-seg" role="group" aria-label="会话子页">
      <button
        className={entry === undefined ? "on" : ""}
        aria-pressed={entry === undefined}
        aria-label="概览"
        title="概览"
        onClick={() => nav(undefined)}
      >
        <Icon name="info" />
      </button>
      <button
        className={entry === "conversation" ? "on" : ""}
        aria-pressed={entry === "conversation"}
        aria-label="对话"
        title="对话"
        onClick={() => nav("conversation")}
      >
        <Icon name="chat" />
      </button>
      {hasTerminalCli && (
        <button
          className={entry === "terminal" ? "on" : ""}
          aria-pressed={entry === "terminal"}
          aria-label="终端"
          title={terminalGate ?? "终端"}
          disabled={terminalGate !== null}
          onClick={() => nav("terminal")}
        >
          <Icon name="terminal" />
        </button>
      )}
    </div>
  );
}
