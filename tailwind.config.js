/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: ["class", '[data-agnes-theme="dark"]'],
  theme: {
    extend: {
      colors: {
        canvas: {
          DEFAULT: "var(--claude-canvas)",
          sidebar: "var(--claude-sidebar)",
          surface: "var(--claude-surface)",
          "surface-soft": "var(--claude-surface-soft)",
          "surface-card": "var(--claude-surface-card)",
          "surface-strong": "var(--claude-surface-strong)",
          input: "var(--claude-input)",
        },
        ink: {
          DEFAULT: "var(--claude-ink)",
          body: "var(--claude-body)",
          muted: "var(--claude-muted)",
          "muted-soft": "var(--claude-muted-soft)",
        },
        clay: {
          DEFAULT: "var(--claude-clay)",
          dark: "var(--claude-clay-dark)",
          soft: "var(--claude-clay-soft)",
        },
        coral: {
          DEFAULT: "var(--claude-clay)",
          dark: "var(--claude-clay-dark)",
          soft: "var(--claude-clay-soft)",
        },
        hairline: {
          DEFAULT: "var(--claude-border)",
          strong: "var(--claude-border-strong)",
        },
        theme: {
          success: "var(--claude-success)",
          warning: "var(--claude-warning)",
          danger: "var(--claude-danger)",
          dark: "var(--claude-dark)",
        },
      },
      fontFamily: {
        sans: [
          '"Noto Serif"',
          '"Noto Serif SC"',
          '"Noto Serif CJK SC"',
          '"Source Han Serif SC"',
          '"Songti SC"',
          "Georgia",
          '"Times New Roman"',
          "serif",
        ],
        serif: [
          '"Noto Serif"',
          '"Noto Serif SC"',
          '"Noto Serif CJK SC"',
          '"Source Han Serif SC"',
          '"Songti SC"',
          "Georgia",
          '"Times New Roman"',
          "serif",
        ],
        mono: [
          '"JetBrains Mono"',
          '"SFMono-Regular"',
          "Menlo",
          "Monaco",
          "Consolas",
          '"Liberation Mono"',
          '"Courier New"',
          "monospace",
        ],
      },
      boxShadow: {
        subtle: "0 1px 2px 0 rgba(31, 30, 27, 0.04)",
        card: "0 1px 3px 0 rgba(31, 30, 27, 0.06), 0 1px 2px -1px rgba(31, 30, 27, 0.04)",
        elevated: "0 4px 16px -2px rgba(31, 30, 27, 0.08), 0 2px 6px -2px rgba(31, 30, 27, 0.04)",
        modal: "0 24px 48px -12px rgba(31, 30, 27, 0.22), 0 0 0 1px rgba(31, 30, 27, 0.06)",
        dropdown: "0 10px 25px -5px rgba(31, 30, 27, 0.12), 0 4px 10px -2px rgba(31, 30, 27, 0.06)",
        floating: "0 12px 36px -6px rgba(31, 30, 27, 0.15), 0 0 0 1px rgba(31, 30, 27, 0.05)",
      },
      borderRadius: {
        xs: "4px",
        sm: "6px",
        md: "8px",
        lg: "12px",
        xl: "16px",
        "2xl": "20px",
      },
      animation: {
        "fade-in": "fadeIn 0.2s cubic-bezier(0.16, 1, 0.3, 1)",
        "scale-in": "scaleIn 0.2s cubic-bezier(0.16, 1, 0.3, 1)",
        "slide-down": "slideDown 0.25s cubic-bezier(0.16, 1, 0.3, 1)",
        shimmer: "shimmer 2s linear infinite",
      },
      keyframes: {
        fadeIn: {
          "0%": { opacity: "0" },
          "100%": { opacity: "1" },
        },
        scaleIn: {
          "0%": { opacity: "0", transform: "scale(0.96)" },
          "100%": { opacity: "1", transform: "scale(1)" },
        },
        slideDown: {
          "0%": { opacity: "0", transform: "translateY(-8px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
        shimmer: {
          "0%": { backgroundPosition: "-200% 0" },
          "100%": { backgroundPosition: "200% 0" },
        },
      },
    },
  },
  plugins: [],
};
