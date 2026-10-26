import Icon from "../../components/Icon";
import type { Route, WorkstreamEntry } from "../../app/routes";

/**
 * 任务详情的子页切换：详情与协作室共用标题与导航（同会话的历史／详情／终端）。
 * 省略 entry 即详情页，路由上不留默认值。
 */
export default function TaskSubpageTabs({
  workstreamId,
  entry,
  initialTitle,
  navigate,
}: {
  workstreamId: string;
  entry?: WorkstreamEntry;
  initialTitle?: string;
  navigate: (r: Route) => void;
}) {
  const detail = entry !== "collaboration";
  const go = (next: WorkstreamEntry) =>
    navigate({ view: "workstream", workstreamId, initialTitle, entry: next === "detail" ? undefined : next });

  return (
    <div className="settings-seg icon-seg" role="group" aria-label="任务子页">
      <button
        type="button"
        className={detail ? "on" : ""}
        aria-pressed={detail}
        aria-label="任务详情"
        title="任务详情"
        onClick={() => go("detail")}
      >
        <Icon name="info" />
      </button>
      <button
        type="button"
        className={!detail ? "on" : ""}
        aria-pressed={!detail}
        aria-label="协作室"
        title="协作室"
        onClick={() => go("collaboration")}
      >
        <Icon name="chatLines" />
      </button>
    </div>
  );
}
