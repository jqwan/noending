import type { ReactNode } from "react";
import Icon from "./Icon";

/** 看板共用的搜索与筛选卡片；筛选内容和读取状态由页面维护。 */
export default function BoardToolbar({ searchLabel, placeholder, query, onQueryChange, children, className = "" }: {
  searchLabel: string;
  placeholder: string;
  query: string;
  onQueryChange: (value: string) => void;
  children: ReactNode;
  className?: string;
}) {
  return <section className={`ui-section board-toolbar ${className}`} aria-label={`${searchLabel}与筛选`}>
    <label className="board-search">
      <Icon name="search" />
      <input type="text" className="ws-search" aria-label={searchLabel} placeholder={placeholder}
        value={query} onChange={event => onQueryChange(event.target.value)} />
    </label>
    {children}
  </section>;
}
