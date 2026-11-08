import { useLoadingDelay } from "../hooks/useLoadingDelay";

/** 首次读取统一延迟展示骨架；已有列表的后台刷新不清空内容。 */
export default function BoardLoading({ loading, viewMode }: { loading: boolean; viewMode: "cards" | "list" }) {
  const visible = useLoadingDelay(loading);
  if (!loading || !visible) return null;
  return <div className={viewMode === "cards" ? "board-grid board-loading" : "board-loading board-loading-list"}
    role="status" aria-label="正在读取列表" aria-busy="true">
    {Array.from({ length: viewMode === "cards" ? 3 : 4 }, (_, index) => <div key={index}
      className={`skeleton ${viewMode === "cards" ? "card board-loading-card" : "board-loading-row"}`} />)}
  </div>;
}
