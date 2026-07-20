import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import {
  Bell,
  CalendarDots as CalendarDays,
  CheckCircle as CheckCircle2,
  CheckSquare as CheckSquare2,
  CaretRight as ChevronRight,
  ShieldWarning as ShieldAlert,
} from "@phosphor-icons/react";

export interface AppNotification {
  id: string;
  kind: "agent_completed" | "approval_requested" | "task_due" | "event_start";
  title: string;
  body: string | null;
  target_kind: "chat" | "task" | "calendar" | "none";
  target_id: string | null;
  source_kind: string;
  source_id: string;
  scheduled_at: string | null;
  delivered_at: string;
  read_at: string | null;
  created_at: string;
}

interface NotificationCenterProps {
  onNavigate: (notification: AppNotification) => void | Promise<void>;
  className?: string;
}

function notificationIcon(kind: AppNotification["kind"]) {
  if (kind === "approval_requested") return ShieldAlert;
  if (kind === "task_due") return CheckSquare2;
  if (kind === "event_start") return CalendarDays;
  return CheckCircle2;
}

function relativeTime(value: string): string {
  const timestamp = new Date(value).getTime();
  if (Number.isNaN(timestamp)) return "刚刚";
  const seconds = Math.max(0, Math.floor((Date.now() - timestamp) / 1000));
  if (seconds < 60) return "刚刚";
  if (seconds < 3_600) return `${Math.floor(seconds / 60)} 分钟前`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3_600)} 小时前`;
  return new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric" }).format(timestamp);
}

export function NotificationCenter({ onNavigate, className }: NotificationCenterProps) {
  const [open, setOpen] = useState(false);
  const [notifications, setNotifications] = useState<AppNotification[]>([]);
  const [loading, setLoading] = useState(true);
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLElement>(null);

  const load = async () => {
    try {
      const rows = await invoke<AppNotification[]>("list_notifications", { limit: 50 });
      setNotifications(rows);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
    const listeners = Promise.all([
      listen<AppNotification>("notification://created", (event) => {
        setNotifications((current) => [
          event.payload,
          ...current.filter((notification) => notification.id !== event.payload.id),
        ]);
      }),
      listen("notification://changed", () => {
        void load();
      }),
    ]);
    return () => {
      void listeners.then((unsubscribers) => unsubscribers.forEach((unsubscribe) => unsubscribe()));
    };
  }, []);

  const unreadCount = useMemo(
    () => notifications.reduce((count, notification) => count + (notification.read_at ? 0 : 1), 0),
    [notifications],
  );

  const openNotification = async (notification: AppNotification) => {
    if (!notification.read_at) {
      await invoke("mark_notification_read", { notificationId: notification.id });
      setNotifications((current) => current.map((item) =>
        item.id === notification.id ? { ...item, read_at: new Date().toISOString() } : item,
      ));
    }
    setOpen(false);
    await onNavigate(notification);
  };

  const markAllRead = async () => {
    await invoke("mark_all_notifications_read");
    setNotifications((current) => current.map((notification) => ({
      ...notification,
      read_at: notification.read_at ?? new Date().toISOString(),
    })));
  };

  const toggle = () => {
    setOpen((current) => {
      const next = !current;
      if (next) setAnchor(triggerRef.current?.getBoundingClientRect() ?? null);
      return next;
    });
  };

  useEffect(() => {
    if (!open) return;

    const closeOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (triggerRef.current?.contains(target) || popoverRef.current?.contains(target)) return;
      setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };

    document.addEventListener("pointerdown", closeOnOutsidePointer, true);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer, true);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  const popover = open && anchor && (
    <section
      ref={popoverRef}
      className="claude-popover fixed z-[100] flex w-[min(25rem,calc(100vw-1rem))] flex-col overflow-hidden rounded-xl border border-stone-200 bg-white shadow-2xl"
      style={{
        top: Math.min(anchor.bottom + 8, window.innerHeight - 72),
        left: Math.min(Math.max(anchor.left, 8), Math.max(8, window.innerWidth - 408)),
        maxHeight: Math.max(160, window.innerHeight - Math.min(anchor.bottom + 8, window.innerHeight - 72) - 8),
      }}
      aria-label="通知中心内容"
    >
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-stone-200 px-4">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-stone-900">通知</h2>
          {unreadCount > 0 && <span className="text-[11px] text-stone-400">{unreadCount} 条未读</span>}
        </div>
        <button
          type="button"
          onClick={() => void markAllRead()}
          disabled={unreadCount === 0}
          className="text-[11px] font-medium text-[#b95f43] disabled:text-stone-300"
        >
          全部已读
        </button>
      </header>
      <div className="min-h-0 overflow-y-auto py-1">
        {loading && <p className="px-4 py-6 text-center text-xs text-stone-400">正在读取通知…</p>}
        {!loading && notifications.length === 0 && (
          <p className="px-4 py-8 text-center text-xs text-stone-400">暂时没有通知</p>
        )}
        {notifications.map((notification) => {
          const Icon = notificationIcon(notification.kind);
          const actionable = notification.target_kind !== "none" && Boolean(notification.target_id);
          return (
            <button
              key={notification.id}
              type="button"
              onClick={() => void openNotification(notification)}
              className={`flex w-full items-start gap-3 px-4 py-3 text-left transition-colors hover:bg-stone-50 ${
                notification.read_at ? "opacity-70" : "bg-[#f5e2da]/35"
              }`}
            >
              <span className={`mt-0.5 grid h-7 w-7 shrink-0 place-items-center rounded-full ${
                notification.kind === "approval_requested"
                  ? "bg-amber-100 text-amber-700"
                  : notification.kind === "task_due"
                  ? "bg-rose-100 text-rose-700"
                  : "bg-[#f5e2da] text-[#b95f43]"
              }`}>
                <Icon className="h-3.5 w-3.5" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-center justify-between gap-3">
                  <span className="truncate text-xs font-semibold text-stone-800">{notification.title}</span>
                  <span className="shrink-0 text-[10px] text-stone-400">{relativeTime(notification.delivered_at)}</span>
                </span>
                {notification.body && <span className="mt-1 block line-clamp-2 text-[11px] leading-5 text-stone-500">{notification.body}</span>}
              </span>
              {actionable && <ChevronRight className="mt-1 h-3.5 w-3.5 shrink-0 text-stone-300" />}
            </button>
          );
        })}
      </div>
    </section>
  );

  return (
    <>
      <div className={className ?? "relative shrink-0"}>
        <button
          ref={triggerRef}
          type="button"
          onClick={toggle}
          className={`relative grid h-9 w-9 place-items-center rounded-full border shadow-sm transition-colors ${
            open ? "border-[#d97757]/30 bg-[#f5e2da] text-[#b95f43]" : "border-stone-200 bg-white text-stone-600 hover:bg-stone-50"
          }`}
          title="通知中心"
          aria-label={unreadCount ? `通知中心，${unreadCount} 条未读` : "通知中心"}
          aria-expanded={open}
        >
          <Bell className="h-4 w-4" />
          {unreadCount > 0 && (
            <span className="absolute -right-1 -top-1 grid min-h-4 min-w-4 place-items-center rounded-full bg-rose-500 px-1 text-[9px] font-semibold text-white">
              {unreadCount > 9 ? "9+" : unreadCount}
            </span>
          )}
        </button>
      </div>
      {popover && createPortal(popover, document.body)}
    </>
  );
}
