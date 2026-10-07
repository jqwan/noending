import Icon from "../../components/Icon";
import { capsOf } from "./sessionFormats";
import type { Agent } from "../../types";
import type { Route, SessionEntry } from "../../app/routes";

/**
 * 会话页右上角的三段子页切换：概览（详情事实）/ 对话（整屏阅读）/ 终端（内嵌 TUI）。
 * 与会话看板的「会话列表/回收站」同一交互语言：settings-seg 分段控件、图标按钮、
 * navigate 驱动、无本地状态。
 *
 * 终端段按会话格式的静态能力出现（sessionFormats 的能力表，与后端 adapters 的
 * 路由事实同源）：antigravity 的桌面存储没有 CLI，不出现终端段——能力跟格式走，
 * 不跟 agent 走。回收站里或源不可用的会话，终端段置灰并说明原因（Resume 的
 * 门槛语义）；这是父组件算好的 `terminalGate`。
 */
export default function SessionSubpageTabs({ sessionId, entry, agent, sourceKind, terminalGate, navigate }: {
  sessionId: string;
  entry: SessionEntry | undefined;
  agent: Agent | null;
  /** 会话的 source_kind；未读到时按 agent 键兜底。 */
  sourceKind: string | undefined;
  /** 非 null：终端段置灰，值为原因；null = 可用。 */
  terminalGate: string | null;
  navigate: (r: Route) => void;
}) {
  const hasTerminal = agent ? capsOf(agent, sourceKind).terminal : false;

  const nav = (next: SessionEntry | undefined) =>
    navigate({ view: "session", sessionId, entry: next });

  return (
    <div className="settings-seg icon-seg" role="group" aria-label="会话子页">
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
      {hasTerminal && (
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
