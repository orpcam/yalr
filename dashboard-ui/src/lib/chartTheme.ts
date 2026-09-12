import { useEffect, useState } from "react";

/**
 * Liest die theme-abhaengigen CSS-variablen (border, muted-foreground, card)
 * als konkrete farbwerte und liefert sie fuer recharts zurueck, da SVG-props
 * kein var() in presentation attributes zuverlaessig aufloesen.
 * Reagiert auf theme-wechsel (`.dark`-klasse auf <html>).
 */
export interface ChartTheme {
  grid: string;
  axis: string;
  tooltipBg: string;
  tooltipBorder: string;
}

function readChartTheme(): ChartTheme {
  const styles = getComputedStyle(document.documentElement);
  const hsl = (name: string, fallback: string) => {
    const raw = styles.getPropertyValue(name).trim();
    return raw ? `hsl(${raw})` : fallback;
  };
  return {
    grid: hsl("--border", "hsl(240 3.7% 15.9%)"),
    axis: hsl("--muted-foreground", "hsl(240 5% 64.9%)"),
    tooltipBg: hsl("--card", "hsl(240 10% 3.9%)"),
    tooltipBorder: hsl("--border", "hsl(240 3.7% 15.9%)"),
  };
}

export function useChartTheme(): ChartTheme {
  const [theme, setTheme] = useState<ChartTheme>(readChartTheme);

  useEffect(() => {
    const root = document.documentElement;
    const observer = new MutationObserver(() => setTheme(readChartTheme()));
    // class-attribut beobachten: dort togglet der ThemeToggle `.dark`
    observer.observe(root, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  return theme;
}
