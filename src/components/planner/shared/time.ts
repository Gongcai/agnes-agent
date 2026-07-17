import { DateTime } from "luxon";
import type { RepeatOption } from "./types";

export const localTimezone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";

export const commonTimezones = Array.from(
  new Set([
    localTimezone,
    "UTC",
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Europe/London",
    "Europe/Paris",
    "America/New_York",
    "America/Los_Angeles",
  ]),
);

export function todayKey(): string {
  return DateTime.local().toISODate() ?? "";
}

export function tomorrowKey(): string {
  return DateTime.local().plus({ days: 1 }).toISODate() ?? "";
}

export function isoToDateKey(iso: string, timezone = localTimezone): string {
  return DateTime.fromISO(iso, { setZone: true }).setZone(timezone).toISODate() ?? "";
}

export function isoToDateTimeInput(iso: string, timezone = localTimezone): string {
  return DateTime.fromISO(iso, { setZone: true })
    .setZone(timezone)
    .toFormat("yyyy-MM-dd'T'HH:mm");
}

export function dateTimeInputToIso(value: string, timezone = localTimezone): string {
  const dateTime = DateTime.fromFormat(value, "yyyy-MM-dd'T'HH:mm", { zone: timezone });
  if (!dateTime.isValid) throw new Error("日期或时间无效");
  const iso = dateTime.toUTC().toISO({ suppressMilliseconds: true });
  if (!iso) throw new Error("无法转换日期或时间");
  return iso;
}

export function allDayDateToIso(value: string, timezone = localTimezone): string {
  const dateTime = DateTime.fromISO(value, { zone: timezone }).startOf("day");
  if (!dateTime.isValid) throw new Error("日期无效");
  const iso = dateTime.toUTC().toISO({ suppressMilliseconds: true });
  if (!iso) throw new Error("无法转换日期");
  return iso;
}

export function inclusiveEndDateToIso(value: string, timezone = localTimezone): string {
  const dateTime = DateTime.fromISO(value, { zone: timezone }).plus({ days: 1 }).startOf("day");
  if (!dateTime.isValid) throw new Error("结束日期无效");
  const iso = dateTime.toUTC().toISO({ suppressMilliseconds: true });
  if (!iso) throw new Error("无法转换结束日期");
  return iso;
}

export function exclusiveEndIsoToDateKey(iso: string, timezone = localTimezone): string {
  return DateTime.fromISO(iso, { setZone: true })
    .setZone(timezone)
    .minus({ days: 1 })
    .toISODate() ?? "";
}

export function formatDateKey(value: string): string {
  const date = DateTime.fromISO(value);
  if (!date.isValid) return value;
  return date.setLocale("zh-CN").toFormat("M月d日 cccc");
}

export function formatDateTime(iso: string, timezone = localTimezone): string {
  const date = DateTime.fromISO(iso, { setZone: true }).setZone(timezone);
  if (!date.isValid) return iso;
  return date.setLocale("zh-CN").toFormat("M月d日 ccc HH:mm");
}

export function eventOccursOnDate(
  startsAt: string,
  endsAt: string,
  allDay: boolean,
  dateKey: string,
  timezone = localTimezone,
): boolean {
  const dayStart = DateTime.fromISO(dateKey, { zone: timezone }).startOf("day");
  const dayEnd = dayStart.plus({ days: 1 });
  const start = DateTime.fromISO(startsAt, { setZone: true }).setZone(timezone);
  const end = DateTime.fromISO(endsAt, { setZone: true }).setZone(timezone);
  if (!start.isValid || !end.isValid || !dayStart.isValid) return false;
  if (allDay) return start.startOf("day") < dayEnd && end > dayStart;
  return start < dayEnd && end > dayStart;
}

export function repeatOptionFromRule(rule: string | null): RepeatOption {
  if (!rule) return "none";
  if (rule === "RRULE:FREQ=DAILY") return "daily";
  if (rule === "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR") return "weekdays";
  if (rule === "RRULE:FREQ=WEEKLY") return "weekly";
  if (rule === "RRULE:FREQ=MONTHLY") return "monthly";
  if (rule === "RRULE:FREQ=YEARLY") return "yearly";
  return "custom";
}

export function repeatRuleFromOption(option: RepeatOption, currentRule: string | null): string | null {
  if (option === "none") return null;
  if (option === "daily") return "RRULE:FREQ=DAILY";
  if (option === "weekdays") return "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR";
  if (option === "weekly") return "RRULE:FREQ=WEEKLY";
  if (option === "monthly") return "RRULE:FREQ=MONTHLY";
  if (option === "yearly") return "RRULE:FREQ=YEARLY";
  return currentRule;
}

export function repeatLabel(rule: string | null): string | null {
  const labels: Record<RepeatOption, string> = {
    none: "不重复",
    daily: "每天",
    weekdays: "每个工作日",
    weekly: "每周",
    monthly: "每月",
    yearly: "每年",
    custom: "自定义重复",
  };
  return rule ? labels[repeatOptionFromRule(rule)] : null;
}
