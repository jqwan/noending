import { useEffect, useMemo, useRef, useState } from "react";
import Icon from "../components/Icon";
import { SIDEBAR_GROUP_OPTIONS, type SidebarGroupBy } from "./sidebarModel";

export default function SidebarGroupMenu({
  groupBy,
  onSelectGroupBy,
}: {
  groupBy: SidebarGroupBy;
  onSelectGroupBy: (groupBy: SidebarGroupBy) => void;
}) {
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);

  const currentGroupOpt = useMemo(() => {
    return SIDEBAR_GROUP_OPTIONS.find((opt) => opt.key === groupBy) ?? SIDEBAR_GROUP_OPTIONS[0];
  }, [groupBy]);

  useEffect(() => {
    if (!open) return;
    const onClickOutside = (e: MouseEvent) => {
      if (wrapperRef.current && !wrapperRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onClickOutside);
    return () => document.removeEventListener("mousedown", onClickOutside);
  }, [open]);

  return (
    <div className="sidebar-group-wrapper" ref={wrapperRef}>
      <button
        type="button"
        className={`sidebar-group-trigger${open ? " active" : ""}`}
        onClick={() => setOpen((prev) => !prev)}
        aria-label={`会话归类：${currentGroupOpt.fullLabel}`}
        title={`切换会话归类方式（当前：${currentGroupOpt.fullLabel}）`}
        aria-expanded={open}
      >
        <span className="sidebar-header-icon">
          <Icon name={currentGroupOpt.icon} />
        </span>
        <span className="sidebar-header-title">{currentGroupOpt.label}</span>
        <Icon name="chevronDown" className="sidebar-group-arrow" />
      </button>
      {open && (
        <div className="sidebar-group-menu" role="menu">
          <div className="sidebar-group-menu-header">
            <span>会话归类</span>
          </div>
          {SIDEBAR_GROUP_OPTIONS.map((opt) => {
            const active = groupBy === opt.key;
            return (
              <button
                key={opt.key}
                type="button"
                className={`sidebar-group-item${active ? " checked" : ""}`}
                onClick={() => {
                  onSelectGroupBy(opt.key);
                  setOpen(false);
                }}
                role="menuitemradio"
                aria-checked={active}
              >
                <span className="sidebar-group-item-icon">
                  <Icon name={opt.icon} />
                </span>
                <span className="sidebar-group-item-label">{opt.fullLabel}</span>
                {active && (
                  <span className="sidebar-group-item-check">
                    <Icon name="check" />
                  </span>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
