import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../../lib/utils";

export const badgeVariants = cva(
  "inline-flex items-center gap-1 font-medium transition-colors select-none",
  {
    variants: {
      variant: {
        default:
          "bg-canvas-surface-strong text-ink border border-hairline",
        secondary:
          "bg-canvas-surface-card text-ink-secondary border border-hairline",
        outline:
          "border border-hairline text-ink-secondary bg-transparent",
        coral:
          "bg-coral-soft/50 text-coral-dark border border-coral/20",
        success:
          "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400 border border-emerald-500/20",
        warning:
          "bg-amber-500/10 text-amber-700 dark:text-amber-400 border border-amber-500/20",
        danger:
          "bg-rose-500/10 text-rose-700 dark:text-rose-400 border border-rose-500/20",
        neutral:
          "bg-stone-500/10 text-stone-600 dark:text-stone-400 border border-stone-500/15",
      },
      size: {
        sm: "px-1.5 py-0.5 text-[10px] rounded",
        md: "px-2 py-0.5 text-[11px] rounded-md",
        lg: "px-2.5 py-1 text-xs rounded-md",
        pill: "px-2.5 py-0.5 text-[11px] rounded-full",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "md",
    },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, size, ...props }: BadgeProps) {
  return (
    <span className={cn(badgeVariants({ variant, size }), className)} {...props} />
  );
}
