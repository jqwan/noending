import { api } from "../../api";
import { createReadResource } from "../../hooks/readResource";

export const collaborationRoomResource = createReadResource(id => api.collaborationRoomGet(id));
