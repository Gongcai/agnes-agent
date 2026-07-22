import { describe, expect, it } from "vitest";
import { getGreetingPeriod, selectConversationGreeting } from "./greetings";

function localDate(hour: number): Date {
  return new Date(2026, 6, 22, hour, 0, 0);
}

describe("conversation greetings", () => {
  it("uses the expected local-time boundaries", () => {
    expect(getGreetingPeriod(4)).toBe("late_night");
    expect(getGreetingPeriod(5)).toBe("morning");
    expect(getGreetingPeriod(11)).toBe("morning");
    expect(getGreetingPeriod(12)).toBe("afternoon");
    expect(getGreetingPeriod(17)).toBe("afternoon");
    expect(getGreetingPeriod(18)).toBe("evening");
    expect(getGreetingPeriod(22)).toBe("evening");
    expect(getGreetingPeriod(23)).toBe("late_night");
  });

  it("keeps the greeting stable for the same conversation", () => {
    const first = selectConversationGreeting(" Caiwen ", localDate(20), "session-1");
    const second = selectConversationGreeting("Caiwen", localDate(20), "session-1");

    expect(first).toBe(second);
    expect(first).not.toContain("{{user}}");
  });

  it("offers time-specific and general short greetings", () => {
    const evening = new Set<string>();
    const lateNight = new Set<string>();
    for (let index = 0; index < 80; index += 1) {
      evening.add(selectConversationGreeting("Caiwen", localDate(20), `session-${index}`));
      lateNight.add(selectConversationGreeting("Caiwen", localDate(1), `session-${index}`));
    }

    expect(evening).toContain("Good evening, Caiwen.");
    expect(evening).toContain("Caiwen returns!");
    expect(lateNight).toContain("Hi, night owl.");
    expect([...evening, ...lateNight].every((greeting) => greeting.length < 48)).toBe(true);
  });

  it("uses natural fallbacks when no user name is configured", () => {
    const greetings = Array.from({ length: 24 }, (_, index) => (
      selectConversationGreeting("", localDate(8), `session-${index}`)
    ));

    expect(greetings.some((greeting) => greeting === "Good morning.")).toBe(true);
    expect(greetings.every((greeting) => !greeting.includes("{{user}}"))).toBe(true);
  });
});
