import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../../lib/utils";

export const buttonVariants = cva(
  "inline-flex items-center justify-center rounded-md font-medium transition-all duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-clay/40 focus-visible:ring-offset-1 disabled:pointer-events-none disabled:opacity-40 select-none active:scale-[0.98]",
  {
    variants: {
      variant: {
        default:
          "bg-ink text-white hover:opacity-90 shadow-subtle",
        primary:
          "bg-coral text-white hover:bg-coral-dark shadow-subtle",
        secondary:
          "bg-canvas-surface-card text-ink hover:bg-canvas-surface-strong border border-hairline",
        outline:
          "border border-hairline-strong bg-transparent text-ink hover:bg-canvas-surface-soft",
        ghost:
          "text-ink-secondary hover:bg-canvas-surface-soft hover:text-ink",
        danger:
          "bg-red-600 text-white hover:bg-red-700 shadow-subtle",
        dangerGhost:
          "text-red-600 hover:bg-red-50 hover:text-red-700 dark:hover:bg-red-950/30",
        subtle:
          "bg-canvas-surface text-ink-secondary hover:bg-canvas-surface-card border border-hairline shadow-subtle",
      },
      size: {
        default: "h-9 px-4 py-2 text-xs",
        sm: "h-7 px-2.5 text-[11px] rounded",
        lg: "h-11 px-6 text-sm rounded-lg",
        icon: "h-8 w-8 p-0 rounded-md",
        "icon-sm": "h-6 w-6 p-0 rounded",
        "icon-xs": "h-5 w-5 p-0 rounded",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, type, ...props }, ref) => (
    <button
      ref={ref}
      type={type ?? "button"}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  ),
);
Button.displayName = "Button";

