import React, { useEffect, useState } from "react";
import { cn } from "../lib/utils";

interface AnimatedDisclosureProps extends React.HTMLAttributes<HTMLDivElement> {
  defaultOpen?: boolean;
  open?: boolean;
  summary: React.ReactNode;
  summaryClassName?: string;
  onOpenChange?: (open: boolean) => void;
}

export const AnimatedDisclosure: React.FC<AnimatedDisclosureProps> = ({
  defaultOpen = false,
  open: controlledOpen,
  summary,
  summaryClassName,
  onOpenChange,
  children,
  className,
  ...props
}) => {
  const [internalOpen, setInternalOpen] = useState(defaultOpen);
  const open = controlledOpen ?? internalOpen;

  useEffect(() => {
    if (controlledOpen === undefined) setInternalOpen(defaultOpen);
  }, [controlledOpen, defaultOpen]);

  const toggle = () => {
    const nextOpen = !open;
    if (controlledOpen === undefined) setInternalOpen(nextOpen);
    onOpenChange?.(nextOpen);
  };

  return (
    <div
      {...props}
      className={cn("agnes-animated-disclosure", className)}
      data-expanded={open}
    >
      <button
        type="button"
        className={summaryClassName}
        aria-expanded={open}
        onClick={toggle}
      >
        {summary}
      </button>
      <div
        className="agnes-disclosure-panel"
        aria-hidden={!open}
      >
        <div className="agnes-disclosure-panel-inner">{children}</div>
      </div>
    </div>
  );
};
