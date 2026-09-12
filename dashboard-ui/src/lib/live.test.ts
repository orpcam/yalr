import { describe, expect, it } from "vitest";
import {
  completedRates,
  entryBelongsToRow,
  inFlightCountByName,
  orderLiveRows,
  reconcileTracked,
  sampleSegments,
  pushSample,
  selectDecodeTps,
  selectPrefillTps,
  sumValues,
  type CompletedInfo,
  type ProviderSample,
  type TrackedRequest,
} from "./live";
import type { InFlightRequest, LiveProviderStats } from "./api";

// --- helpers ---

function req(over: Partial<InFlightRequest> = {}): InFlightRequest {
  return {
    request_id: "r1",
    provider: "p1",
    provider_name: "P1",
    model: "m1",
    key_name: "k1",
    started_at_ms: 0,
    first_byte_ms: null,
    ...over,
  };
}

function completed(id: string, over: Partial<CompletedInfo> = {}): CompletedInfo {
  return {
    request_id: id,
    prompt_tokens: 100,
    completion_tokens: 50,
    duration_ms: 1200,
    first_byte_ms: 30,
    ...over,
  };
}

// --- selectDecodeTps ---

describe("selectDecodeTps", () => {
  it("prefers live metric over all others", () => {
    expect(selectDecodeTps(12.5, 9, 3, true)).toBe(12.5);
    expect(selectDecodeTps(12.5, 9, 3, false)).toBe(12.5);
  });

  it("falls back to rolling metric when no live", () => {
    expect(selectDecodeTps(null, 9, 3, true)).toBe(9);
    expect(selectDecodeTps(null, 9, 3, false)).toBe(9);
  });

  it("uses own value only when busy", () => {
    expect(selectDecodeTps(null, null, 3, true)).toBe(3);
    expect(selectDecodeTps(null, null, 3, false)).toBeNull();
  });

  it("returns null when nothing available", () => {
    expect(selectDecodeTps(null, null, null, true)).toBeNull();
    expect(selectDecodeTps(null, null, null, false)).toBeNull();
  });

  it("handles mix of live and fallback across providers (sum scenario)", () => {
    // provider A: live=20, B: no live but rolling=15 (busy), C: all null
    const a = selectDecodeTps(20, 10, 5, true);
    const b = selectDecodeTps(null, 15, 5, true);
    const c = selectDecodeTps(null, null, 5, false);
    expect(sumValues([a, b, c])).toBe(35);
  });
});

// --- selectPrefillTps ---

describe("selectPrefillTps", () => {
  it("prefers live metric", () => {
    expect(selectPrefillTps(7, 5, true)).toBe(7);
    expect(selectPrefillTps(7, 5, false)).toBe(7);
  });

  it("returns rolling metric only when busy", () => {
    expect(selectPrefillTps(null, 5, true)).toBe(5);
    expect(selectPrefillTps(null, 5, false)).toBeNull();
  });

  it("returns null when nothing", () => {
    expect(selectPrefillTps(null, null, true)).toBeNull();
    expect(selectPrefillTps(null, null, false)).toBeNull();
  });
});

// --- sumValues ---

describe("sumValues", () => {
  it("sums available values", () => {
    expect(sumValues([1.5, null, 2.5])).toBe(4);
  });

  it("ignores non-finite values", () => {
    expect(sumValues([NaN, 2, Infinity, null])).toBe(2);
  });

  it("returns null when all null", () => {
    expect(sumValues([null, null])).toBeNull();
  });

  it("returns null for empty array", () => {
    expect(sumValues([])).toBeNull();
  });

  it("sums multiple values correctly", () => {
    expect(sumValues([10, 20, 30])).toBe(60);
  });
});

// --- inFlightCountByName ---

describe("inFlightCountByName", () => {
  it("counts per provider instance name, not per slug", () => {
    const result = inFlightCountByName([
      req(),
      req({ request_id: "r2", provider: "openai_compat", provider_name: "GLM" }),
      req({ request_id: "r3", provider: "openai_compat", provider_name: "RTX 6000" }),
    ]);
    // drei entries, alle mit gleichem kind-slug, aber drei namen
    expect(result).toEqual({ P1: 1, GLM: 1, "RTX 6000": 1 });
  });

  it("sums multiple requests of one instance", () => {
    const result = inFlightCountByName([
      req(),
      req({ request_id: "r2" }),
      req({ request_id: "r3", provider_name: "RTX 6000" }),
    ]);
    expect(result).toEqual({ P1: 2, "RTX 6000": 1 });
  });

  it("returns empty object for empty array", () => {
    expect(inFlightCountByName([])).toEqual({});
  });
});

// --- entryBelongsToRow ---

describe("entryBelongsToRow", () => {
  const rowRtx = { provider_name: "RTX 6000" } satisfies Pick<LiveProviderStats, "provider_name">;
  const rowGlm = { provider_name: "GLM" } satisfies Pick<LiveProviderStats, "provider_name">;

  it("matches entries to their own row only, not crosswise on shared slug", () => {
    const rtxEntry = req({ provider: "openai_compat", provider_name: "RTX 6000" });
    const glmEntry = req({ provider: "openai_compat", provider_name: "GLM" });

    expect(entryBelongsToRow(rtxEntry, rowRtx)).toBe(true);
    expect(entryBelongsToRow(rtxEntry, rowGlm)).toBe(false);
    expect(entryBelongsToRow(glmEntry, rowGlm)).toBe(true);
    expect(entryBelongsToRow(glmEntry, rowRtx)).toBe(false);
  });

  it("matches no row for unknown/renamed providers (-> rest group)", () => {
    const entry = req({ provider: "openai_compat", provider_name: "Umbenannt" });
    expect(entryBelongsToRow(entry, rowRtx)).toBe(false);
    expect(entryBelongsToRow(entry, rowGlm)).toBe(false);
  });
});

// --- completedRates ---

describe("completedRates", () => {
  it("computes prefill and decode for a normal stream case", () => {
    const r = completedRates(
      completed("r1", {
        prompt_tokens: 22289,
        completion_tokens: 63,
        duration_ms: 3630,
        first_byte_ms: 1200,
      }),
    );
    expect(r.prefill).toBeCloseTo(22289 / 1.2, 2); // ≈18574.17
    expect(r.decode).toBeCloseTo(63 / 2.43);
  });

  it("returns both null for non-stream (first_byte_ms = 0)", () => {
    const r = completedRates(
      completed("r1", { first_byte_ms: 0 }),
    );
    expect(r.prefill).toBeNull();
    expect(r.decode).toBeNull();
  });

  it("returns both null when first_byte_ms is null", () => {
    const r = completedRates(
      completed("r1", { first_byte_ms: null }),
    );
    expect(r.prefill).toBeNull();
    expect(r.decode).toBeNull();
  });

  it("decode null when duration == first_byte_ms, prefill still computable", () => {
    const r = completedRates(
      completed("r1", {
        prompt_tokens: 1000,
        completion_tokens: 50,
        duration_ms: 2000,
        first_byte_ms: 2000,
      }),
    );
    expect(r.prefill).toBeCloseTo(1000 / 2, 5);
    expect(r.decode).toBeNull();
  });

  it("decode null when duration < first_byte_ms (negative-rate guard), prefill still computable", () => {
    const r = completedRates(
      completed("r1", {
        prompt_tokens: 1000,
        completion_tokens: 50,
        duration_ms: 1500,
        first_byte_ms: 2000,
      }),
    );
    expect(r.prefill).toBeCloseTo(1000 / 2, 5);
    expect(r.decode).toBeNull();
  });

  it("returns both null for negative first_byte_ms", () => {
    const r = completedRates(
      completed("r1", {
        prompt_tokens: 1000,
        completion_tokens: 50,
        duration_ms: 3000,
        first_byte_ms: -500,
      }),
    );
    expect(r.prefill).toBeNull();
    expect(r.decode).toBeNull();
  });

  it("decode null when completion_tokens = 0, prefill still computable", () => {
    const r = completedRates(
      completed("r1", {
        prompt_tokens: 1000,
        completion_tokens: 0,
        duration_ms: 3000,
        first_byte_ms: 1000,
      }),
    );
    expect(r.prefill).toBeCloseTo(1000 / 1, 5);
    expect(r.decode).toBeNull();
  });

  it("prefill null when prompt_tokens = 0", () => {
    const r = completedRates(
      completed("r1", {
        prompt_tokens: 0,
        completion_tokens: 50,
        duration_ms: 3000,
        first_byte_ms: 1000,
      }),
    );
    expect(r.prefill).toBeNull();
  });

  it("collapses non-finite results to null", () => {
    // Number.MIN_VALUE / 1000 underflowt zu 0 -> Division durch 0 -> Infinity,
    // der einzige real konstruierbare nicht-endliche Pfad in JS.
    const r = completedRates(
      completed("r1", {
        prompt_tokens: 1,
        completion_tokens: 1,
        duration_ms: Number.MIN_VALUE * 2,
        first_byte_ms: Number.MIN_VALUE,
      }),
    );
    expect(r.prefill).toBeNull();
    expect(r.decode).toBeNull();
  });
});

// --- reconcileTracked ---

describe("reconcileTracked", () => {
  const T0 = 1_000_000;

  it("tracks new in-flight requests", () => {
    const next = reconcileTracked({}, [req()], [], T0);
    expect(Object.keys(next)).toEqual(["r1"]);
    expect(next["r1"].missingPolls).toBe(0);
    expect(next["r1"].completed).toBeNull();
    expect(next["r1"].hiddenAt).toBeNull();
  });

  it("tracks multiple requests of same provider independently", () => {
    const next = reconcileTracked({}, [req(), req({ request_id: "r2" })], [], T0);
    expect(Object.keys(next).sort()).toEqual(["r1", "r2"]);
  });

  it("hides after 2 consecutive polls without completion, drops after fade", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    // poll 1: fehlt -> 1 missing, noch sichtbar
    state = reconcileTracked(state, [], [], T0 + 1000);
    expect(state["r1"]).toBeDefined();
    expect(state["r1"]!.missingPolls).toBe(1);
    expect(state["r1"]!.hiddenAt).toBeNull();
    // poll 2: fehlt -> 2 missing -> hiddenAt gesetzt, noch vorhanden
    state = reconcileTracked(state, [], [], T0 + 2000);
    expect(state["r1"]).toBeDefined();
    expect(state["r1"]!.missingPolls).toBe(2);
    expect(state["r1"]!.hiddenAt).toBe(T0 + 2000);
    // nach fade-ablauf (default 2000 ms) endlich verworfen
    state = reconcileTracked(state, [], [], T0 + 4001);
    expect(state["r1"]).toBeUndefined();
  });

  it("fades out completed request after 60s, drops it after the fade", () => {
    const state = reconcileTracked({}, [req()], [], T0);
    const withDone = reconcileTracked(state, [], [completed("r1")], T0 + 1000);
    expect(withDone["r1"].completed?.prompt_tokens).toBe(100);
    expect(withDone["r1"].finishedAt).toBe(T0 + 1000);
    expect(withDone["r1"].hiddenAt).toBeNull();

    // noch im 60s-fenster -> sichtbar
    const within = reconcileTracked(withDone, [], [], T0 + 30_000);
    expect(within["r1"]).toBeDefined();
    expect(within["r1"].hiddenAt).toBeNull();

    // nach 60s: fade startet (hiddenAt gesetzt), aber noch nicht verworfen
    const expired = reconcileTracked(withDone, [], [], T0 + 1000 + 60_000 + 1);
    expect(expired["r1"]).toBeDefined();
    expect(expired["r1"]!.hiddenAt).toBe(T0 + 1000 + 60_000 + 1);

    // nach fade-ablauf weg
    const after = reconcileTracked(expired, [], [], T0 + 1000 + 60_000 + 1 + 2001);
    expect(after["r1"]).toBeUndefined();
  });

  it("completed event at missing-poll boundary prevents ghost drop", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    state = reconcileTracked(state, [], [], T0 + 1000); // 1 missing
    expect(state["r1"]!.missingPolls).toBe(1);
    // completed arrives instead of second missing poll
    state = reconcileTracked(state, [], [completed("r1")], T0 + 2000);
    expect(state["r1"]!.completed).not.toBeNull();
  });

  it("resets missingPolls when request reappears in in_flight", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    state = reconcileTracked(state, [], [], T0 + 1000);
    expect(state["r1"]!.missingPolls).toBe(1);
    // request comes back
    state = reconcileTracked(state, [req({ started_at_ms: T0 + 2000 })], [], T0 + 2000);
    expect(state["r1"]!.missingPolls).toBe(0);
    expect(state["r1"]!.hiddenAt).toBeNull();
  });

  it("ignores completed events for unknown request ids", () => {
    const state = reconcileTracked({}, [], [completed("ghost")], T0);
    expect(Object.keys(state)).toEqual([]);
  });

  it("keeps completed request that is still in in_flight (race condition)", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    // completed arrives while request still in in_flight
    state = reconcileTracked(state, [req()], [completed("r1")], T0 + 500);
    expect(state["r1"]!.completed).not.toBeNull();
    expect(state["r1"].missingPolls).toBe(0);
    // next poll: not in in_flight anymore, but completed -> stays visible
    state = reconcileTracked(state, [], [], T0 + 1500);
    expect(state["r1"]).toBeDefined();
    expect(state["r1"]!.completed).not.toBeNull();
    expect(state["r1"]!.hiddenAt).toBeNull();
  });

  it("fades out old completed entries even if other entries are active", () => {
    const a = req({ request_id: "a" });
    const b = req({ request_id: "b" });
    let state = reconcileTracked({}, [a, b], [], T0);
    // a completes at T0+1000
    state = reconcileTracked(state, [b], [completed("a")], T0 + 1000);
    // after 60s, a is hidden (fade started), b still there
    const later = reconcileTracked(state, [b], [], T0 + 1000 + 61_000);
    expect(later["a"]).toBeDefined();
    expect(later["a"]!.hiddenAt).toBe(T0 + 1000 + 61_000);
    expect(later["b"]).toBeDefined();
    // after fade, a is gone, b still there
    const after = reconcileTracked(later, [b], [], T0 + 1000 + 61_000 + 2001);
    expect(after["a"]).toBeUndefined();
    expect(after["b"]).toBeDefined();
  });

  it("shows at most 3 completed rows per provider, hides and drops older ones", () => {
    const ids = ["r1", "r2", "r3", "r4"];
    let state: Record<string, TrackedRequest> = {};
    for (let i = 0; i < ids.length; i++) {
      state = reconcileTracked(state, [req({ request_id: ids[i] })], [], T0 + i * 1000);
      state = reconcileTracked(state, [], [completed(ids[i])], T0 + i * 1000 + 100);
    }
    // direkt nach dem loop (T0+3100): r1 verdraengt, hiddenAt gesetzt,
    // noch vorhanden
    expect(state["r1"]!.hiddenAt).toBe(T0 + 3100);
    expect(state["r2"]!.hiddenAt).toBeNull();
    expect(state["r3"]!.hiddenAt).toBeNull();
    expect(state["r4"]!.hiddenAt).toBeNull();
    // nach fade-ablauf (default 2000 ms) ist r1 verworfen
    const after = reconcileTracked(state, [], [], T0 + 5101);
    expect(after["r1"]).toBeUndefined();
    expect(after["r2"]).toBeDefined();
    expect(after["r3"]).toBeDefined();
    expect(after["r4"]).toBeDefined();
  });

  it("applies the 3-row limit independently per provider_name", () => {
    const a1 = req({ request_id: "a1" });
    const a2 = req({ request_id: "a2" });
    const a3 = req({ request_id: "a3" });
    const a4 = req({ request_id: "a4" });
    const b1 = req({ request_id: "b1", provider_name: "P2" });
    const b2 = req({ request_id: "b2", provider_name: "P2" });
    const b3 = req({ request_id: "b3", provider_name: "P2" });
    const b4 = req({ request_id: "b4", provider_name: "P2" });

    let state: Record<string, TrackedRequest> = {};
    state = reconcileTracked(state, [a1, b1], [], T0);
    state = reconcileTracked(state, [], [completed("a1"), completed("b1")], T0 + 100);
    state = reconcileTracked(state, [a2, b2], [], T0 + 1000);
    state = reconcileTracked(state, [], [completed("a2"), completed("b2")], T0 + 1100);
    state = reconcileTracked(state, [a3, b3], [], T0 + 2000);
    state = reconcileTracked(state, [], [completed("a3"), completed("b3")], T0 + 2100);
    state = reconcileTracked(state, [a4, b4], [], T0 + 3000);
    state = reconcileTracked(state, [], [completed("a4"), completed("b4")], T0 + 3100);
    const all = reconcileTracked(state, [], [], T0 + 4000);

    // P1: a1 verdr\u00e4ngt (\u00e4lteste), a2/a3/a4 sichtbar
    expect(all["a1"]!.hiddenAt).not.toBeNull();
    expect(all["a2"]!.hiddenAt).toBeNull();
    expect(all["a3"]!.hiddenAt).toBeNull();
    expect(all["a4"]!.hiddenAt).toBeNull();
    // P2: b1 verdr\u00e4ngt, b2/b3/b4 sichtbar (unabh\u00e4ngig von P1)
    expect(all["b1"]!.hiddenAt).not.toBeNull();
    expect(all["b2"]!.hiddenAt).toBeNull();
    expect(all["b3"]!.hiddenAt).toBeNull();
    expect(all["b4"]!.hiddenAt).toBeNull();
  });

  it("bricht identische finishedAt deterministisch ueber started_at_ms auf (aelteste wird verdraengt)", () => {
    // 4 abgeschlossene requests desselben providers, alle im selben poll completed
    // -> identisches finishedAt. Die Auswahl muss von der eingabe-reihenfolge
    // unabhängig deterministisch sein: absteigend nach started_at_ms, also
    // verliert der älteste eintrag den platz.
    const r1 = req({ request_id: "r1", started_at_ms: 100 });
    const r2 = req({ request_id: "r2", started_at_ms: 200 });
    const r3 = req({ request_id: "r3", started_at_ms: 300 });
    const r4 = req({ request_id: "r4", started_at_ms: 400 });
    let state = reconcileTracked({}, [r4, r3, r2, r1], [], T0);
    state = reconcileTracked(state, [], [completed("r4"), completed("r3"), completed("r2"), completed("r1")], T0 + 1000);
    expect(state["r1"]!.hiddenAt).toBe(T0 + 1000);
    expect(state["r2"]!.hiddenAt).toBeNull();
    expect(state["r3"]!.hiddenAt).toBeNull();
    expect(state["r4"]!.hiddenAt).toBeNull();
  });

  it("halt finishedAt und completed-werte bei duplikaten completed-events (erstes event gewinnt)", () => {
    // ueber zwei poll
    let state = reconcileTracked({}, [req()], [], T0);
    state = reconcileTracked(state, [], [completed("r1")], T0 + 1000);
    expect(state["r1"]!.finishedAt).toBe(T0 + 1000);
    state = reconcileTracked(state, [], [completed("r1", { prompt_tokens: 999 })], T0 + 5000);
    expect(state["r1"]!.finishedAt).toBe(T0 + 1000);
    expect(state["r1"]!.completed?.prompt_tokens).toBe(100);
    // im selben poll-array
    const fresh = reconcileTracked({}, [req({ request_id: "x" })], [], T0);
    const dup = reconcileTracked(fresh, [], [completed("x"), completed("x", { prompt_tokens: 999 })], T0 + 500);
    expect(dup["x"]!.finishedAt).toBe(T0 + 500);
    expect(dup["x"]!.completed?.prompt_tokens).toBe(100);
  });

  it("wiederbelebt einen versteckten geist, wenn das completed-event doch noch trifft", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    state = reconcileTracked(state, [], [], T0 + 1000); // 1 missing
    state = reconcileTracked(state, [], [], T0 + 2000); // 2 missing -> hidden
    expect(state["r1"]!.hiddenAt).toBe(T0 + 2000);
    state = reconcileTracked(state, [], [completed("r1")], T0 + 3000);
    expect(state["r1"]!.completed).not.toBeNull();
    expect(state["r1"]!.finishedAt).toBe(T0 + 3000);
    expect(state["r1"]!.hiddenAt).toBeNull();
  });

  it("wendet die grenzen exakt an: bei keepCompletedForMs wird hiddenAt gesetzt, bei fadeCompletedForMs verworfen", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    state = reconcileTracked(state, [], [completed("r1")], T0 + 1000);
    // genau 60s nach completion -> hiddenAt zum ersten mal gesetzt
    const atKeep = reconcileTracked(state, [], [], T0 + 61_000);
    expect(atKeep["r1"]).toBeDefined();
    expect(atKeep["r1"]!.hiddenAt).toBe(T0 + 61_000);
    // genau 2000 ms nach verstecken -> verworfen
    const atFade = reconcileTracked(atKeep, [], [], T0 + 61_000 + 2_000);
    expect(atFade["r1"]).toBeUndefined();
  });

  it("ueberschreibt keepCompletedForMs und fadeCompletedForMs ueber opts", () => {
    const opts = { keepCompletedForMs: 5000, fadeCompletedForMs: 3000 };
    let state = reconcileTracked({}, [req()], [], T0, opts);
    state = reconcileTracked(state, [], [completed("r1")], T0 + 1000, opts);
    // nach 4s noch sichtbar (wäre auch bei default 60s sichtbar)
    const before = reconcileTracked(state, [], [], T0 + 5000, opts);
    expect(before["r1"]!.hiddenAt).toBeNull();
    // nach 5s versteckt (wäre bei default 60s noch sichtbar)
    const expired = reconcileTracked(state, [], [], T0 + 6001, opts);
    expect(expired["r1"]!.hiddenAt).toBe(T0 + 6001);
    // 3s nach verstecken noch da (wäre bei default 2s schon weg)
    const kept = reconcileTracked(expired, [], [], T0 + 8999, opts);
    expect(kept["r1"]).toBeDefined();
    const gone = reconcileTracked(expired, [], [], T0 + 9002, opts);
    expect(gone["r1"]).toBeUndefined();
  });

  it("setzt hiddenAt bei einem zwischen-poll nicht neu (zeitstempel bleibt stehen)", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    state = reconcileTracked(state, [], [], T0 + 1000); // 1 missing
    state = reconcileTracked(state, [], [], T0 + 2000); // 2 missing -> hiddenAt = T0+2000
    const mid = reconcileTracked(state, [], [], T0 + 2400);
    expect(mid["r1"]).toBeDefined();
    expect(mid["r1"]!.hiddenAt).toBe(T0 + 2000); // nicht T0+2400
    const gone = reconcileTracked(mid, [], [], T0 + 4000);
    expect(gone["r1"]).toBeUndefined();
  });

  it("regression: abgeschlossener request, der dauerhaft in in_flight bleibt, verschwindet trotzdem (leak)", () => {
    let state = reconcileTracked({}, [req()], [], T0);
    state = reconcileTracked(state, [req()], [completed("r1")], T0 + 1000);
    expect(state["r1"]!.completed).not.toBeNull();
    // genau 60s nach completion, immer noch in in_flight -> hiddenAt wird
    // gesetzt und darf beim nächsten poll nicht wieder auf null zurückfallen
    const hidden = reconcileTracked(state, [req()], [], T0 + 61_000);
    expect(hidden["r1"]!.hiddenAt).toBe(T0 + 61_000);
    // nach fade-ablauf (keepCompletedForMs + fadeCompletedForMs) endgültig verworfen,
    // trotz weiterem in_flight-melden
    const gone = reconcileTracked(hidden, [req()], [], T0 + 63_001);
    expect(gone["r1"]).toBeUndefined();
  });

  it("completed event for a request still in in_flight counts toward visibility", () => {
    // r1 ist noch in in_flight, completed-event trifft ein
    const opts = { maxVisibleCompleted: 1 };
    let state: Record<string, TrackedRequest> = {};
    state = reconcileTracked(state, [req({ request_id: "r1" }), req({ request_id: "r2" })], [], T0, opts);
    // beide completed, r1 ist noch in_flight
    state = reconcileTracked(state, [req({ request_id: "r1" })], [completed("r1"), completed("r2")], T0 + 1000, opts);
    // r1 ist in_flight + completed -> sichtbar (top 1); r2 ist completed und verdr\u00e4ngt
    expect(state["r1"]!.completed).not.toBeNull();
    expect(state["r1"]!.hiddenAt).toBeNull();
    // r2 ist completed, aber index 1 >= maxVisible=1 -> hidden
    expect(state["r2"]!.completed).not.toBeNull();
    expect(state["r2"]!.hiddenAt).toBe(T0 + 1000);
  });
});

// --- orderLiveRows ---

describe("orderLiveRows", () => {
  function tracked(over: Partial<InFlightRequest> = {}, completedInfo: CompletedInfo | null = null, finishedAt: number | null = null): TrackedRequest {
    return { entry: req(over), completed: completedInfo, finishedAt, missingPolls: 0, hiddenAt: null };
  }

  it("orders running before completed, newest first within each group", () => {
    const r1 = tracked({ request_id: "r1", started_at_ms: 1000 });
    const r2 = tracked({ request_id: "r2", started_at_ms: 2000 });
    const c1 = tracked({ request_id: "c1", started_at_ms: 500 }, completed("c1"), 3000);
    const c2 = tracked({ request_id: "c2", started_at_ms: 400 }, completed("c2"), 4000);
    const result = orderLiveRows([c1, r1, c2, r2]);
    // running first (r2 started_at=2000 > r1 started_at=1000), then completed (c2 finished=4000 > c1 finished=3000)
    expect(result.map((tr) => tr.entry.request_id)).toEqual(["r2", "r1", "c2", "c1"]);
  });

  it("returns empty array for empty input", () => {
    expect(orderLiveRows([])).toEqual([]);
  });

  it("does not mutate the input array with mixed running and completed entries", () => {
    const r1 = tracked({ request_id: "r1", started_at_ms: 1000 });
    const c1 = tracked({ request_id: "c1", started_at_ms: 500 }, completed("c1"), 3000);
    const r2 = tracked({ request_id: "r2", started_at_ms: 2000 });
    const c2 = tracked({ request_id: "c2", started_at_ms: 400 }, completed("c2"), 4000);
    const input = [c1, r1, c2, r2];
    const snapshot = input.map((tr) => tr.entry.request_id);
    const result = orderLiveRows(input);
    expect(input.map((tr) => tr.entry.request_id)).toEqual(snapshot);
    expect(result.map((tr) => tr.entry.request_id)).toEqual(["r2", "r1", "c2", "c1"]);
  });
});

// --- pushSample ---

describe("pushSample", () => {
  it("appends a sample to an existing provider", () => {
    const h: Record<string, ProviderSample[]> = { a: [{ decode: 1, kv: null }] };
    const next = pushSample(h, "a", { decode: 2, kv: 0.5 });
    expect(next.a).toEqual([
      { decode: 1, kv: null },
      { decode: 2, kv: 0.5 },
    ]);
  });

  it("creates a new array for an unknown provider", () => {
    const next = pushSample({}, "b", { decode: 3, kv: 0.2 });
    expect(next.b).toEqual([{ decode: 3, kv: 0.2 }]);
    expect(Object.keys(next)).toEqual(["b"]);
  });

  it("trims to maxLen (default 300) removing oldest first", () => {
    let h: Record<string, ProviderSample[]> = {};
    for (let i = 0; i < 305; i++) {
      h = pushSample(h, "a", { decode: i, kv: null });
    }
    expect(h.a).toHaveLength(300);
    expect(h.a![0].decode).toBe(5);
    expect(h.a![299].decode).toBe(304);
  });

  it("respects a custom maxLen", () => {
    let h: Record<string, ProviderSample[]> = {};
    for (let i = 0; i < 5; i++) {
      h = pushSample(h, "a", { decode: i, kv: null }, 3);
    }
    expect(h.a).toHaveLength(3);
    expect(h.a!.map((s) => s.decode)).toEqual([2, 3, 4]);
  });

  it("keeps other providers untouched", () => {
    const a = { decode: 1, kv: null };
    const h: Record<string, ProviderSample[]> = { a: [a], b: [{ decode: 9, kv: 0.9 }] };
    const next = pushSample(h, "b", { decode: 10, kv: null });
    expect(next.a).toBe(h.a);
    expect(next.b).toHaveLength(2);
  });

  it("does not mutate its inputs", () => {
    const arr: ProviderSample[] = [{ decode: 1, kv: null }];
    const h: Record<string, ProviderSample[]> = { a: arr };
    const next = pushSample(h, "a", { decode: 2, kv: null });
    expect(h).toEqual({ a: [{ decode: 1, kv: null }] });
    expect(h.a).toBe(arr);
    expect(h.a).toHaveLength(1);
    expect(next).not.toBe(h);
    expect(next.a).not.toBe(arr);
  });
});

// --- sampleSegments ---

describe("sampleSegments", () => {
  it("normalizes values to the height for the given scaleMax", () => {
    expect(sampleSegments([0, 0.5, 1], 10, 10, 1)).toEqual(["0,10 5,5 10,0"]);
  });

  it("clamps values outside [0, scaleMax]", () => {
    expect(sampleSegments([2, -1, 0], 10, 10, 1)).toEqual(["0,0 5,10 10,10"]);
  });

  it("breaks the line at nulls (multiple segments)", () => {
    expect(sampleSegments([1, 0, null, 0, 1], 20, 10, 1)).toEqual([
      "0,0 5,10",
      "15,10 20,0",
    ]);
  });

  it("produces no segment for single points or all-null input", () => {
    expect(sampleSegments([null, 1, null], 10, 10, 1)).toEqual([]);
    expect(sampleSegments([null, null], 10, 10, 1)).toEqual([]);
    expect(sampleSegments([null], 10, 10, 1)).toEqual([]);
  });

  it("renders a flat baseline when scaleMax is null or <= 0", () => {
    expect(sampleSegments([1, 2, 3], 10, 10, null)).toEqual(["0,10 5,10 10,10"]);
    expect(sampleSegments([1, 2, 3], 10, 10, 0)).toEqual(["0,10 5,10 10,10"]);
  });

  it("returns an empty array for empty input", () => {
    expect(sampleSegments([], 10, 10, 1)).toEqual([]);
  });
});
