import React, { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, X } from "lucide-react";

export interface ConfirmDialogOptions {
  title: string;
  description?: string;
  confirmLabel?: string;
  cancelLabel?: string;
}

interface ConfirmDialogRequest extends ConfirmDialogOptions {
  resolve: (confirmed: boolean) => void;
}

interface ConfirmDialogContextValue {
  confirm: (options: ConfirmDialogOptions) => Promise<boolean>;
}

const ConfirmDialogContext = createContext<ConfirmDialogContextValue | null>(null);

export function useConfirmDialog(): ConfirmDialogContextValue["confirm"] {
  const context = useContext(ConfirmDialogContext);
  if (!context) throw new Error("useConfirmDialog must be used within ConfirmDialogProvider");
  return context.confirm;
}

export const ConfirmDialogProvider: React.FC<React.PropsWithChildren> = ({ children }) => {
  const [request, setRequest] = useState<ConfirmDialogRequest | null>(null);
  const previousActiveElement = useRef<HTMLElement | null>(null);
  const dialogRef = useRef<HTMLElement | null>(null);

  const close = useCallback((confirmed: boolean) => {
    setRequest((current) => {
      if (!current) return null;
      current.resolve(confirmed);
      return null;
    });
  }, []);

  const confirm = useCallback((options: ConfirmDialogOptions) => new Promise<boolean>((resolve) => {
    setRequest((current) => {
      current?.resolve(false);
      previousActiveElement.current = document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
      return { ...options, resolve };
    });
  }), []);

  useEffect(() => {
    if (!request) return;
    const frame = requestAnimationFrame(() => {
      document.getElementById("agnes-confirm-dialog-cancel")?.focus();
    });
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close(false);
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
        "button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])",
      );
      if (!focusable || focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [close, request]);

  useEffect(() => {
    if (request) return;
    previousActiveElement.current?.focus();
    previousActiveElement.current = null;
  }, [request]);

  return (
    <ConfirmDialogContext.Provider value={{ confirm }}>
      {children}
      {request && createPortal(
        <div
          className="agnes-confirm-overlay fixed inset-0 z-[200] grid place-items-center p-4"
          role="presentation"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) close(false);
          }}
        >
          <section
            ref={dialogRef}
            className="agnes-confirm-dialog w-full max-w-md overflow-hidden rounded-lg border shadow-2xl"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="agnes-confirm-dialog-title"
            aria-describedby={request.description ? "agnes-confirm-dialog-description" : undefined}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header className="agnes-confirm-dialog-header flex items-start gap-3 border-b px-5 py-4">
              <span className="agnes-confirm-dialog-icon grid h-9 w-9 shrink-0 place-items-center rounded-md">
                <AlertTriangle className="h-4 w-4" />
              </span>
              <div className="min-w-0 flex-1">
                <h2 id="agnes-confirm-dialog-title" className="text-sm font-semibold leading-5">{request.title}</h2>
                {request.description && (
                  <p id="agnes-confirm-dialog-description" className="agnes-confirm-dialog-description mt-1.5 text-xs leading-5">
                    {request.description}
                  </p>
                )}
              </div>
              <button
                type="button"
                onClick={() => close(false)}
                className="agnes-confirm-dialog-close grid h-7 w-7 shrink-0 place-items-center rounded-md transition-colors"
                title="关闭"
                aria-label="关闭确认弹窗"
              >
                <X className="h-4 w-4" />
              </button>
            </header>
            <footer className="agnes-confirm-dialog-footer flex justify-end gap-2 border-t px-5 py-3">
              <button
                id="agnes-confirm-dialog-cancel"
                type="button"
                onClick={() => close(false)}
                className="agnes-confirm-dialog-cancel h-9 rounded-md px-3 text-xs font-medium transition-colors"
              >
                {request.cancelLabel ?? "取消"}
              </button>
              <button
                type="button"
                onClick={() => close(true)}
                className="agnes-confirm-dialog-submit h-9 rounded-md px-3.5 text-xs font-semibold transition-colors"
              >
                {request.confirmLabel ?? "确认删除"}
              </button>
            </footer>
          </section>
        </div>,
        document.body,
      )}
    </ConfirmDialogContext.Provider>
  );
};
