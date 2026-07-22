import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Check, ChevronDown } from "lucide-react";

export interface ProjectSelectOption<T extends string = string> {
  value: T;
  label: string;
}

interface ProjectSelectProps<T extends string> {
  value: T;
  options: readonly ProjectSelectOption<T>[];
  onChange: (value: T) => void;
  ariaLabel: string;
  className?: string;
  disabled?: boolean;
}

export function ProjectSelect<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
  className = "",
  disabled = false,
}: ProjectSelectProps<T>) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [openUpward, setOpenUpward] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const selected = options.find((option) => option.value === value) ?? options[0];

  useEffect(() => {
    if (!open) return;
    const closeOnOutsidePress = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
        setActiveIndex(-1);
      }
    };
    document.addEventListener("pointerdown", closeOnOutsidePress);
    return () => document.removeEventListener("pointerdown", closeOnOutsidePress);
  }, [open]);

  useEffect(() => {
    if (disabled) {
      setOpen(false);
      setActiveIndex(-1);
    }
  }, [disabled]);

  const toggle = () => {
    if (disabled) return;
    if (!open && rootRef.current) {
      const triggerRect = rootRef.current.getBoundingClientRect();
      const boundaryRect = rootRef.current
        .closest(".agnes-settings-content")
        ?.getBoundingClientRect();
      const boundaryTop = boundaryRect?.top ?? 0;
      const boundaryBottom = boundaryRect?.bottom ?? window.innerHeight;
      const requiredHeight = Math.min(options.length * 34 + 12, 264);
      const spaceAbove = triggerRect.top - boundaryTop;
      const spaceBelow = boundaryBottom - triggerRect.bottom;
      setOpenUpward(spaceBelow < requiredHeight && spaceAbove > spaceBelow);
      setActiveIndex(-1);
    }
    setOpen((current) => !current);
  };

  const choose = (option: ProjectSelectOption<T>) => {
    onChange(option.value);
    setOpen(false);
    setActiveIndex(-1);
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "Escape") {
      setOpen(false);
      setActiveIndex(-1);
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (open && activeIndex >= 0) {
        choose(options[activeIndex]);
      } else {
        toggle();
      }
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    if (!open) {
      toggle();
      const selectedIndex = options.findIndex((option) => option.value === value);
      setActiveIndex(selectedIndex >= 0 ? selectedIndex : 0);
      return;
    }
    const direction = event.key === "ArrowDown" ? 1 : -1;
    setActiveIndex((current) => {
      if (current < 0) {
        const selectedIndex = options.findIndex((option) => option.value === value);
        return selectedIndex >= 0 ? selectedIndex : 0;
      }
      return (current + direction + options.length) % options.length;
    });
  };

  return (
    <div ref={rootRef} className={`agnes-project-select relative ${className}`}>
      <button
        type="button"
        className="agnes-project-select-trigger flex w-full items-center justify-between gap-3 rounded-lg border border-stone-200 bg-white px-3 py-2.5 text-left text-xs font-normal text-stone-800"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        data-open={open}
        disabled={disabled}
        onClick={toggle}
        onKeyDown={handleKeyDown}
      >
        <span className="min-w-0 flex-1 truncate">{selected?.label ?? value}</span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0 text-stone-400 transition-transform duration-150" />
      </button>

      {open && (
        <div
          role="listbox"
          aria-label={ariaLabel}
          className={`agnes-project-select-menu claude-popover absolute left-0 z-50 max-h-64 min-w-full overflow-y-auto rounded-lg border border-stone-200 bg-white p-1 shadow-xl ${
            openUpward ? "bottom-full mb-1" : "top-full mt-1"
          }`}
          data-direction={openUpward ? "up" : "down"}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              setOpen(false);
              setActiveIndex(-1);
              rootRef.current?.querySelector<HTMLButtonElement>(".agnes-project-select-trigger")?.focus();
            }
          }}
        >
          {options.map((option, index) => (
            <button
              key={option.value}
              type="button"
              role="option"
              aria-selected={option.value === value}
              data-active={activeIndex === index}
              className="agnes-project-select-option flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left text-xs text-stone-700"
              onPointerMove={() => setActiveIndex(index)}
              onPointerLeave={() => setActiveIndex(-1)}
              onClick={() => choose(option)}
            >
              <span className="min-w-0 flex-1 truncate">{option.label}</span>
              {option.value === value && <Check className="h-3.5 w-3.5 shrink-0" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
