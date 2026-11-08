import { api } from "../../api";
import { createReadResource } from "../../hooks/readResource";

export const extractionSettingsResource = createReadResource(() => api.getExtractionSettings());
export const workspaceSettingsResource = createReadResource(() => api.getWorkspaceSettings());
