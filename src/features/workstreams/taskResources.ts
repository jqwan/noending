import { api } from "../../api";
import { createReadResource } from "../../hooks/readResource";

export const taskItemsResource = createReadResource(id => api.listTaskItems(id));
export const taskSummaryResource = createReadResource(id => api.getWorkstreamContextState(id));
