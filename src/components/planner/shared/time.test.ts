import { describe, expect, it } from "vitest";
import {
  allDayDateToIso,
  dateTimeInputToIso,
  eventOccursOnDate,
  exclusiveEndIsoToDateKey,
  inclusiveEndDateToIso,
  repeatOptionFromRule,
  repeatRuleFromOption,
} from "./time";

describe("planner time helpers", () => {
  it("converts wall-clock input through its IANA timezone", () => {
    expect(dateTimeInputToIso("2026-07-17T09:00", "Asia/Shanghai")).toBe(
      "2026-07-17T01:00:00Z",
    );
    expect(dateTimeInputToIso("2026-03-08T09:00", "America/New_York")).toBe(
      "2026-03-08T13:00:00Z",
    );
  });

  it("stores all-day ranges with an exclusive end instant", () => {
    expect(allDayDateToIso("2026-07-17", "Asia/Shanghai")).toBe("2026-07-16T16:00:00Z");
    const end = inclusiveEndDateToIso("2026-07-18", "Asia/Shanghai");
    expect(end).toBe("2026-07-18T16:00:00Z");
    expect(exclusiveEndIsoToDateKey(end, "Asia/Shanghai")).toBe("2026-07-18");
  });

  it("matches timed and all-day events to agenda dates", () => {
    expect(
      eventOccursOnDate(
        "2026-07-17T15:30:00Z",
        "2026-07-17T16:30:00Z",
        false,
        "2026-07-18",
        "Asia/Shanghai",
      ),
    ).toBe(true);
    expect(
      eventOccursOnDate(
        "2026-07-16T16:00:00Z",
        "2026-07-18T16:00:00Z",
        true,
        "2026-07-18",
        "Asia/Shanghai",
      ),
    ).toBe(true);
  });

  it("maps common recurrence choices without exposing raw RRULE input", () => {
    expect(repeatOptionFromRule("RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR")).toBe("weekdays");
    expect(repeatRuleFromOption("monthly", null)).toBe("RRULE:FREQ=MONTHLY");
    expect(repeatRuleFromOption("custom", "RRULE:FREQ=DAILY;COUNT=3")).toBe(
      "RRULE:FREQ=DAILY;COUNT=3",
    );
  });
});
