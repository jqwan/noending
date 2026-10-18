import type { ProjectContextView } from "../../types";

export const projectSummaryCache = new Map<string, ProjectContextView>();

export function clearProjectSummaryCache() {
  projectSummaryCache.clear();
}
