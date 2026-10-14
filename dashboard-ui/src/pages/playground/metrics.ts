// Latenz/durchsatz-messung fuer eine playground-antwort.
import type { Usage } from "./types";

export interface MetricsSummary {
  /** zeit bis zum ersten sichtbaren delta (content oder reasoning) */
  ttftMs?: number;
  totalMs: number;
  completionTokens?: number;
  tokensPerSec?: number;
  /** true = tokens aus zeichen geschaetzt (keine usage) */
  estimated: boolean;
}

export interface Metrics {
  /** bei jedem sichtbaren delta aufrufen; chars = laenge des textes */
  delta(chars: number, at?: number): void;
  summary(usage?: Usage, endedAt?: number): MetricsSummary;
}

export function createMetrics(sentAt: number): Metrics {
  let first: number | undefined;
  let last: number | undefined;
  let chars = 0;
  return {
    delta(n, at = performance.now()) {
      if (n <= 0) return;
      if (first === undefined) first = at;
      last = at;
      chars += n;
    },
    summary(usage, endedAt = last ?? performance.now()) {
      const totalMs = Math.max(0, endedAt - sentAt);
      const ttftMs = first !== undefined ? first - sentAt : undefined;
      let completionTokens = usage?.completion_tokens;
      let estimated = false;
      if (completionTokens === undefined && chars > 0) {
        completionTokens = Math.max(1, Math.round(chars / 4));
        estimated = true;
      }
      let tokensPerSec: number | undefined;
      if (completionTokens !== undefined && first !== undefined && last !== undefined && last > first) {
        tokensPerSec = completionTokens / ((last - first) / 1000);
      }
      return { ttftMs, totalMs, completionTokens, tokensPerSec, estimated };
    },
  };
}
