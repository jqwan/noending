import type {
  WorkstreamContext as WorkstreamContextData,
  WorkstreamContextView,
} from "../../types";

export interface WorkstreamDetailCached {
  ctx: WorkstreamContextData;
  ctxState: WorkstreamContextView | null;
}

/** Task organization and the current Markdown summary. All reads are pure. */
export const workstreamDetailCache = new Map<string, WorkstreamDetailCached>();

export function clearWorkstreamDetailCache() {
  workstreamDetailCache.clear();
}
