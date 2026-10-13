import { useEffect, useRef, useState } from "react";
import Icon, { type IconName } from "./Icon";

export interface CustomSelectOption {
  value: string;
  label: string;
  icon?: IconName | string;
  disabled?: boolean;
}

export interface CustomSelectProps {
  value: string;
  onChange: (value: string) => void;
  options: CustomSelectOption[];
  ariaLabel?: string;
  title?: string;
  disabled?: boolean;
  className?: string;
  placeholder?: string;
  prefix?: React.ReactNode;
  placement?: "auto" | "top" | "bottom";
}

export default function CustomSelect({
  value,
  onChange,
  options,
  ariaLabel,
  title,
  disabled = false,
  className = "",
  placeholder,
  prefix,
  placement = "auto",
}: CustomSelectProps) {
  const [open, setOpen] = useState(false);
  const [dropup, setDropup] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const selectedOption = options.find((o) => o.value === value);
  const displayLabel = selectedOption?.label ?? placeholder ?? (options[0]?.label ?? "");

  const toggleOpen = () => {
    if (disabled) return;
    setOpen((prev) => {
      const next = !prev;
      if (next && containerRef.current) {
        if (placement === "top") {
          setDropup(true);
        } else if (placement === "bottom") {
          setDropup(false);
        } else {
          const rect = containerRef.current.getBoundingClientRect();
          const spaceBelow = window.innerHeight - rect.bottom;
          const spaceAbove = rect.top;
          setDropup(spaceBelow < 220 && spaceAbove > spaceBelow);
        }
      }
      return next;
    });
  };

  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div
      ref={containerRef}
      className={`custom-select-control ${open ? "open" : ""} ${dropup ? "dropup" : ""} ${disabled ? "disabled" : ""} ${className}`}
      onClick={(e) => {
        if (disabled) return;
        if ((e.target as HTMLElement).closest(".custom-select-menu")) return;
        toggleOpen();
      }}
      role="button"
      tabIndex={disabled ? -1 : 0}
      aria-haspopup="listbox"
      aria-expanded={open}
      aria-label={ariaLabel ? `${ariaLabel}：${displayLabel}` : title ? `${title}：${displayLabel}` : undefined}
      title={title || displayLabel}
      onKeyDown={(e) => {
        if (disabled) return;
        if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
          e.preventDefault();
          toggleOpen();
        }
      }}
    >
      {prefix}
      <span className="custom-select-label truncate" data-label={displayLabel} />
      <Icon name="chevronDown" className="custom-select-chevron" />

      {/* Hidden underlying native select for full accessibility and testing compatibility */}
      <select
        aria-label={ariaLabel}
        title={title}
        value={value}
        onChange={(e) => {
          onChange(e.target.value);
        }}
        disabled={disabled}
        tabIndex={-1}
        className="custom-select-native"
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value} disabled={opt.disabled}>
            {opt.label}
          </option>
        ))}
      </select>

      {open && (
        <div className="custom-select-menu" role="listbox" aria-label={ariaLabel || title}>
          <div className="custom-select-list">
            {options.map((opt) => {
              const isSelected = opt.value === value;
              return (
                <button
                  key={opt.value}
                  type="button"
                  className={`custom-select-item ${isSelected ? "selected" : ""}`}
                  disabled={opt.disabled}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (opt.disabled) return;
                    onChange(opt.value);
                    setOpen(false);
                  }}
                  role="option"
                  aria-selected={isSelected}
                >
                  {opt.icon && (
                    <span className="custom-select-item-icon">
                      <Icon name={opt.icon as IconName} />
                    </span>
                  )}
                  <span className="custom-select-item-label truncate">{opt.label}</span>
                  {isSelected && (
                    <span className="custom-select-item-check">
                      <Icon name="check" />
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
