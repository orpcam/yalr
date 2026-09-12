import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
  ReactNode,
} from "react";
import { CheckCircle2, XCircle } from "lucide-react";
import { cn } from "@/lib/utils";

type ToastKind = "success" | "error";

interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

interface ToastContextValue {
  toast: (kind: ToastKind, message: string) => void;
  success: (message: string) => void;
  error: (message: string) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

const TOAST_TTL_MS = 4000;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  // counter statt Date.now()-fallback: id muss nur eindeutig sein
  const nextId = useRef(0);

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const toast = useCallback(
    (kind: ToastKind, message: string) => {
      const id = nextId.current++;
      setToasts((prev) => [...prev.slice(-4), { id, kind, message }]);
      window.setTimeout(() => dismiss(id), TOAST_TTL_MS);
    },
    [dismiss]
  );

  const value: ToastContextValue = {
    toast,
    success: useCallback((m: string) => toast("success", m), [toast]),
    error: useCallback((m: string) => toast("error", m), [toast]),
  };

  return (
    <ToastContext.Provider value={value}>
      {children}
      {/* fixed unten rechts, ueber allem; aria-live fuer screenreader */}
      <div
        aria-live="polite"
        className="pointer-events-none fixed bottom-4 right-4 z-50 flex w-80 flex-col gap-2"
      >
        {toasts.map((t) => (
          <div
            key={t.id}
            role="status"
            className={cn(
              "pointer-events-auto flex items-start gap-2 rounded-md border p-3 text-sm shadow-md",
              t.kind === "error"
                ? "border-destructive/40 bg-card text-foreground"
                : "border-emerald-500/40 bg-card text-foreground"
            )}
          >
            {t.kind === "error" ? (
              <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" />
            ) : (
              <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-emerald-500" />
            )}
            <span className="min-w-0 break-words">{t.message}</span>
            <button
              aria-label="×"
              className="ml-auto shrink-0 text-muted-foreground hover:text-foreground"
              onClick={() => dismiss(t.id)}
            >
              <XCircle className="h-4 w-4" />
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used within ToastProvider");
  return ctx;
}

/** toast.error mit fehlermeldung aus einem gefangenen error */
export function toastFromError(
  toast: ToastContextValue,
  err: unknown,
  fallback: string
) {
  toast.error(
    err instanceof Error && err.message ? `${fallback}: ${err.message}` : fallback
  );
}
