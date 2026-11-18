import type { AssistantCard, AssistantDetail } from "../../types";

export interface AssistantCardsCacheState {
  unarchived: AssistantCard[] | null;
  archived: AssistantCard[] | null;
  all: AssistantCard[] | null;
}

export const assistantCardsCache: AssistantCardsCacheState = {
  unarchived: null,
  archived: null,
  all: null,
};

export const assistantDetailCache = new Map<string, AssistantDetail>();

export function clearAssistantCardsCache() {
  assistantCardsCache.unarchived = null;
  assistantCardsCache.archived = null;
  assistantCardsCache.all = null;
}

export function clearAssistantDetailCache(id?: string) {
  if (id) {
    assistantDetailCache.delete(id);
  } else {
    assistantDetailCache.clear();
  }
}

export function clearAssistantsCache() {
  clearAssistantCardsCache();
  clearAssistantDetailCache();
}
