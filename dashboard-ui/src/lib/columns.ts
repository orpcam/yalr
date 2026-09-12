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

interface VisibilityState {
  hidden: Set<string>;
  /** true, wenn das initiale Set aus dem Mobile-Default abgeleitet wurde
   *  (nicht vom User gesetzt) -> nicht persistieren, sonst wuerde das
   *  5-Spalten-Subset unter dem gemeinsamen Key den Desktop-Default
   *  ueberschreiben */
  skipPersist: boolean;
}

function deriveInitialState<T>(
  storageId: string,
  columns: ColumnDef<T>[],
  mobileVisibleIds?: string[]
): VisibilityState {
  try {
    const raw = localStorage.getItem(storageId);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed.hidden)) {
        return { hidden: new Set<string>(parsed.hidden), skipPersist: false };
      }
    }
  } catch {
    // ignorieren -> defaults
  }
  // Mobile-Default: keine gespeicherten Preferences + schmaler Viewport
  // -> nur das kompakte Subset sichtbar (desktop bleibt alle Spalten)
  if (
    mobileVisibleIds !== undefined &&
    typeof window !== "undefined" &&
    window.innerWidth < 768
  ) {
    const visible = new Set(mobileVisibleIds);
    return {
      hidden: new Set<string>(columns.filter((c) => !visible.has(c.id)).map((c) => c.id)),
      skipPersist: true,
    };
  }
  return {
    hidden: new Set<string>(columns.filter((c) => c.defaultVisible === false).map((c) => c.id)),
    skipPersist: false,
  };
}

export function useColumnVisibility<T>(storageKey: string, columns: ColumnDef<T>[], mobileVisibleIds?: string[]) {
  const storageId = `${STORAGE_PREFIX}${storageKey}`;
  const defaultVisibleIds = columns
    .filter((c) => c.defaultVisible !== false)
    .map((c) => c.id);

  const [state, setState] = useState<VisibilityState>(() =>
    deriveInitialState(storageId, columns, mobileVisibleIds)
  );
  const { hidden, skipPersist } = state;

  useEffect(() => {
    if (skipPersist) return;
    try {
      localStorage.setItem(storageId, JSON.stringify({ hidden: [...hidden] }));
    } catch {
      // localStorage nicht verfügbar -> egal
    }
  }, [storageId, hidden, skipPersist]);

  const toggle = useCallback((id: string) => {
    setState((prev) => {
      const next = new Set(prev.hidden);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return { hidden: next, skipPersist: false };
    });
  }, []);

  const reset = useCallback(() => setState({ hidden: new Set(), skipPersist: false }), []);

  const visibleColumns = columns.filter((c) => !hidden.has(c.id));

  return { hidden, toggle, reset, visibleColumns, defaultVisibleIds };
}
