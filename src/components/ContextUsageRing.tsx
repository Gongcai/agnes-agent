import React from "react";

interface ContextUsageRingProps {
  usedTokens: number;
  limitTokens: number;
  warningThreshold?: number;
}

export function formatCompactTokens(tokens: number): string {
  const safeTokens = Math.max(0, Math.round(tokens));
  if (safeTokens < 1_000) return String(safeTokens);
  if (safeTokens < 1_000_000) {
    const value = safeTokens / 1_000;
    return `${value >= 100 ? value.toFixed(0) : value.toFixed(1).replace(/\.0$/, "")}K`;
  }
  const value = safeTokens / 1_000_000;
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1).replace(/\.0$/, "")}M`;
}

export const ContextUsageRing: React.FC<ContextUsageRingProps> = ({
  usedTokens,
  limitTokens,
  warningThreshold = 0.85,
}) => {
  const safeUsed = Math.max(0, usedTokens);
  const safeLimit = Math.max(1, limitTokens);
  const ratio = safeUsed / safeLimit;
  const visibleRatio = Math.min(1, ratio);
  const radius = 6;
  const circumference = 2 * Math.PI * radius;
  const status = ratio >= 1 ? "danger" : ratio >= warningThreshold ? "warning" : "normal";
  const percentage = Math.round(ratio * 100);
  const label = `上下文 ${formatCompactTokens(safeUsed)} / ${formatCompactTokens(safeLimit)}（${percentage}%）`;

  return (
    <span
      className="agnes-context-usage-ring"
      data-status={status}
      role="img"
      aria-label={label}
      title={label}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <circle className="agnes-context-ring-track" cx="8" cy="8" r={radius} />
        <circle
          className="agnes-context-ring-value"
          cx="8"
          cy="8"
          r={radius}
          style={{
            strokeDasharray: circumference,
            strokeDashoffset: circumference * (1 - visibleRatio),
          }}
        />
      </svg>
    </span>
  );
};
