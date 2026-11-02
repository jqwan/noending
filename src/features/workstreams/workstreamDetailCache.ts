import type {
  WorkstreamContext as WorkstreamContextData,
} from "../../types";

export interface WorkstreamDetailCached {
  ctx: WorkstreamContextData;
}

/** Cached task organization; summary data is owned by its card. */
export const workstreamDetailCache = new Map<string, WorkstreamDetailCached>();

export function clearWorkstreamDetailCache() {
  workstreamDetailCache.clear();
}
