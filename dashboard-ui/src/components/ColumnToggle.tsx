import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useLanguage } from "@/lib/i18n";

export interface ColumnToggleProps {
  /** spalten: id + label (label wird im dropdown angezeigt) */
  columns: { id: string; label: string }[];
  hidden: Set<string>;
  onToggle: (id: string) => void;
  onReset?: () => void;
}

export function ColumnToggle({ columns, hidden, onToggle, onReset }: ColumnToggleProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const { t } = useLanguage();

  useEffect(() => {
    function onClickOutside(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", onClickOutside);
    return () => document.removeEventListener("mousedown", onClickOutside);
  }, []);

  return (
    <div className="relative" ref={ref}>
      <Button variant="outline" size="sm" onClick={() => setOpen((o) => !o)}>
        {t("columns")}
        <span className="ml-1 text-muted-foreground">({columns.length - hidden.size}/{columns.length})</span>
      </Button>
      {open && (
        <div
          className={cn(
            "absolute right-0 z-10 mt-1 min-w-44 max-w-[calc(100vw-2rem)] rounded-md border border-border",
            "bg-card p-1 shadow-md"
          )}
        >
          {columns.map((col) => (
            <label
              key={col.id}
              className="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent"
            >
              <input
                type="checkbox"
                checked={!hidden.has(col.id)}
                onChange={() => onToggle(col.id)}
                className="h-4 w-4 accent-primary"
              />
              {col.label}
            </label>
          ))}
          {onReset && (
            <>
              <div className="my-1 border-t border-border" />
              <button
                className="w-full rounded-sm px-2 py-1.5 text-left text-sm text-muted-foreground hover:bg-accent"
                onClick={() => {
                  onReset();
                  setOpen(false);
                }}
              >
                {t("reset")}
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
