import { useCallback, useEffect, useState } from "react";

export interface ColumnDef<T> {
  id: string;
  label: string;
  /** Render-funktion für tabellen-zellen */
  render: (row: T) => React.ReactNode;
  /** CSS-classes für kopfzeile + zellen */
  className?: string;
  /** standardmäßig sichtbar (default: true) */
  defaultVisible?: boolean;
}

const STORAGE_PREFIX = "llm-gw:columns:";

export function useColumnVisibility<T>(storageKey: string, columns: ColumnDef<T>[]) {
  const storageId = `${STORAGE_PREFIX}${storageKey}`;
  const defaultVisibleIds = columns
    .filter((c) => c.defaultVisible !== false)
    .map((c) => c.id);

  const [hidden, setHidden] = useState<Set<string>>(() => {
    try {
      const raw = localStorage.getItem(storageId);
      if (raw) {
        const parsed = JSON.parse(raw);
        if (Array.isArray(parsed.hidden)) {
          return new Set<string>(parsed.hidden);
        }
      }
    } catch {
      // ignorieren -> defaults
    }
    return new Set<string>(
      columns.filter((c) => c.defaultVisible === false).map((c) => c.id)
    );
  });

  useEffect(() => {
    try {
      localStorage.setItem(storageId, JSON.stringify({ hidden: [...hidden] }));
    } catch {
      // localStorage nicht verfügbar -> egal
    }
  }, [storageId, hidden]);

  const toggle = useCallback((id: string) => {
    setHidden((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }, []);

  const reset = useCallback(() => setHidden(new Set()), []);

  const visibleColumns = columns.filter((c) => !hidden.has(c.id));

  return { hidden, toggle, reset, visibleColumns, defaultVisibleIds };
}
