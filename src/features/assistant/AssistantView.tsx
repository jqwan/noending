import NewSessionView from "../sessions/NewSessionView";
import type { Route } from "../../app/routes";
import type { AssistantWorkspace } from "../../types";

/** Assistant launches native conversations in its built-in project. */
export default function AssistantView({ workspace, error, retry, navigate }: {
  workspace: AssistantWorkspace | null;
  error: string;
  retry: () => void;
  navigate: (route: Route) => void;
}) {
  if (!workspace) return <main className="main new-session-page">
    <div className="new-session-content"><h1>新助手会话</h1>
      {error && <div role="alert">{error} <button className="btn" onClick={retry}>重试</button></div>}
    </div>
  </main>;
  return <NewSessionView navigate={navigate} assistantWorkspace={workspace} />;
}
