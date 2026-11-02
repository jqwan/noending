import { DetailList, DetailListRow, DetailSection } from "../../components/DetailSection";
import { recentFirst } from "../../components/detailOrder";
import Icon from "../../components/Icon";
import Notice from "../../components/Notice";
import type { Route } from "../../app/routes";
import type { Workstream } from "../../types";

/** 会话详情与终端共用展示，数据刷新和写入仍由各自的容器负责。 */
export default function SessionTrackedTasks({ tasks, navigate, onAdd, onRemove, busy = false, bound = true, error }: {
  tasks: Workstream[] | null;
  navigate: (route: Route) => void;
  onAdd: () => void;
  onRemove: (id: string) => void;
  busy?: boolean;
  bound?: boolean;
  error?: string;
}) {
  return <DetailSection sectionId="tracked-tasks" label="跟踪任务" title={<><Icon name="tasks" /><span>跟踪任务（{tasks?.length ?? 0}）</span></>} actions={<button type="button" className="btn ghost icon-button small" title="添加跟踪任务" aria-label="添加跟踪任务"
        disabled={busy || !bound || !tasks} onClick={onAdd}><Icon name="plus" /></button>} status={<>
      {error && <Notice>{error}</Notice>}
    </>}>


    {!bound ? <p className="detail-empty">绑定会话后可管理跟踪任务。</p>
      : !tasks ? <p className="detail-empty">正在读取任务…</p>
      : !tasks.length ? <p className="detail-empty">未跟踪任何任务</p>
      : <DetailList label="跟踪任务列表">
        {recentFirst(tasks, task => task.updated_at).map(task => <DetailListRow key={task.id} icon={<Icon name="tasks" />}
          title={task.title} onOpen={() => navigate({ view: "workstream", workstreamId: task.id, initialTitle: task.title })}
          meta={task.visibility === "archived" && <span className="badge warn">已归档</span>}
          actions={<button type="button" className="btn ghost icon-button small" title="取消跟踪" aria-label={`取消跟踪：${task.title}`}
            disabled={busy} onClick={() => onRemove(task.id)}><Icon name="close" /></button>}
        />)}
      </DetailList>}
  </DetailSection>;
}
