import type { PackageDetail, PackageInventory } from "../../types";

export const packageDetailCache = new Map<string, PackageDetail>();

export let packageCatalogCache: PackageInventory | null = null;

export function setPackageCatalogCache(inventory: PackageInventory | null): void {
  packageCatalogCache = inventory;
}
