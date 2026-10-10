import type { ProjectKind } from "../../types";

export const projectKindLabels: Record<ProjectKind, string> = {
  git: "Git 项目",
  directory: "普通项目",
  chat_directory: "默认项目",
};

export function isChatDirectoryProject(project: { kind?: ProjectKind }): boolean {
  return project.kind === "chat_directory";
}
