import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";

export default function NewProjectSessionButton({ projectId, archivePathId, navigate }: {
  projectId: string;
  archivePathId?: string;
  navigate: (route: Route) => void;
}) {
  return (
    <button type="button" className="btn ghost icon-button small" aria-label="新建会话" title="新建会话"
      onClick={event => {
        event.stopPropagation();
        navigate({ view: "new-session", projectId, ...(archivePathId && { archivePathId }) });
      }}>
      <Icon name="plus" />
    </button>
  );
}
