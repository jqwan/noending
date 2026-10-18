import React, { useRef, useState } from "react";
import Icon from "../components/Icon";
import AgentIcon from "../components/AgentIcon";
import type { Agent } from "../types";
import type { SidebarSessionGroup } from "./sidebarModel";

export default function SidebarGroupHeader({
  group,
  isCollapsed,
  isUnassignedGroup,
  isKnownAgent,
  iconName,
  onToggle,
}: {
  group: SidebarSessionGroup;
  isCollapsed: boolean;
  isUnassignedGroup: boolean;
  isKnownAgent: boolean;
  iconName: "tasks" | "tasksExpanded" | "project" | "projectOpen" | "bot" | "minus" | "inbox" | "chevronRight" | "chevronDown";
  onToggle: () => void;
}) {
  const titleRef = useRef<HTMLSpanElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const [overflow, setOverflow] = useState(0);
  const [hovered, setHovered] = useState(false);

  const onMouseEnter = () => {
    setHovered(true);
    if (titleRef.current) {
      const available = titleRef.current.clientWidth;
      const textWidth = textRef.current
        ? textRef.current.getBoundingClientRect().width || textRef.current.scrollWidth
        : 0;
      const contentWidth = textWidth > 0 ? textWidth : (titleRef.current.scrollWidth > titleRef.current.clientWidth ? titleRef.current.scrollWidth : 0);
      const diff = Math.ceil(contentWidth - available);
      setOverflow(diff > 2 && contentWidth > available ? diff : 0);
    }
  };

  const onMouseLeave = () => {
    setHovered(false);
    setOverflow(0);
  };

  const isScrolling = hovered && overflow > 0;
  const duration = Math.max(2.5, overflow / 30);

  return (
    <button
      type="button"
      className={`sidebar-recent-group-header ${isCollapsed ? "collapsed" : ""}${isUnassignedGroup ? " unassigned-group" : ""}`}
      onClick={onToggle}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      aria-expanded={!isCollapsed}
      title={`${group.title} (${group.sessions.length})`}
    >
      <span className={`sidebar-group-icon${isKnownAgent ? " agent-group-icon" : ""}`}>
        {isKnownAgent ? (
          <AgentIcon agent={group.id as Agent} size={14} />
        ) : (
          <Icon name={iconName} />
        )}
      </span>
      <span
        ref={titleRef}
        className={`sidebar-recent-group-title${isScrolling ? " scrolling" : " truncate"}`}
        style={isScrolling ? ({
          "--overflow-width": `${overflow}px`,
          "--scroll-duration": `${duration}s`,
        } as React.CSSProperties) : undefined}
      >
        <span ref={textRef} className="sidebar-recent-group-title-text">{group.title}</span>
      </span>
      <span className="sidebar-recent-count">{group.sessions.length}</span>
    </button>
  );
}
