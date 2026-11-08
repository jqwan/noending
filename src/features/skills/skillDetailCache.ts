import type { SkillDetailBundle, SkillInventory, Project } from "../../types";

export const skillDetailCache = new Map<string, SkillDetailBundle>();

export let skillCatalogCache: SkillInventory | null = null;

export let skillProjectsCache: Project[] | null = null;
export function setSkillProjectsCache(projects: Project[] | null) { skillProjectsCache = projects; }

export function setSkillCatalogCache(catalog: SkillInventory | null): void {
  skillCatalogCache = catalog;
}

export function clearSkillDetailCache(): void {
  skillDetailCache.clear();
  skillCatalogCache = null;
  skillProjectsCache = null;
}
