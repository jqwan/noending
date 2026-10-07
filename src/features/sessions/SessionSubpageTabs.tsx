import Icon from "../../components/Icon";
import type { Route, SessionEntry } from "../../app/routes";

/**
 * 会话页右上角的两段子页切换：概览（详情事实）/ 对话（整屏阅读）。
 * 与会话看板的「会话列表/回收站」同一交互语言：settings-seg 分段控件、
 * 图标按钮、navigate 驱动、无本地状态。
 *
 * 终端不在这里——它是一等独立视图（view:"terminal"），入口是
 * SessionHeaderActions 里的终端按钮（先跳后启）。
 */
export default function SessionSubpageTabs({ sessionId, entry, navigate }: {
  sessionId: string;
  entry: SessionEntry | undefined;
  navigate: (r: Route) => void;
}) {
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
    </div>
  );
}
