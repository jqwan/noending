import { DetailSection } from "../../components/DetailSection";
import Icon from "../../components/Icon";
import type { ReactNode } from "react";
import type { SessionListItem } from "../../types";

/** 显式跟踪该任务的会话；一个会话可以同时出现在多个任务中。 */
export default function WorkstreamSessions({ sessions, onManageSessions, children }: {
  sessions: SessionListItem[];
  onManageSessions?: () => void;
  children: ReactNode;
}) {
  return (
    <DetailSection sectionId="sessions" label="关联会话" title={<>
          <Icon name="chat" />
          <span>关联会话（{sessions.length}）</span>
        </>} actions={<>{onManageSessions && (
          <button className="btn ghost icon-button small" title="管理关联会话" aria-label="管理关联会话"
            onClick={onManageSessions}>
            <Icon name="plus" />
          </button>
        )}</>}>
      {children}
    </DetailSection>
  );
}
