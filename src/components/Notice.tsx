import type { HTMLAttributes } from "react";
import Icon from "./Icon";

/** 多行反馈使用提示块，短状态继续使用 badge。 */
export default function Notice({ tone = "error", className = "", children, ...props }: HTMLAttributes<HTMLDivElement> & { tone?: "error" | "warning" | "info" }) {
  return <div {...props} className={`notice notice-${tone} ${className}`} role={props.role ?? (tone === "error" ? "alert" : undefined)}>
    <Icon name="info" />
    <div className="notice-content">{children}</div>
  </div>;
}
