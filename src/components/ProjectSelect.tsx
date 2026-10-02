import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { Check, CaretDown } from "@phosphor-icons/react";

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
  menuBoundarySelector?: string;
}

export function ProjectSelect<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
  className = "",
  disabled = false,
  menuBoundarySelector = ".agnes-settings-content",
}: ProjectSelectProps<T>) {
  const id = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [openUpward, setOpenUpward] = useState(false);
  const [menuMaxHeight, setMenuMaxHeight] = useState(264);
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

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      if (!rootRef.current) return;
      const triggerRect = rootRef.current.getBoundingClientRect();
      const boundaryRect = rootRef.current
        .closest(menuBoundarySelector)
        ?.getBoundingClientRect();
      const viewport = window.visualViewport;
      const viewportTop = viewport?.offsetTop ?? 0;
      const viewportBottom = viewportTop + (viewport?.height ?? window.innerHeight);
      const boundaryTop = Math.max(boundaryRect?.top ?? viewportTop, viewportTop);
      const boundaryBottom = Math.min(boundaryRect?.bottom ?? viewportBottom, viewportBottom);
      const requiredHeight = Math.min(options.length * 44 + 12, 264);
      const spaceAbove = triggerRect.top - boundaryTop;
      const spaceBelow = boundaryBottom - triggerRect.bottom;
      const upward = spaceBelow < requiredHeight && spaceAbove > spaceBelow;
      setOpenUpward(upward);
      setMenuMaxHeight(Math.min(264, Math.max(0, (upward ? spaceAbove : spaceBelow) - 8)));
    };
    place();
    const observer = new ResizeObserver(place);
    if (rootRef.current) observer.observe(rootRef.current);
    const boundary = rootRef.current?.closest(menuBoundarySelector);
    if (boundary) observer.observe(boundary);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    window.visualViewport?.addEventListener("resize", place);
    window.visualViewport?.addEventListener("scroll", place);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      window.visualViewport?.removeEventListener("resize", place);
      window.visualViewport?.removeEventListener("scroll", place);
    };
  }, [open, options.length, menuBoundarySelector]);

  useEffect(() => {
    if (open && activeIndex >= 0) {
      menuRef.current?.querySelector<HTMLElement>(`[data-index="${activeIndex}"]`)?.scrollIntoView({ block: "nearest" });
    }
  }, [open, activeIndex]);

  const toggle = () => {
    if (disabled || options.length === 0) return;
    setActiveIndex(open ? -1 : Math.max(0, options.findIndex((option) => option.value === value)));
    setOpen((current) => !current);
  };

  const choose = (option: ProjectSelectOption<T>) => {
    onChange(option.value);
    setOpen(false);
    setActiveIndex(-1);
    rootRef.current?.querySelector<HTMLButtonElement>(".agnes-project-select-trigger")?.focus({ preventScroll: true });
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (disabled || event.nativeEvent.isComposing) return;
    if (event.key === "Escape" && open) {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      setActiveIndex(-1);
      rootRef.current?.querySelector<HTMLButtonElement>(".agnes-project-select-trigger")?.focus({ preventScroll: true });
      return;
    }
    if (event.key === "Tab") {
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
    if (open && (event.key === "Home" || event.key === "End")) {
      event.preventDefault();
      setActiveIndex(event.key === "Home" ? 0 : options.length - 1);
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    if (options.length === 0) return;
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
    <div
      ref={rootRef}
      className={`agnes-project-select relative ${className}`}
      onKeyDown={handleKeyDown}
      onBlur={(event) => {
        if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
          setActiveIndex(-1);
        }
      }}
    >
      <button
        type="button"
        className="agnes-project-select-trigger flex w-full items-center justify-between gap-3 rounded-lg border border-stone-200 bg-white px-3 py-2.5 text-left text-xs font-normal text-stone-800"
        aria-label={ariaLabel}
        role="combobox"
        aria-autocomplete="none"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? `${id}-listbox` : undefined}
        aria-activedescendant={open && activeIndex >= 0 ? `${id}-option-${activeIndex}` : undefined}
        data-open={open}
        disabled={disabled}
        onClick={toggle}
      >
        <span className="min-w-0 flex-1 truncate">{selected?.label ?? value}</span>
        <CaretDown className="h-3.5 w-3.5 shrink-0 text-stone-400 transition-transform duration-150" aria-hidden="true" />
      </button>

      {open && (
        <div
          ref={menuRef}
          id={`${id}-listbox`}
          role="listbox"
          aria-label={ariaLabel}
          className={`agnes-project-select-menu claude-popover absolute left-0 z-50 max-h-64 w-full overflow-y-auto overscroll-contain rounded-lg border border-stone-200 bg-white p-1 shadow-xl ${
            openUpward ? "bottom-full mb-1" : "top-full mt-1"
          }`}
          data-direction={openUpward ? "up" : "down"}
          style={{ maxHeight: menuMaxHeight }}
        >
          {options.map((option, index) => (
            <button
              key={option.value}
              type="button"
              role="option"
              id={`${id}-option-${index}`}
              tabIndex={-1}
              aria-selected={option.value === value}
              data-index={index}
              data-active={activeIndex === index}
              className="agnes-project-select-option flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left text-xs text-stone-700"
              onPointerMove={() => setActiveIndex(index)}
              onPointerLeave={() => setActiveIndex(-1)}
              onClick={() => choose(option)}
            >
              <span className="min-w-0 flex-1 truncate" title={option.label}>{option.label}</span>
              {option.value === value && <Check className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
