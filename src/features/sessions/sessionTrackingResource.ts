import { api } from "../../api";
import { createReadResource } from "../../hooks/readResource";

export const sessionTrackingResource = createReadResource(id => api.getSessionTracking(id));
