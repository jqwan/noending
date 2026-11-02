import type { HTMLAttributes } from "react";

/** 弹窗底部操作区，适用于直接内容或 form 内的提交按钮。 */
export default function DialogActions({ className = "", ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div {...props} className={`modal-actions ${className}`} />;
}
