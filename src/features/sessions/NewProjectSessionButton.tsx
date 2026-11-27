import type { Route } from "../../app/routes";
import Icon from "../../components/Icon";

export default function NewProjectSessionButton({ projectId, archivePathId, navigate }: {
  projectId: string;
  archivePathId?: string;
  navigate: (route: Route) => void;
}) {
  const label = archivePathId ? "开始对话" : "新建会话";
  return (
    <button type="button" className="btn ghost icon-button small" aria-label={label} title={label}
      onClick={event => {
        event.stopPropagation();
        navigate({ view: "new-session", projectId, ...(archivePathId && { archivePathId }) });
      }}>
      <Icon name="plus" />
    </button>
  );
}
