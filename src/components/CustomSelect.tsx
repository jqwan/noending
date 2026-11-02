import { useEffect, useRef, useState } from "react";
import Icon, { type IconName } from "./Icon";
import PopupMenu, { PopupMenuItem } from "./PopupMenu";

export interface CustomSelectOption {
  value: string;
  label: string;
  icon?: IconName | string | React.ReactNode;
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

export default function CustomSelect({ value, onChange, options, ariaLabel, title, disabled = false, className = "", placeholder, prefix, placement = "auto" }: CustomSelectProps) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  const displayLabel = options.find(option => option.value === value)?.label ?? placeholder ?? options[0]?.label ?? "";
  const close = () => setOpen(false);
  const select = (next: string) => { onChange(next); close(); trigger.current?.focus(); };

  return (
    <>
      <button type="button" ref={trigger} className={`custom-select-control ${open ? "open" : ""} ${className}`}
        disabled={disabled} aria-haspopup="listbox" aria-expanded={open && !disabled}
        aria-label={ariaLabel ? `${ariaLabel}：${displayLabel}` : title ? `${title}：${displayLabel}` : displayLabel}
        title={title || displayLabel} onClick={() => setOpen(value => !value)}
        onKeyDown={event => {
          if (event.key === "Escape") close();
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            setOpen(true);
          }
        }}>
        {prefix}
        <span className="custom-select-label truncate">{displayLabel}</span>
        <Icon name="chevronDown" className="custom-select-chevron" />
      </button>
      {open && !disabled && (
        <PopupMenu anchorRef={trigger} placement={placement} onDismiss={close} focusOnOpen role="listbox" aria-label={ariaLabel || title}
          onKeyDown={event => {
            if (event.key.length !== 1 || event.ctrlKey || event.metaKey || event.altKey) return;
            const option = options.find(option => !option.disabled && option.label.toLocaleLowerCase().startsWith(event.key.toLocaleLowerCase()));
            if (!option) return;
            const item = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="option"]')].find(item => item.dataset.value === option.value);
            item?.focus();
          }}>
          {options.map(option => (
            <PopupMenuItem key={option.value} data-value={option.value} role="option" selected={option.value === value} disabled={option.disabled}
              title={option.label} onClick={() => select(option.value)}>
              {option.icon && <span className="custom-select-item-icon">{typeof option.icon === "string" ? <Icon name={option.icon as IconName} /> : option.icon}</span>}
              <span className="popup-menu-label">{option.label}</span>
            </PopupMenuItem>
          ))}
        </PopupMenu>
      )}
    </>
  );
}
