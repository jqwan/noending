import type { TerminalSummary } from "../../types";
import { terminalActivityLabel } from "./terminalActivity";

/** Agent activity is separate from the terminal connection/action icon. */
export default function TerminalActivityIndicator({ terminal }: {
  terminal: TerminalSummary;
}) {
  const activity = terminal.activity;
  const state = !terminal.live ? "exited"
    : (!activity || activity.state === "unknown") ? "unknown"
    : activity?.state === "working" ? "working"
    : activity?.state === "waiting" ? "waiting"
    : activity?.state === "failed" ? "failed" : "idle";
  const unread = state === "idle" && activity?.unread;
  const label = terminalActivityLabel(terminal);
  return <span className="terminal-activity-indicator" role="img" aria-label={label} title={label}>
    <span aria-hidden="true" className={`terminal-activity-marker is-${state}${unread ? " is-unread" : ""}`}>
      {state === "unknown" ? "?" : state === "waiting" || state === "failed" ? "!" : null}
    </span>
  </span>;
}
