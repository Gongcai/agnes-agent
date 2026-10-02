interface AppLogoProps {
  size?: number;
  className?: string;
}

/** Decorative brand mark; the surrounding text supplies its accessible name. */
export function AppLogo({ size = 28, className = "" }: AppLogoProps) {
  return (
    <img
      src="/logo.png"
      alt=""
      aria-hidden="true"
      width={size}
      height={size}
      draggable={false}
      className={`shrink-0 object-contain ${className}`}
    />
  );
}
