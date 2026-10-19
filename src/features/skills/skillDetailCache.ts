import type { SkillDetailBundle, SkillInventory } from "../../types";

export const skillDetailCache = new Map<string, SkillDetailBundle>();

export let skillCatalogCache: SkillInventory | null = null;

export function setSkillCatalogCache(catalog: SkillInventory | null): void {
  skillCatalogCache = catalog;
}

export function clearSkillDetailCache(): void {
  skillDetailCache.clear();
  skillCatalogCache = null;
}
