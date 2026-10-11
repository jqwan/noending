import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { EVT_SYNCED, onEvent } from "../../app/routes";
import type { AssistantWorkspace } from "../../types";

/** One app-owned project, loaded lazily; native conversation data stays in the
 * ordinary session services. Re-fetch after edits so a renamed project is shown. */
export function useAssistantWorkspace(enabled: boolean) {
  const generation = useRef(0);
  const [workspace, setWorkspace] = useState<AssistantWorkspace | null>(null);
  const [error, setError] = useState("");
  const reload = useCallback(() => {
    const request = ++generation.current;
    setError("");
    api.assistantWorkspace().then(value => {
      if (generation.current === request) setWorkspace(value);
    }).catch(reason => { if (generation.current === request) setError(String(reason)); });
    return () => { if (generation.current === request) ++generation.current; };
  }, []);
  useEffect(() => { if (enabled) return reload(); }, [enabled, reload]);
  useEffect(() => enabled ? onEvent(EVT_SYNCED, reload) : undefined, [enabled, reload]);
  return { workspace, error, reload };
}
