import type { ProjectKind } from "../../types";

export const projectKindLabels: Record<ProjectKind, string> = {
  git: "Git 项目",
  directory: "普通单目录项目",
  chat_directory: "默认聊天目录项目",
};
