/** Zentrale formatierer fuer USD, tokens/s etc. (frueher in einzelnen pages dupliziert) */

export type Currency = "USD" | "EUR";

const CURRENCY_SYMBOLS: Record<Currency, string> = {
  USD: "$",
  EUR: "€",
};

const DEFAULT_LOCALE = "en-US";

function withDecimals(v: number, decimals: number, locale: string): string {
  return v.toLocaleString(locale, {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
    useGrouping: false,
  });
}

/**
 * Kosten formatieren mit Waehrungssymbol als Praefix.
 * Adaptive Dezimalen: |v| < 1 -> 4 Dezimalstellen, sonst 2.
 * Das Waehrungssymbol bleibt festes Praefix (bewusste Entscheidung, kein
 * Intl-Currency-Suffix); nur der Zahlenwert wird lookalisiert.
 */
export function formatCost(
  v: number,
  currency: Currency,
  locale: string = DEFAULT_LOCALE,
): string {
  if (!Number.isFinite(v)) return "–";
  const symbol = CURRENCY_SYMBOLS[currency];
  const value = v === 0 ? 0 : v; // -0 -> 0, um "-0.0000" zu vermeiden
  const decimals = Math.abs(value) < 1 ? 4 : 2;
  return `${symbol}${withDecimals(value, decimals, locale)}`;
}

/** tokens pro sekunde: vorsichtig gerundet, "unendlich" bei ~0 ms */
export function tps(
  tokens: number,
  ms: number,
  locale: string = DEFAULT_LOCALE,
): string {
  const t = tokens / (ms / 1000);
  if (!Number.isFinite(t)) return "∞";
  return formatTps(t, locale);
}

export function formatTps(v: number | null, locale: string = DEFAULT_LOCALE): string {
  if (v === null || !Number.isFinite(v)) return "–";
  const value = v === 0 ? 0 : v; // -0 -> 0, um "-0.0 t/s" zu vermeiden
  return `${withDecimals(value, value >= 100 ? 0 : 1, locale)} t/s`;
}

/**
 * Prozentwert mit 1 Dezimalstelle, z. B. en "2.3%", de "2,3 %".
 * v ist der prozentuale Wert (2.3 = 2,3%).
 */
export function formatPercent(v: number, locale: string = DEFAULT_LOCALE): string {
  if (!Number.isFinite(v)) return "–";
  const value = v === 0 ? 0 : v; // -0 -> 0, um "-0.0%" zu vermeiden
  return new Intl.NumberFormat(locale, {
    style: "percent",
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  })
    .format(value / 100)
    .replace(/\u00a0/g, " ");
}
