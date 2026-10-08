import type { ProjectKind } from "../../types";

export const projectKindLabels: Record<ProjectKind, string> = {
  git: "Git 项目",
  directory: "普通项目",
  chat_directory: "默认项目",
};

export function isNoEndingDefaultProject(
  project: { name?: string | null; id?: string; kind?: ProjectKind },
  defaultProjectId?: string,
): boolean {
  return (
    project.name === "NoEnding Workspace" ||
    (Boolean(defaultProjectId) && project.id === defaultProjectId)
  );
}

export function isAgentDefaultProject(
  project: { name?: string | null; id?: string; kind?: ProjectKind },
  defaultProjectId?: string,
): boolean {
  return project.kind === "chat_directory" && !isNoEndingDefaultProject(project, defaultProjectId);
}
