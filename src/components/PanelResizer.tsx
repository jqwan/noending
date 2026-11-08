import { useCallback, useEffect, useRef, useState } from "react";

export default function PanelResizer({ side, label, controls, value, min, max, defaultValue, onResize, className = "" }: {
  side: "left" | "right";
  label: string;
  controls: string;
  value: number;
  min: number;
  max: number;
  defaultValue: number;
  onResize: (width: number) => void;
  className?: string;
}) {
  const drag = useRef<{ x: number; width: number; pointer: number; target: HTMLDivElement } | null>(null);
  const [dragging, setDragging] = useState(false);
  const finish = useCallback((update = true) => {
    const current = drag.current;
    if (!current) return;
    drag.current = null;
    delete document.documentElement.dataset.panelResizing;
    if (update) setDragging(false);
    if (current.target.hasPointerCapture(current.pointer)) current.target.releasePointerCapture(current.pointer);
  }, []);
  useEffect(() => {
    const blur = () => finish();
    window.addEventListener("blur", blur);
    return () => { window.removeEventListener("blur", blur); finish(false); };
  }, [finish]);
  const direction = side === "left" ? 1 : -1;
  const resize = (width: number) => onResize(Math.max(min, Math.min(max, Math.round(width))));
  return <div className={`panel-resizer${dragging ? " is-dragging" : ""} ${className}`} role="separator" tabIndex={0}
    aria-label={label} aria-controls={controls} aria-orientation="vertical"
    aria-valuemin={min} aria-valuemax={max} aria-valuenow={value}
    aria-valuetext={`${value} 像素`} title="拖动调整宽度，双击恢复默认"
    onDoubleClick={() => resize(defaultValue)}
    onKeyDown={event => {
      if (event.key === "Escape" && drag.current) {
        event.preventDefault(); event.stopPropagation(); finish(); return;
      }
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      resize(event.key === "Home" ? min : event.key === "End" ? max
        : value + (event.key === "ArrowRight" ? 8 : -8) * direction);
    }}
    onPointerDown={event => {
      if (event.button !== 0 || event.isPrimary === false || document.documentElement.dataset.panelResizing) return;
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { x: event.clientX, width: value, pointer: event.pointerId, target: event.currentTarget };
      document.documentElement.dataset.panelResizing = "true";
      setDragging(true);
    }}
    onPointerMove={event => {
      if (drag.current?.pointer === event.pointerId && event.currentTarget.hasPointerCapture(event.pointerId)) {
        resize(drag.current.width + (event.clientX - drag.current.x) * direction);
      }
    }}
    onPointerUp={event => {
      if (drag.current?.pointer === event.pointerId) finish();
    }}
    onPointerCancel={event => { if (drag.current?.pointer === event.pointerId) finish(); }}
    onLostPointerCapture={event => { if (drag.current?.pointer === event.pointerId) finish(); }}
  />;
}
