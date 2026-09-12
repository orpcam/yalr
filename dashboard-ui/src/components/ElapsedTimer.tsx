import { useEffect, useState } from "react";

/**
 * Laufzeit-anzeige fuer in-flight-requests. Besitzt sein eigenes 100ms-
 * interval, damit nicht die gesamte seite (tabellen mit hunderten zellen)
 * 10x pro sekunde neu gerendert wird.
 */
export function ElapsedTimer({
  startedAt,
  format,
}: {
  startedAt: number;
  format: (ms: number) => string;
}) {
  // elapsed als state statt Date.now() im render (reine renderfunktion)
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), 100);
    return () => clearInterval(iv);
  }, []);

  return <span className="font-mono text-xs">{format(now - startedAt)}</span>;
}
