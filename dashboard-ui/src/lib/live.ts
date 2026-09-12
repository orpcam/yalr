/**
 * Reine Helfer f\u00fcr die Live-Ansicht (Overview): TPS-Auswahl/-Summen pro
 * Provider und Reconcile von in-flight-requests (1s-poll) gegen
 * completed-events (SSE). Bewusst framework-frei, damit die Logik unit-
 * testbar ist.
 */
import type { InFlightRequest, LiveProviderStats } from "./api";

/**
 * Decode-TPS eines Providers w\u00e4hlen: live-metric bevorzugt, dann das
 * rolling-fenster-metric, und erst bei aktiver anfrage (busy) der eigene
 * aktivitaetswert. Ohne busy-gate duerfen inaktive provider ihren alten
 * wert nicht mehr anzeigen.
 */
export function selectDecodeTps(
  decodeTpsLive: number | null,
  decodeTps: number | null,
  decodeTpsOwn: number | null,
  busy: boolean,
): number | null {
  if (decodeTpsLive != null) return decodeTpsLive;
  if (decodeTps != null) return decodeTps;
  return busy ? decodeTpsOwn : null;
}

/** Prefill-TPS: live-metric bevorzugt, sonst nur bei aktiver anfrage. */
export function selectPrefillTps(
  prefillTpsLive: number | null,
  prefillTps: number | null,
  busy: boolean,
): number | null {
  if (prefillTpsLive != null) return prefillTpsLive;
  return busy ? prefillTps : null;
}

/**
 * Summe \u00fcber die provider-werte; null (=> Anzeige "\u2013"), wenn kein provider
 * einen wert liefert. NICHT der arithmetische mittelwert.
 */
export function sumValues(values: ReadonlyArray<number | null>): number | null {
  let sum = 0;
  let any = false;
  for (const v of values) {
    if (v != null && Number.isFinite(v)) {
      sum += v;
      any = true;
    }
  }
  return any ? sum : null;
}

/** Anzahl in-flight-requests pro provider-instanz-name (aus dem 1s-poll). */
export function inFlightCountByName(
  inFlight: readonly InFlightRequest[],
): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const r of inFlight) {
    counts[r.provider_name] = (counts[r.provider_name] ?? 0) + 1;
  }
  return counts;
}

/**
 * Ob ein getrackter request zu einer provider-zeile gehoert. Der Vergleich
 * erfolgt ueber den provider-instanz-namen (`provider_name`), NICHT ueber
 * den kind-slug (`provider`): mehrere provider-instanzen koennen denselben
 * slug teilen (z. B. `openai_compat`).
 */
export function entryBelongsToRow(
  entry: InFlightRequest,
  row: Pick<LiveProviderStats, "provider_name">,
): boolean {
  return entry.provider_name === row.provider_name;
}

/** Werte eines SSE-completed-events, die f\u00fcr die nachlaufende zeile relevant sind. */
export interface CompletedInfo {
  request_id: string;
  prompt_tokens: number;
  completion_tokens: number;
  duration_ms: number;
  first_byte_ms: number | null;
}

/**
 * Prefill- und Decode-Rate (Tokens/s) eines abgeschlossenen requests.
 *
 * prefill = prompt_tokens / (first_byte_ms / 1000)
 * decode  = completion_tokens / ((duration_ms - first_byte_ms) / 1000)
 *
 * Nur berechenbar, wenn es eine beobachtbare Phase gibt. Non-Stream-Antworten
 * haben first_byte_ms = 0 -> es gibt keine getrennte Prefill-/Decode-Phase, und
 * die Raten w\u00e4ren irref\u00fchrend. Deshalb bewusst null (=> Anzeige "\u2013"),
 * konsistent mit dem Backend, das Prefill-/Decode-Raten nur bei is_stream
 * aggregiert. Anders als die Backend-\u03a3/\u03a3 wird hier bewusst NICHT nach
 * status gefiltert: die sub-zeile zeigt pro-request-fakten (auch die
 * token-chips zeigen fehlgeschlagene requests), die rate ist f\u00fcr den
 * einzelnen request exakt.
 * Nicht-endliche Ergebnisse werden ebenfalls zu null gefalten.
 */
export function completedRates(c: CompletedInfo): {
  prefill: number | null;
  decode: number | null;
} {
  let prefill: number | null = null;
  let decode: number | null = null;

  if (c.first_byte_ms !== null && c.first_byte_ms > 0) {
    const prefillMs = c.first_byte_ms;
    const decodeMs = c.duration_ms - c.first_byte_ms;

    if (c.prompt_tokens > 0) {
      const p = c.prompt_tokens / (prefillMs / 1000);
      if (Number.isFinite(p)) prefill = p;
    }

    if (c.duration_ms > c.first_byte_ms && c.completion_tokens > 0) {
      const d = c.completion_tokens / (decodeMs / 1000);
      if (Number.isFinite(d)) decode = d;
    }
  }

  return { prefill, decode };
}

/** Ein in der live-tabelle getrackter request (laufend oder frisch abgeschlossen). */
export interface TrackedRequest {
  entry: InFlightRequest;
  completed: CompletedInfo | null;
  /** zeitstempel, zu dem der request als abgeschlossen erkannt wurde (basis der 60s-anzeige) */
  finishedAt: number | null;
  /** aufeinanderfolgende poll-antworten, in denen der request nicht in in_flight war */
  missingPolls: number;
  /** zeitpunkt (epoch ms), ab dem die zeile nicht mehr sichtbar ist und ausblendet; null = sichtbar */
  hiddenAt: number | null;
}

export interface ReconcileOptions {
  /** wie lange eine abgeschlossene zeile sichtbar bleibt (default 60s) */
  keepCompletedForMs?: number;
  /** nach wievielen on-folge-polls ohne completed-event eine zeile verworfen wird (default 2) */
  dropAfterMissedPolls?: number;
  /** max. anzahl sichtbarer abgeschlossener zeilen pro provider (default 3) */
  maxVisibleCompleted?: number;
  /** wie lange eine ausgeblendete zeile noch mitgehalten wird, bevor sie verworfen wird (default 2000 ms, passend zur 2s-css-fade-dauer der ui) */
  fadeCompletedForMs?: number;
}

/**
 * Reconcile: merged den 1s-poll (in_flight, quelle der wahrheit f\u00fcr laufende
 * anfragen) mit dem vorherigen state und neuen completed-events (SSE).
 *
 * - request in in_flight -> (neu oder) weiterverfolgt, missingPolls reset,
 *   sichtbar (hiddenAt null)
 * - request nicht mehr in in_flight + completed-event -> 60s als
 *   "abgeschlossen" sichtbar
 * - request nicht mehr in in_flight, kein completed-event,
 *   dropAfterMissedPolls mal in folge -> ausgeblendet (kein geister-"laeuft seit")
 * - pro provider bleiben max. maxVisibleCompleted abgeschlossene zeilen
 *   sichtbar (neueste zuerst); abgelaufene oder verdraengte zeilen faden
 *   aus (hiddenAt) und werden nach fadeCompletedForMs verworfen
 *
 * Pure funktion: `now` wird \u00fcbergeben, damit das deterministisch testbar ist.
 */
export function reconcileTracked(
  prev: Record<string, TrackedRequest>,
  inFlight: readonly InFlightRequest[],
  newCompleted: readonly CompletedInfo[],
  now: number,
  opts: ReconcileOptions = {},
): Record<string, TrackedRequest> {
  const keepMs = opts.keepCompletedForMs ?? 60_000;
  const dropAfter = opts.dropAfterMissedPolls ?? 2;
  const maxVisible = opts.maxVisibleCompleted ?? 3;
  const fadeMs = opts.fadeCompletedForMs ?? 2_000;

  const next: Record<string, TrackedRequest> = {};

  // 1) in_flight: neu aufnehmen oder weiterverfolgen; wieder da = wieder sichtbar
  for (const req of inFlight) {
    const existing = prev[req.request_id];
    next[req.request_id] = existing
      ? { ...existing, entry: req, missingPolls: 0, hiddenAt: existing.completed ? existing.hiddenAt : null }
      : { entry: req, completed: null, finishedAt: null, missingPolls: 0, hiddenAt: null };
  }

  // 2) completed-events: markieren (auch f\u00fcr requests, die gerade noch in in_flight sind);
  //    hiddenAt bleibt, wie es war (normalerweise null) \u2013 ein ausfadender
  //    "geist" kann so wieder sichtbar werden
  for (const c of newCompleted) {
    const existing = next[c.request_id] ?? prev[c.request_id];
    if (!existing || existing.completed) continue;
    next[c.request_id] = { ...existing, completed: c, finishedAt: now };
  }

  // 3) prev-entries, die nicht mehr in in_flight sind und nicht gerade completed markiert wurden
  for (const [id, tr] of Object.entries(prev)) {
    if (id in next) continue;
    if (tr.completed) {
      // abgeschlossen: nicht mehr hart am 60s-fenster verwerfen, sondern nur
      // den fade-start ansto\u00dfen; entfernt wird ausschlie\u00dflich ueber den fade-ablauf
      const finishedAt = tr.finishedAt ?? now;
      if (now - finishedAt >= keepMs) {
        next[id] = { ...tr, finishedAt, hiddenAt: tr.hiddenAt ?? now };
      } else {
        next[id] = { ...tr, finishedAt };
      }
    } else {
      // kein completed-event: missing-counter; statt hartem verwarf jetzt ausblenden
      const missing = tr.missingPolls + 1;
      if (missing >= dropAfter) {
        next[id] = { ...tr, missingPolls: missing, hiddenAt: tr.hiddenAt ?? now };
      } else {
        next[id] = { ...tr, missingPolls: missing };
      }
    }
  }

  // 4) sichtbarkeit: pro provider max. maxVisible abgeschlossene zeilen (neueste
  //    zuerst); abgelaufene oder verdraengte bleiben ausgeblendet
  const byProvider: Record<string, string[]> = {};
  for (const [id, tr] of Object.entries(next)) {
    if (!tr.completed) continue;
    (byProvider[tr.entry.provider_name] ??= []).push(id);
  }
  for (const ids of Object.values(byProvider)) {
    ids.sort((a, b) => {
      const diff = (next[b]!.finishedAt ?? 0) - (next[a]!.finishedAt ?? 0);
      if (diff !== 0) return diff;
      return next[b]!.entry.started_at_ms - next[a]!.entry.started_at_ms;
    });
    for (let i = 0; i < ids.length; i++) {
      const tr = next[ids[i]]!;
      const finishedAt = tr.finishedAt ?? now;
      if (i < maxVisible && now - finishedAt < keepMs) {
        next[ids[i]] = { ...tr, hiddenAt: null };
      } else {
        // abgelaufen (top, bleibt ausgeblendet) oder verdraengt (index >= maxVisible)
        next[ids[i]] = { ...tr, hiddenAt: tr.hiddenAt ?? now };
      }
    }
  }

  // 5) verwarf: alles, was l\u00e4nger als fadeMs ausgeblendet ist
  for (const [id, tr] of Object.entries(next)) {
    if (tr.hiddenAt !== null && now - tr.hiddenAt >= fadeMs) {
      delete next[id];
    }
  }

  return next;
}

/** anzeige-reihenfolge: erst laufende (neueste zuerst), dann abgeschlossene (neueste zuerst) */
export function orderLiveRows(entries: readonly TrackedRequest[]): TrackedRequest[] {
  const running = entries.filter((tr) => tr.completed === null);
  const done = entries.filter((tr) => tr.completed !== null);
  running.sort((a, b) => b.entry.started_at_ms - a.entry.started_at_ms);
  done.sort((a, b) => (b.finishedAt ?? 0) - (a.finishedAt ?? 0));
  return [...running, ...done];
}

/** Ein sample pro provider und poll-zyklus: decode-tps + kv-cache-ratio. */
export type ProviderSample = { decode: number; kv: number | null };

/**
 * Haengt ein sample an die provider-historie an (immutabel, alte objekt und
 * arrays bleiben unveraendert) und begrenzt die laenge auf maxLen eintraege,
 * aelteste zuerst entfernen. Unbekannter name erzeugt ein neues array.
 */
export function pushSample(
  history: Record<string, ProviderSample[]>,
  name: string,
  sample: ProviderSample,
  maxLen = 300,
): Record<string, ProviderSample[]> {
  const prev = history[name] ?? [];
  const next = [...prev, sample];
  if (next.length > maxLen) next.splice(0, next.length - maxLen);
  return { ...history, [name]: next };
}

/**
 * Wandelt eine sample-folge in svg-points-segmente um ("x1,y1 x2,y2 ...").
 * x ist gleichmaessig ueber width verteilt (step = width/(n-1)); y = height -
 * (v/scaleMax)*height, geclamped auf [0, height]. scaleMax null oder <= 0
 * fuehrt zu einer flachen grundlinie bei y=height. null-samples unterbrechen
 * die linie (segment-bruch); onereihenfolge nulls oder ein einzelner wert
 * ergeben kein segment; alles null -> leeres array.
 */
export function sampleSegments(
  samples: (number | null)[],
  width: number,
  height: number,
  scaleMax: number | null,
): string[] {
  if (samples.length === 0) return [];
  const usable = scaleMax !== null && scaleMax > 0;
  const step = samples.length > 1 ? width / (samples.length - 1) : 0;
  const segments: string[] = [];
  let current: string[] = [];
  const flush = () => {
    if (current.length > 1) segments.push(current.join(" "));
    current = [];
  };
  for (let i = 0; i < samples.length; i++) {
    const v = samples[i];
    if (v === null) {
      flush();
      continue;
    }
    const x = i * step;
    const y = usable
      ? Math.min(height, Math.max(0, height - (v / scaleMax) * height))
      : height;
    current.push(`${x},${y}`);
  }
  flush();
  return segments;
}
