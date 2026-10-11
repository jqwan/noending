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
