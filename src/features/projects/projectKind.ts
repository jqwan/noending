import type { ProjectKind, ProjectPurpose } from "../../types";

export const projectKindLabels: Record<ProjectKind, string> = {
  git: "Git 项目",
  single_directory: "单目录项目",
  directory_group: "目录组项目",
};

export const projectPurposeLabels: Record<ProjectPurpose, string> = {
  chat: "聊天",
  assistant: "助手",
};

export function isDirectoryGroupProject(project: { kind?: ProjectKind }): boolean {
  return project.kind === "directory_group";
}

export function isProjectAllPathsMissing(project: {
  path_count?: number;
  missing_path_count?: number;
  search_paths?: string[];
  representative_paths?: string[];
}): boolean {
  const total = project.path_count ?? (project.search_paths?.length || project.representative_paths?.length || 0);
  const missing = project.missing_path_count ?? 0;
  if (total === 0) return true;
  return missing >= total;
}

export function isSelectableProject(project: {
  id?: string;
  name?: string;
  visibility?: string;
  kind?: ProjectKind;
  purpose?: ProjectPurpose | null;
  path_count?: number;
  missing_path_count?: number;
  search_paths?: string[];
  representative_paths?: string[];
}): boolean {
  if (project.visibility === "archived") return false;
  return !isProjectAllPathsMissing(project);
}
