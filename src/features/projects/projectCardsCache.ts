import type { ProjectCardData } from "../../types";

export const projectCardsCache = {
  cards: null as ProjectCardData[] | null,
};

export function clearProjectCardsCache() {
  projectCardsCache.cards = null;
}
