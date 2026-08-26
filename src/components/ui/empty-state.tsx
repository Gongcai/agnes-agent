import * as React from "react";
import { cn } from "../../lib/utils";

export interface EmptyStateProps extends React.HTMLAttributes<HTMLDivElement> {
  icon?: React.ReactNode;
  title: string;
  description?: string;
  action?: React.ReactNode;
}

export function EmptyState({
  icon,
  title,
  description,
  action,
  className,
  ...props
}: EmptyStateProps) {
  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center p-8 text-center animate-fade-in",
        className
      )}
      {...props}
    >
      {icon && (
        <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-xl bg-canvas-surface-card border border-hairline text-ink-secondary shadow-subtle">
          {icon}
        </div>
      )}
      <h4 className="text-sm font-medium text-ink tracking-tight">{title}</h4>
      {description && (
        <p className="mt-1 max-w-sm text-xs text-ink-muted leading-relaxed">
          {description}
        </p>
      )}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}
