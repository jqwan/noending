import { forwardRef } from "react";

/** 左右栏使用同一图形，箭头表示下一次点击的方向。 */
const PanelToggle = forwardRef<HTMLButtonElement, {
  side: "left" | "right";
  open: boolean;
  label: string;
  controls: string;
  hint?: string;
  onToggle: () => void;
}>(function PanelToggle({ side, open, label, controls, hint, onToggle }, ref) {
  const action = `${open ? "收起" : "展开"}${label}`;
  return <button ref={ref} type="button" className="icon-button panel-toggle"
    aria-label={action} title={`${action}${hint ? ` · ${hint}` : ""}`}
    aria-expanded={open} aria-controls={controls} onClick={onToggle}>
    <svg className="ui-icon" viewBox="0 0 18 18" aria-hidden="true">
      <g transform={side === "left" ? "translate(18 0) scale(-1 1)" : undefined}>
        {open && <path className="panel-toggle-fill" d="M12 3h2a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-2Z" />}
        <rect x="2" y="3" width="14" height="12" rx="2" />
        <path d="M12 3v12" />
        <path d={open ? "M6.5 6.5 9 9l-2.5 2.5" : "M9 6.5 6.5 9 9 11.5"} />
      </g>
    </svg>
  </button>;
});

export default PanelToggle;
