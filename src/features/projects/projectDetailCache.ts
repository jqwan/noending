import type { ProjectDetailData } from "../../types";
import { clearProjectSummaryCache } from "./projectSummaryCache";


/**
 * Project Detail 缓存。
 * 一次 `get_project_detail` 读到全部真相——它拥有的 WorkspacePaths、
 * 经由这些路径到达的 Workstreams，以及 cwd 落在这些路径上的 Sessions。
 */
export const projectDetailCache = new Map<string, ProjectDetailData>();

export function clearProjectDetailCache() {
  projectDetailCache.clear();
  clearProjectSummaryCache();
}
