import React from "react";

export interface TokenUsageDay {
  date: string;
  input_tokens: number;
  cached_tokens: number;
  output_tokens: number;
  total_tokens: number;
}

interface TokenUsageCalendarProps {
  days: TokenUsageDay[];
  today?: Date;
}

const WEEK_COUNT = 53;
const DAYS_PER_WEEK = 7;
const CELL_SIZE = 9;
const CELL_GAP = 3;
const INTENSITY_CLASSES = [
  "bg-stone-100",
  "bg-emerald-200",
  "bg-emerald-400",
  "bg-emerald-600",
  "bg-emerald-800",
] as const;

function atLocalNoon(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate(), 12);
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

function toDateKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function formatCalendarDate(date: Date): string {
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

export function getTokenIntensity(value: number, maximum: number): number {
  if (value <= 0 || maximum <= 0) return 0;
  const normalized = Math.log1p(value) / Math.log1p(maximum);
  return Math.max(1, Math.min(4, Math.ceil(normalized * 4)));
}

export const TokenUsageCalendar: React.FC<TokenUsageCalendarProps> = ({
  days,
  today = new Date(),
}) => {
  const currentDay = atLocalNoon(today);
  const currentWeekStart = addDays(currentDay, -currentDay.getDay());
  const gridStart = addDays(currentWeekStart, -(WEEK_COUNT - 1) * DAYS_PER_WEEK);
  const usageByDate = new Map(days.map((day) => [day.date, day]));
  const cells = Array.from({ length: WEEK_COUNT * DAYS_PER_WEEK }, (_, index) => {
    const date = addDays(gridStart, index);
    const usage = usageByDate.get(toDateKey(date));
    return {
      date,
      isFuture: date.getTime() > currentDay.getTime(),
      usage,
      total: Math.max(0, usage?.total_tokens ?? 0),
    };
  });
  const visibleCells = cells.filter((cell) => !cell.isFuture);
  const maximum = Math.max(0, ...visibleCells.map((cell) => cell.total));
  const activeDays = visibleCells.filter((cell) => cell.total > 0).length;
  const visibleTotal = visibleCells.reduce((sum, cell) => sum + cell.total, 0);
  const monthLabels = Array.from({ length: WEEK_COUNT }, (_, weekIndex) => {
    const week = cells.slice(weekIndex * DAYS_PER_WEEK, (weekIndex + 1) * DAYS_PER_WEEK);
    const monthStart = week.find((cell) => cell.date.getDate() === 1);
    if (monthStart) return `${monthStart.date.getMonth() + 1}月`;
    return weekIndex === 0 ? `${week[0].date.getMonth() + 1}月` : "";
  });
  const graphWidth = WEEK_COUNT * CELL_SIZE + (WEEK_COUNT - 1) * CELL_GAP;

  return (
    <section className="rounded-md border border-stone-200 bg-white px-3 py-3.5" aria-labelledby="token-activity-title">
      <div className="mb-3 flex items-baseline justify-between gap-3">
        <h4 id="token-activity-title" className="text-xs font-semibold text-stone-700">
          近一年 Token 活跃度
        </h4>
        <span className="font-mono text-[10px] tabular-nums text-stone-400">
          {activeDays} 个活跃日 · {visibleTotal.toLocaleString("zh-CN")} Token
        </span>
      </div>

      <div className="overflow-x-auto pb-1">
        <div className="w-max min-w-full">
          <div
            className="ml-8 grid text-[9px] leading-3 text-stone-400"
            style={{
              gridTemplateColumns: `repeat(${WEEK_COUNT}, ${CELL_SIZE}px)`,
              columnGap: `${CELL_GAP}px`,
              width: `${graphWidth}px`,
            }}
            aria-hidden="true"
          >
            {monthLabels.map((label, index) => (
              <span key={`${label}-${index}`} className="h-3 whitespace-nowrap">
                {label}
              </span>
            ))}
          </div>

          <div className="mt-1.5 grid grid-cols-[24px_auto] gap-2">
            <div
              className="grid text-[9px] leading-none text-stone-400"
              style={{
                gridTemplateRows: `repeat(${DAYS_PER_WEEK}, ${CELL_SIZE}px)`,
                rowGap: `${CELL_GAP}px`,
              }}
              aria-hidden="true"
            >
              {["", "一", "", "三", "", "五", ""].map((label, index) => (
                <span key={`${label}-${index}`} className="flex items-center justify-end">
                  {label}
                </span>
              ))}
            </div>

            <div
              role="grid"
              aria-label="近一年每日 Token 使用量"
              className="grid grid-flow-col"
              style={{
                gridTemplateColumns: `repeat(${WEEK_COUNT}, ${CELL_SIZE}px)`,
                gridTemplateRows: `repeat(${DAYS_PER_WEEK}, ${CELL_SIZE}px)`,
                gap: `${CELL_GAP}px`,
                width: `${graphWidth}px`,
              }}
            >
              {cells.map((cell) => {
                if (cell.isFuture) {
                  return <span key={toDateKey(cell.date)} aria-hidden="true" />;
                }
                const intensity = getTokenIntensity(cell.total, maximum);
                const usage = cell.usage;
                const title = usage
                  ? `${formatCalendarDate(cell.date)}：${cell.total.toLocaleString("zh-CN")} Token\n输入 ${usage.input_tokens.toLocaleString("zh-CN")} · 缓存 ${usage.cached_tokens.toLocaleString("zh-CN")} · 输出 ${usage.output_tokens.toLocaleString("zh-CN")}`
                  : `${formatCalendarDate(cell.date)}：0 Token`;
                return (
                  <span
                    key={toDateKey(cell.date)}
                    role="gridcell"
                    aria-label={title.replace("\n", "，")}
                    title={title}
                    className={`rounded-[2px] border border-black/5 ${INTENSITY_CLASSES[intensity]}`}
                  />
                );
              })}
            </div>
          </div>

          <div className="mt-2.5 flex items-center justify-end gap-1.5 text-[9px] text-stone-400" aria-hidden="true">
            <span>少</span>
            {INTENSITY_CLASSES.map((color) => (
              <span key={color} className={`h-[9px] w-[9px] rounded-[2px] border border-black/5 ${color}`} />
            ))}
            <span>多</span>
          </div>
        </div>
      </div>
    </section>
  );
};
