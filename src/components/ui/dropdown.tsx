import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import { Check, ChevronDown, Trash2 } from "lucide-react";

export interface DropdownItem<T extends string> {
  value: T;
  label: ReactNode;
  textValue?: string;
  disabled?: boolean;
  checked?: boolean;
  leading?: ReactNode;
  shortcut?: string;
  destructiveActionLabel?: string;
  onDestructiveAction?: (value: T) => void;
}

export interface DropdownGroup<T extends string> {
  label?: string;
  items: Array<DropdownItem<T>>;
}

interface DropdownMenuProps<T extends string> {
  className?: string;
  disabled?: boolean;
  emptyText?: string;
  groups: Array<DropdownGroup<T>>;
  menuLabel: string;
  onSelect: (value: T) => void;
  trigger: (args: { open: boolean; buttonId: string; menuId: string }) => ReactNode;
}

export function DropdownMenu<T extends string>({
  className,
  disabled,
  emptyText = "无可用选项",
  groups,
  menuLabel,
  onSelect,
  trigger,
}: DropdownMenuProps<T>) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonId = useId();
  const menuId = useId();
  const flatItems = groups.flatMap((group) => group.items).filter((item) => !item.disabled);

  useEffect(() => {
    if (!open) return;
    const closeOnOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const current = document.activeElement;
        const items = Array.from(
          rootRef.current?.querySelectorAll<HTMLButtonElement>(
            ".lf-dropdown-item:not(:disabled)",
          ) ?? [],
        );
        if (items.length === 0) return;
        const currentIndex = items.findIndex((item) => item === current);
        const delta = event.key === "ArrowDown" ? 1 : -1;
        const nextIndex =
          currentIndex < 0 ? 0 : (currentIndex + delta + items.length) % items.length;
        items[nextIndex]?.focus();
      }
    };
    window.addEventListener("pointerdown", closeOnOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOnOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  return (
    <div className={className ?? "lf-dropdown"} ref={rootRef}>
      <button
        aria-label={menuLabel}
        aria-controls={menuId}
        aria-expanded={open}
        aria-haspopup="menu"
        disabled={disabled}
        id={buttonId}
        type="button"
        onClick={() => !disabled && setOpen((value) => !value)}
      >
        {trigger({ open, buttonId, menuId })}
      </button>
      {open && (
        <div
          aria-label={menuLabel}
          aria-labelledby={buttonId}
          className="lf-dropdown-menu"
          id={menuId}
          role="menu"
          data-empty={flatItems.length === 0 || undefined}
        >
          {flatItems.length === 0 ? (
            <div className="lf-dropdown-empty">{emptyText}</div>
          ) : (
            groups.map((group, groupIndex) => (
              <div className="lf-dropdown-group" key={`${group.label ?? "group"}-${groupIndex}`}>
                {group.label && <div className="lf-dropdown-label">{group.label}</div>}
                {group.items.map((item) => (
                  <div className="lf-dropdown-row" key={item.value}>
                    <button
                      className="lf-dropdown-item"
                      disabled={item.disabled}
                      role="menuitemradio"
                      type="button"
                      aria-checked={item.checked}
                      data-checked={item.checked || undefined}
                      onClick={() => {
                        if (item.disabled) return;
                        onSelect(item.value);
                        setOpen(false);
                      }}
                    >
                      <span className="lf-dropdown-check">{item.checked ? <Check /> : null}</span>
                      {item.leading && <span className="lf-dropdown-leading">{item.leading}</span>}
                      <span className="lf-dropdown-text">{item.label}</span>
                      {item.shortcut && (
                        <span className="lf-dropdown-shortcut">{item.shortcut}</span>
                      )}
                    </button>
                    {item.onDestructiveAction && (
                      <button
                        aria-label={item.destructiveActionLabel ?? "删除"}
                        className="lf-dropdown-delete"
                        type="button"
                        onClick={(event) => {
                          event.stopPropagation();
                          item.onDestructiveAction?.(item.value);
                        }}
                      >
                        <Trash2 />
                      </button>
                    )}
                  </div>
                ))}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}

export function SelectField<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (value: T) => void;
}) {
  const selected = options.find((option) => option.value === value);
  return (
    <label className="lf-form-field">
      <span>{label}</span>
      <DropdownMenu
        className="lf-select-field"
        groups={[
          {
            items: options.map((option) => ({
              value: option.value,
              label: option.label,
              checked: option.value === value,
            })),
          },
        ]}
        menuLabel={label}
        onSelect={onChange}
        trigger={() => (
          <>
            <span>{selected?.label ?? value}</span>
            <ChevronDown />
          </>
        )}
      />
    </label>
  );
}

export function ColorSelect<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (value: T) => void;
}) {
  const selected = options.find((option) => option.value === value);
  return (
    <DropdownMenu
      className="lf-color-dropdown"
      groups={[
        {
          items: options.map((option) => ({
            value: option.value,
            label: option.label,
            checked: option.value === value,
            leading: <span className="lf-color-swatch" data-color={option.value} />,
          })),
        },
      ]}
      menuLabel="高亮颜色"
      onSelect={onChange}
      trigger={() => (
        <>
          <span>{selected?.label ?? value}</span>
          <span className="lf-color-swatch" data-color={value} />
          <ChevronDown />
        </>
      )}
    />
  );
}
