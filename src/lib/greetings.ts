export type GreetingPeriod = "morning" | "afternoon" | "evening" | "late_night";

const USER_TEMPLATES: Record<GreetingPeriod, readonly string[]> = {
  morning: [
    "Good morning, {{user}}.",
    "Morning, {{user}}.",
    "Ready for a fresh start, {{user}}?",
    "What's first, {{user}}?",
    "New day, new ideas.",
  ],
  afternoon: [
    "Good afternoon, {{user}}.",
    "What's next, {{user}}?",
    "Ready when you are, {{user}}.",
    "Anything on your mind, {{user}}?",
    "Let's keep moving, {{user}}.",
  ],
  evening: [
    "Good evening, {{user}}.",
    "Evening, {{user}}.",
    "What's on your mind, {{user}}?",
    "One more thing, {{user}}?",
    "Let's wrap something up, {{user}}.",
  ],
  late_night: [
    "Hi, night owl.",
    "Still awake, {{user}}?",
    "Late-night idea, {{user}}?",
    "Quiet hours, clear thoughts.",
    "Midnight mode, {{user}}.",
  ],
};

const GENERAL_USER_TEMPLATES = [
  "{{user}} returns!",
  "Anything, {{user}}?",
  "Ready, {{user}}?",
] as const;

const CJK_USER_TEMPLATES: Record<GreetingPeriod, readonly string[]> = {
  morning: [
    "早上好，{{user}}",
    "早安，{{user}}",
    "今天想推进点什么，{{user}}？",
    "新的一天，从哪里开始？",
  ],
  afternoon: [
    "下午好，{{user}}",
    "接下来做点什么，{{user}}？",
    "有什么新想法，{{user}}？",
    "随时准备就绪，{{user}}",
  ],
  evening: [
    "晚上好，{{user}}",
    "此刻在想什么，{{user}}？",
    "今天还有什么想收尾的吗，{{user}}？",
    "晚安前再聊点什么，{{user}}？",
  ],
  late_night: [
    "夜深了，{{user}}",
    "还在忙碌吗，{{user}}？",
    "深夜灵感时刻，{{user}}",
    "夜静思明，想聊点什么？",
  ],
};

const CJK_GENERAL_USER_TEMPLATES = [
  "欢迎回来，{{user}}",
  "今天想构建点什么，{{user}}？",
] as const;

const ANONYMOUS_TEMPLATES: Record<GreetingPeriod, readonly string[]> = {
  morning: ["Good morning.", "A fresh start.", "What's first?"],
  afternoon: ["Good afternoon.", "What's next?", "Keep it moving."],
  evening: ["Good evening.", "One more thing?", "What's on your mind?"],
  late_night: ["Hi, night owl.", "Still awake?", "Quiet hours, clear thoughts."],
};

const GENERAL_ANONYMOUS_TEMPLATES = [
  "Welcome back!",
  "Anything on your mind?",
  "Ready when you are.",
] as const;

export function getGreetingPeriod(hour: number): GreetingPeriod {
  if (hour >= 5 && hour < 12) return "morning";
  if (hour >= 12 && hour < 18) return "afternoon";
  if (hour >= 18 && hour < 23) return "evening";
  return "late_night";
}

function stableHash(value: string): number {
  let hash = 2_166_136_261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return hash >>> 0;
}

function localDateKey(date: Date): string {
  return `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`;
}

function containsCjk(value: string): boolean {
  return /[\u3400-\u9fff\uf900-\ufaff]/.test(value);
}

export function selectConversationGreeting(userName: string, date: Date, seed: string): string {
  const name = userName.trim();
  const period = getGreetingPeriod(date.getHours());
  const useCjk = containsCjk(name);
  const templates = name
    ? useCjk
      ? [...CJK_USER_TEMPLATES[period], ...CJK_GENERAL_USER_TEMPLATES]
      : [...USER_TEMPLATES[period], ...GENERAL_USER_TEMPLATES]
    : [...ANONYMOUS_TEMPLATES[period], ...GENERAL_ANONYMOUS_TEMPLATES];
  const selectionKey = `${localDateKey(date)}:${period}:${seed}:${name}`;
  const template = templates[stableHash(selectionKey) % templates.length];
  return name ? template.split("{{user}}").join(name) : template;
}
