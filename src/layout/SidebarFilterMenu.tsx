import { useEffect, useMemo, useRef, useState } from "react";
import Icon from "../components/Icon";
import {
  SIDEBAR_FILTER_OPTIONS,
  SIDEBAR_SORT_OPTIONS,
  type SidebarFilterKey,
  type SidebarSortKey,
} from "./sidebarModel";

export default function SidebarFilterMenu({
  filters,
  onToggleFilter,
  onClearFilters,
  sortBy,
  onSelectSortBy,
  showSessionIcons,
  onToggleShowSessionIcons,
}: {
  filters: SidebarFilterKey[];
  onToggleFilter: (key: SidebarFilterKey) => void;
  onClearFilters: () => void;
  sortBy: SidebarSortKey;
  onSelectSortBy: (sortKey: SidebarSortKey) => void;
  showSessionIcons: boolean;
  onToggleShowSessionIcons: () => void;
}) {
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);

  const currentSortOpt = useMemo(() => {
    return SIDEBAR_SORT_OPTIONS.find((opt) => opt.key === sortBy) ?? SIDEBAR_SORT_OPTIONS[0];
  }, [sortBy]);

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
    <div className="sidebar-filter-wrapper" ref={wrapperRef}>
      <button
        type="button"
        className={`sidebar-filter-trigger${filters.length > 0 ? " active" : ""}`}
        onClick={() => setOpen((prev) => !prev)}
        aria-label="筛选与排序会话"
        title={
          filters.length > 0
            ? `已筛选 ${filters.length} 项 · ${currentSortOpt.label}`
            : `筛选与排序（当前：${currentSortOpt.label}）`
        }
        aria-expanded={open}
      >
        <Icon name="sliders" />
      </button>
      {open && (
        <div className="sidebar-filter-menu" role="menu">
          <div className="sidebar-filter-menu-header">
            <span>排序方式</span>
          </div>
          {SIDEBAR_SORT_OPTIONS.map((opt) => {
            const checked = sortBy === opt.key;
            return (
              <button
                key={opt.key}
                type="button"
                className={`sidebar-filter-item radio${checked ? " checked" : ""}`}
                onClick={() => onSelectSortBy(opt.key)}
                role="menuitemradio"
                aria-checked={checked}
              >
                <span className="sidebar-filter-radio">
                  {checked && <span className="sidebar-filter-radio-dot" />}
                </span>
                <span className="sidebar-filter-label">{opt.label}</span>
              </button>
            );
          })}

          <div className="sidebar-filter-menu-divider" />

          <div className="sidebar-filter-menu-header">
            <span>筛选条件</span>
            {filters.length > 0 && (
              <button
                type="button"
                className="sidebar-filter-clear-btn"
                onClick={onClearFilters}
              >
                重置
              </button>
            )}
          </div>
          {SIDEBAR_FILTER_OPTIONS.map((opt) => {
            const checked = filters.includes(opt.key);
            return (
              <button
                key={opt.key}
                type="button"
                className={`sidebar-filter-item${checked ? " checked" : ""}`}
                onClick={() => onToggleFilter(opt.key)}
                role="menuitemcheckbox"
                aria-checked={checked}
              >
                <span className="sidebar-filter-check">
                  {checked && <Icon name="check" />}
                </span>
                <span className="sidebar-filter-label">{opt.label}</span>
              </button>
            );
          })}

          <div className="sidebar-filter-menu-divider" />

          <div className="sidebar-filter-menu-header">
            <span>显示设置</span>
          </div>
          <button
            type="button"
            className={`sidebar-filter-item${showSessionIcons ? " checked" : ""}`}
            onClick={onToggleShowSessionIcons}
            role="menuitemcheckbox"
            aria-checked={showSessionIcons}
          >
            <span className="sidebar-filter-check">
              {showSessionIcons && <Icon name="check" />}
            </span>
            <span className="sidebar-filter-label">显示会话图标</span>
          </button>
        </div>
      )}
    </div>
  );
}
