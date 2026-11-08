import { api } from "../../api";
import { createReadResource } from "../../hooks/readResource";

export const collaborationRoomResource = createReadResource(id => api.collaborationRoomGet(id));
export const collaborationCandidatesResource = createReadResource(id => api.collaborationCandidates(id, null, 20));

/** 首屏成员数据不等待消息历史；停止时的候选也在切页前准备好。 */
export function prepareCollaborationMembers(taskId: string): Promise<void> | undefined {
  const candidates = () => {
    const view = collaborationRoomResource.get(taskId);
    if (view?.run?.status !== "active" && view?.run?.status !== "starting") {
      return collaborationCandidatesResource.ensure(taskId);
    }
  };
  const room = collaborationRoomResource.ensure(taskId);
  return room ? room.then(candidates) : candidates();
}
