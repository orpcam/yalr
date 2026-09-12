import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ResponsiveContainer,
  AreaChart,
  Area,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
} from "recharts";
import { api, Stats, TimeseriesPoint, BreakdownItem, VirtualKey, LiveStats, LiveEvent } from "@/lib/api";
import {
  inFlightCountByName,
  entryBelongsToRow,
  orderLiveRows,
  reconcileTracked,
  selectDecodeTps,
  selectPrefillTps,
  sumValues,
  pushSample,
  type CompletedInfo,
  type ProviderSample,
  type TrackedRequest,
} from "@/lib/live";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Sparkline } from "@/components/Sparkline";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useToast } from "@/components/ui/toast";
import { useChartTheme } from "@/lib/chartTheme";
import { cn } from "@/lib/utils";
import { formatTps, formatPercent } from "@/lib/format";
import { completedRates } from "@/lib/live";
import { useLanguage, useCurrency, type TranslationKey } from "@/lib/i18n";

function errorRateClass(rate: number) {
  if (rate > 5) return "text-destructive";
  if (rate > 1) return "text-yellow-500";
  return "";
}

export default function Overview() {
  const [hours, setHours] = useState(24);
  const [keyName, setKeyName] = useState("");
  const [stats, setStats] = useState<Stats | null>(null);
  const [ts, setTs] = useState<TimeseriesPoint[]>([]);
  const [byModel, setByModel] = useState<BreakdownItem[]>([]);
  const [keys, setKeys] = useState<VirtualKey[]>([]);
  const [liveOn, setLiveOn] = useState(true);
  const [liveWindow, setLiveWindow] = useState(60);
  const [liveStats, setLiveStats] = useState<LiveStats | null>(null);
  const [tracked, setTracked] = useState<Record<string, TrackedRequest>>({});
  const completedRef = useRef<Record<string, CompletedInfo>>({});
  // sparkline-verlauf pro provider; wird nur im poll-callback mutiert,
  // nie waehrend des render
  const historyRef = useRef<Record<string, ProviderSample[]>>({});
  const { t, formatNumber, formatTime, formatDuration, locale } = useLanguage();
  const { formatCost } = useCurrency();
  const toast = useToast();
  const chart = useChartTheme();
  const keyNameRef = useRef(keyName);

  // latest-ref: der SSE-handler liest den aktuellen key-filter, ohne dass die
  // verbindung bei jedem filter-wechsel neu aufgebaut werden muss
  useEffect(() => {
    keyNameRef.current = keyName;
  }, [keyName]);

  // Abort-Controller des gerade in-flighten Overview-Loads. Ein neuer Load
  // abortet den vorherigen -> alte Antworten koennen den neuen State nicht
  // mehr ueberschreiben (Stale-Overwrite).
  const loadControllerRef = useRef<AbortController | null>(null);

  // stats/timeseries/breakdown mit aktuellen filtern laden (mount + live-poll).
  // Zu jedem zeitpunkt ist max. ein load in-flight.
  const loadOverview = useCallback(
    (opts: { silent?: boolean } = {}) => {
      const silent = opts.silent ?? false;
      loadControllerRef.current?.abort();
      const controller = new AbortController();
      loadControllerRef.current = controller;
      const fail = (e: unknown) => {
        if (e instanceof DOMException && e.name === "AbortError") return; // abgebrochen: still
        if (!silent) toast.error(e instanceof Error ? e.message : t("load_failed"));
      };
      const params = new URLSearchParams({ hours: String(hours) });
      if (keyName) params.set("key_name", keyName);
      const qs = `?${params.toString()}`;

      api.get<Stats>(`/stats${qs}`, { signal: controller.signal }).then(setStats).catch(fail);
      api.get<{ timeseries: TimeseriesPoint[] }>(`/timeseries${qs}`, { signal: controller.signal })
        .then((r) => setTs(r.timeseries))
        .catch(fail);
      api
        .get<{ breakdown: BreakdownItem[] }>(`/breakdown${qs}&group_by=model_provider`, {
          signal: controller.signal,
        })
        .then((r) => setByModel(r.breakdown))
        .catch(fail);
    },
    [hours, keyName, toast, t]
  );

  // Initial-Load + filterwechsel. Im live-modus ist dies der einzige
  // load-ausloeser beim mount/filterwechsel — der 5s-poll unten startete
  // frueher eigenstaendig einen zweiten (doppelten) load beim mount.
  useEffect(() => {
    loadOverview();
  }, [loadOverview]);

  useEffect(() => {
    api
      .get<{ keys: VirtualKey[] }>("/keys")
      .then((r) => setKeys(r.keys))
      .catch((e) => toast.error(e instanceof Error ? e.message : t("load_failed")));
  }, [toast, t]);

  // Live-Modus: 1s-Poll auf /live/stats (key-filter wird respektiert).
  // Bei verstecktem tab pausiert das polling (visibilitychange); die signale
  // brechen in-flight-requests ab. Reset passiert im toggle-handler.
  useEffect(() => {
    if (!liveOn) return;
    let aborted = false;
    // monoton steigende seq: verspaetete responses verwerfen
    let seq = 0;
    const poll = (signal?: AbortSignal) => {
      const params = new URLSearchParams({ window: String(liveWindow) });
      if (keyName) params.set("key_name", keyName);
      const mySeq = ++seq;
      api
        .get<LiveStats>(`/live/stats?${params.toString()}`, signal ? { signal } : undefined)
        .then((r) => {
          if (aborted || mySeq !== seq) return; // verspaetete response verwerfen
          const fresh = Object.values(completedRef.current);
          completedRef.current = {};
          setTracked((prev) => reconcileTracked(prev, r.in_flight ?? [], fresh, Date.now()));
          setLiveStats(r);
          // 1 sample pro provider und poll-zyklus (nur ref, kein extra
          // state/render: setLiveStats rendert ohnehin 1x/s). Gleiche
          // busy-formel wie liveRows, damit sparkline und zellenwert nicht
          // divergieren (decode_tps_own-fall).
          const ifByName = inFlightCountByName(r.in_flight ?? []);
          for (const p of r.providers) {
            const busy =
              (ifByName[p.provider_name] ?? 0) > 0 ||
              (p.running ?? 0) > 0 ||
              (p.queued ?? 0) > 0;
            const decode =
              selectDecodeTps(p.decode_tps_live, p.decode_tps, p.decode_tps_own, busy) ?? 0;
            historyRef.current = pushSample(historyRef.current, p.provider_name || p.provider, {
              decode,
              kv: p.kv_cache_usage ?? null,
            });
          }
          // verwaiste keys (provider-rename) entfernen
          const known = new Set(r.providers.map((p) => p.provider_name || p.provider));
          historyRef.current = Object.fromEntries(
            Object.entries(historyRef.current).filter(([k]) => known.has(k))
          );
        })
        .catch(() => {
          // ausfall oder abort: naechster poll kommt ohnehin uebers interval
        });
    };
    const tick = () => {
      if (document.hidden) return; // pausiert solange der tab unsichtbar ist
      controller.abort();
      controller = new AbortController();
      poll(controller.signal);
    };
    let controller = new AbortController();
    poll(controller.signal);
    const iv = setInterval(tick, 1000);
    const onVisibility = () => {
      if (!document.hidden) {
        // sofort nachladen, wenn der tab wieder sichtbar wird
        controller.abort();
        controller = new AbortController();
        poll(controller.signal);
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      aborted = true;
      clearInterval(iv);
      document.removeEventListener("visibilitychange", onVisibility);
      controller.abort();
    };
  }, [liveOn, liveWindow, keyName]);

  // Live-Modus: 5s-Poll auf /stats, /timeseries und /breakdown mit aktuellen
  // filtern. Der initiale load passiert ueber den effekt oben (nicht-still,
  // toasts erhalten) — dieser effekt macht NUR die wiederholungen, alle still.
  // Abbruch des vorherigen loads passiert zentral in loadOverview.
  useEffect(() => {
    if (!liveOn) return;
    const poll = () => loadOverview({ silent: true });
    const tick = () => {
      if (document.hidden) return; // pausiert solange der tab unsichtbar ist
      poll();
    };
    const iv = setInterval(tick, 5000);
    const onVisibility = () => {
      if (!document.hidden) poll();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearInterval(iv);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [liveOn, loadOverview]);

  // Live-Modus: SSE nur noch fuer completed-events (anreichern der
  // abgeschlossenen zeilen). In-flight wird NICHT mehr ueber request_started
  // gesammelt, sondern aus dem 1s-poll (liveStats.in_flight) — sonst double-counting.
  useEffect(() => {
    if (!liveOn) return;
    const es = new EventSource("/dashboard-api/live");
    es.addEventListener("completed", (e) => {
      const ev = JSON.parse((e as MessageEvent).data) as Extract<
        LiveEvent,
        { type: "completed" }
      >;
      const log = ev.log;
      if (keyNameRef.current && log.key_name !== keyNameRef.current) return;
      completedRef.current[log.request_id] = {
        request_id: log.request_id,
        prompt_tokens: log.prompt_tokens,
        completion_tokens: log.completion_tokens,
        duration_ms: log.duration_ms,
        first_byte_ms: log.first_byte_ms,
      };
    });
    return () => es.close();
  }, [liveOn]);

  const chartData = useMemo(
    () => ts.map((p) => ({ ...p, time: formatTime(p.bucket) })),
    [ts, formatTime]
  );

  // in-flight pro provider: quelle der wahrheit ist der 1s-poll (in_flight);
  // p.running bleibt als fallback, falls das pollfeld fehlt.
  const inFlightByName = inFlightCountByName(liveStats?.in_flight ?? []);

  // live-werte pro provider: provider-metric (live) bevorzugt, sonst das
  // rolling-fenster-metric; decode-own nur solange der provider anfragen
  // bearbeitet (busy), danach "–". das rolling window gilt nur fuer ttft/p50/p95.
  const liveRows = (liveStats?.providers ?? []).map((p) => {
    const name = p.provider_name || p.provider;
    const inFlight = inFlightByName[p.provider_name] ?? 0;
    const busy =
      inFlight > 0 || (p.running ?? 0) > 0 || (p.queued ?? 0) > 0;
    return {
      p,
      name,
      inFlight,
      decode: selectDecodeTps(p.decode_tps_live, p.decode_tps, p.decode_tps_own, busy),
      prefill: selectPrefillTps(p.prefill_tps_live, p.prefill_tps, busy),
    };
  });

  // karten: summe ueber alle provider (null => "–", nicht 0.0 t/s)
  const totalDecode = sumValues(liveRows.map((r) => r.decode));
  const totalPrefill = sumValues(liveRows.map((r) => r.prefill));
  const totalReqs = liveRows.reduce((a, r) => a + r.p.reqs, 0);
  const totalErrors = liveRows.reduce((a, r) => a + r.p.errors, 0);

  // in-flight gesamt: 1s-poll ist quelle der wahrheit; fallback auf running
  // nur ohne key-filter (provider-metrics kennen keine keys).
  const pollInFlight = liveStats?.in_flight;
  const totalInFlight =
    pollInFlight && pollInFlight.length > 0
      ? pollInFlight.length
      : keyName
        ? 0
        : liveRows.reduce(
            (acc, r) => (r.p.running !== null ? acc + Math.round(r.p.running) : acc + r.inFlight),
            0
          );
  // queued: summe ueber alle provider mit metrics-endpoint (null = N/A)
  const queuedValues = liveRows
    .map((r) => r.p.queued)
    .filter((v): v is number => v != null);
  const totalQueued =
    queuedValues.length > 0 ? Math.round(queuedValues.reduce((a, b) => a + b, 0)) : null;
  const errRate = totalReqs > 0 ? (totalErrors / totalReqs) * 100 : 0;

  const trackedEntries = Object.values(tracked);
  // tickende "läuft-seit"-anzeige: jetzt als state, per intervall aktualisiert
  // (Date.now in render wäre impure laut react-hooks-lint). ticker läuft
  // unbedingte 1/s — overview re-rendert ohnehin im 1-s-live-poll-takt, also
  // ohne zusätzliche kosten; kein set-state-in-effect, kein stale-"läuft-seit-0s".
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(iv);
  }, []);

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="w-full text-2xl font-bold sm:w-auto">{t("nav_overview")}</h1>
        <div className="flex w-full flex-wrap items-center gap-2 sm:w-auto">
          <Button
            variant={liveOn ? "default" : "outline"}
            className="h-9"
            onClick={() => {
              setLiveOn((l) => {
                const next = !l;
                if (!next) {
                  setLiveStats(null);
                  setTracked({});
                  completedRef.current = {};
                  historyRef.current = {};
                }
                return next;
              });
            }}
          >
            {liveOn && (
              <span className="mr-2 inline-block h-2 w-2 animate-pulse rounded-full bg-emerald-400" />
            )}
            {t("live")}
          </Button>
          <Select
            className="w-full sm:w-44"
            value={keyName}
            onChange={(e) => setKeyName(e.target.value)}
          >
            <option value="">{t("all_keys")}</option>
            {keys.map((k) => (
              <option key={k.id} value={k.name}>
                {k.name}
              </option>
            ))}
          </Select>
          <Select
            className="w-full sm:w-36"
            value={hours}
            onChange={(e) => setHours(Number(e.target.value))}
          >
            <option value={1}>{t("last_hour")}</option>
            <option value={24}>{t("last_24h")}</option>
            <option value={168}>{t("last_7d")}</option>
            <option value={720}>{t("last_30d")}</option>
          </Select>
        </div>
      </div>

      {liveOn && (
        <Card>
          <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2">
            <CardTitle>{t("live_performance")}</CardTitle>
            <Select
              className="w-full sm:w-28"
              value={liveWindow}
              onChange={(e) => setLiveWindow(Number(e.target.value))}
            >
              <option value={30}>{t("window_30s")}</option>
              <option value={60}>{t("window_60s")}</option>
              <option value={300}>{t("window_5m")}</option>
            </Select>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-5">
              <div>
                <p className="text-sm text-muted-foreground">{t("decode_tps")}</p>
                <p className="truncate tabular-nums text-2xl font-bold">
                  {totalDecode !== null ? formatTps(totalDecode, locale) : "–"}
                </p>
              </div>
              <div>
                <p className="text-sm text-muted-foreground">{t("prefill_tps")}</p>
                <p className="truncate tabular-nums text-2xl font-bold">
                  {totalPrefill !== null ? formatTps(totalPrefill, locale) : "–"}
                </p>
              </div>
              <div>
                <p className="text-sm text-muted-foreground">{t("requests_in_progress")}</p>
                <p className="tabular-nums text-2xl font-bold">{totalInFlight}</p>
              </div>
              <div>
                <p className="text-sm text-muted-foreground">{t("queued")}</p>
                <p className="tabular-nums text-2xl font-bold">{totalQueued !== null ? totalQueued : "–"}</p>
              </div>
              <div>
                <p className="text-sm text-muted-foreground">{t("error_rate")}</p>
                <p className={`truncate tabular-nums text-2xl font-bold ${errorRateClass(errRate)}`}>
                  {totalReqs > 0 ? formatPercent(errRate, locale) : "–"}
                </p>
              </div>
            </div>
            <p className="text-xs text-muted-foreground">
              {t("last_seconds", { s: liveStats?.window ?? liveWindow })} · {t("live_window_note")}
            </p>

            {liveStats && liveStats.providers.length > 0 ? (
              <div className="w-full overflow-x-auto overscroll-x-contain">
                <Table className="min-w-[960px]">
                  <TableHeader>
                    <TableRow>
                      <TableHead className="min-w-36 whitespace-nowrap pl-4">{t("provider")}</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">{t("in_flight")}</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">{t("queued")}</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">{t("avg_ttft")}</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">P50</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">P95</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">{t("decode_tps")}</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">{t("kv_cache")}</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">{t("prefill_tps")}</TableHead>
                      <TableHead className="min-w-16 whitespace-nowrap text-right">{t("error_rate")}</TableHead>
                      <TableHead className="min-w-20 whitespace-nowrap text-right pr-4">{t("cost")}</TableHead>
                    </TableRow>
                  </TableHeader>
                <TableBody>
                  {liveRows.map(({ p, name, inFlight, decode, prefill }) => {
                    const rate = p.reqs > 0 ? (p.errors / p.reqs) * 100 : 0;
                    return (
                      <Fragment key={`${p.provider}-${name}`}>
                      <TableRow>
                        <TableCell className="whitespace-nowrap pl-4">
                          <span className="font-medium">{name}</span>
                          <span className="ml-2 text-xs text-muted-foreground">{p.provider}</span>
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-xs">
                          {p.running !== null ? Math.round(p.running) : inFlight}
                        </TableCell>
                        <TableCell
                          className={`text-right font-mono tabular-nums text-xs ${
                            p.queued !== null && p.queued > 0 ? "text-yellow-500" : ""
                          }`}
                        >
                          {p.queued !== null ? Math.round(p.queued) : t("na")}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-xs">
                          {p.avg_ttft !== null ? formatDuration(p.avg_ttft) : "–"}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-xs">
                          {p.p50 > 0 ? formatDuration(p.p50) : "–"}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-xs">
                          {p.p95 > 0 ? formatDuration(p.p95) : "–"}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-xs">
                          {decode !== null ? (
                            <div className="flex items-center justify-end gap-1.5">
                              {formatTps(decode, locale)}
                              <Sparkline
                                samples={historyRef.current[name]?.map((s) => s.decode) ?? []}
                                color="#10b981"
                                label={`${t("decode_tps")}: ${formatTps(decode, locale)}`}
                              />
                            </div>
                          ) : (
                            "–"
                          )}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-xs">
                          {p.kv_cache_usage !== null ? (
                            <div className="flex items-center justify-end gap-1.5">
                              {formatPercent(p.kv_cache_usage * 100, locale)}
                              <Sparkline
                                samples={historyRef.current[name]?.map((s) => s.kv) ?? []}
                                color="#f59e0b"
                                scaleMax={1}
                                label={`${t("kv_cache")}: ${formatPercent(p.kv_cache_usage * 100, locale)}`}
                              />
                            </div>
                          ) : (
                            "–"
                          )}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-xs">
                          {formatTps(prefill, locale)}
                        </TableCell>
                        <TableCell className={`text-right font-mono tabular-nums text-xs ${errorRateClass(rate)}`}>
                          {p.reqs > 0 ? formatPercent(rate, locale) : "–"}
                        </TableCell>
                        <TableCell className="pr-4 text-right font-mono tabular-nums text-xs">
                          {formatCost(p.cost_usd)}
                        </TableCell>
                      </TableRow>
                      {orderLiveRows(
                        trackedEntries.filter((tr) => entryBelongsToRow(tr.entry, p))
                      ).map((tr) => (
                          <LiveRequestRow
                            key={tr.entry.request_id}
                            tr={tr}
                            now={now}
                            t={t}
                            formatNumber={formatNumber}
                            formatDuration={formatDuration}
                            formatTps={formatTps}
                            locale={locale}
                          />
                        ))}
                      </Fragment>
                    );
                  })}
                  {orderLiveRows(
                    trackedEntries.filter(
                      (tr) => !liveRows.some((r) => entryBelongsToRow(tr.entry, r.p)),
                    )
                  ).map((tr) => (
                      <LiveRequestRow
                        key={tr.entry.request_id}
                        tr={tr}
                        now={now}
                        t={t}
                        formatNumber={formatNumber}
                        formatDuration={formatDuration}
                        formatTps={formatTps}
                        locale={locale}
                      />
                    ))}
                </TableBody>
              </Table>
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">{t("no_data")}</p>
            )}
          </CardContent>
        </Card>
      )}

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              {t("requests")}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="tabular-nums text-2xl font-bold">
              {stats ? formatNumber(stats.total_requests) : "–"}
            </div>
            {stats && stats.total_requests > 0 && (
              <p className="text-xs text-muted-foreground">
                {t("success_rate", {
                  pct: formatPercent(
                    (stats.success_requests / stats.total_requests) * 100,
                    locale,
                  ),
                })}
              </p>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              {t("cost")}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="truncate tabular-nums text-2xl font-bold">
              {stats ? formatCost(stats.total_cost) : "–"}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              {t("tokens")}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="truncate tabular-nums text-2xl font-bold">
              {stats
                ? formatNumber(stats.total_prompt_tokens + stats.total_completion_tokens)
                : "–"}
            </div>
            {stats && (
              <p className="text-xs text-muted-foreground">
                {t("tokens_in_out", {
                  in: formatNumber(stats.total_prompt_tokens),
                  out: formatNumber(stats.total_completion_tokens),
                })}
              </p>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              {t("avg_latency")}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="tabular-nums text-2xl font-bold">
              {stats ? formatDuration(stats.avg_duration_ms) : "–"}
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t("requests_cost_over_time")}</CardTitle>
        </CardHeader>
        <CardContent>
          {chartData.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("no_data_range")}</p>
          ) : (
            <div className="h-72">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={chartData}>
                  <defs>
                    <linearGradient id="colorRequests" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#10b981" stopOpacity={0.3} />
                      <stop offset="100%" stopColor="#10b981" stopOpacity={0} />
                    </linearGradient>
                    <linearGradient id="colorCost" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#8b5cf6" stopOpacity={0.3} />
                      <stop offset="100%" stopColor="#8b5cf6" stopOpacity={0} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" stroke={chart.grid} />
                  <XAxis dataKey="time" stroke={chart.axis} fontSize={12} />
                  <YAxis yAxisId="left" stroke={chart.axis} fontSize={12} />
                  <YAxis
                    yAxisId="right"
                    orientation="right"
                    stroke={chart.axis}
                    fontSize={12}
                    tickFormatter={(v) => formatCost(Number(v))}
                  />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: chart.tooltipBg,
                      border: `1px solid ${chart.tooltipBorder}`,
                      borderRadius: 8,
                    }}
                    formatter={(value, name) =>
                      name === t("cost") ? formatCost(Number(value)) : value
                    }
                  />
                  <Area
                    yAxisId="left"
                    type="monotone"
                    dataKey="requests"
                    stroke="#10b981"
                    fill="url(#colorRequests)"
                    name={t("requests")}
                    isAnimationActive={false}
                  />
                  <Area
                    yAxisId="right"
                    type="monotone"
                    dataKey="cost"
                    stroke="#8b5cf6"
                    fill="url(#colorCost)"
                    name={t("cost")}
                    isAnimationActive={false}
                  />
                </AreaChart>
              </ResponsiveContainer>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("cost_per_model")}</CardTitle>
        </CardHeader>
        <CardContent>
          {byModel.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("no_data")}</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("model")}</TableHead>
                  <TableHead>{t("provider")}</TableHead>
                  <TableHead className="text-right">{t("requests")}</TableHead>
                  <TableHead className="text-right">{t("tokens")}</TableHead>
                  <TableHead className="text-right">{t("cost")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {byModel.map((m) => (
                  <TableRow key={`${m.group}-${m.provider_name ?? ""}`}>
                    <TableCell className="font-medium">{m.group}</TableCell>
                    <TableCell className="text-muted-foreground">
                      {m.provider_name || "–"}
                    </TableCell>
                    <TableCell className="text-right">{formatNumber(m.requests)}</TableCell>
                    <TableCell className="text-right">{formatNumber(m.tokens)}</TableCell>
                    <TableCell className="text-right">{formatCost(m.cost)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

/** Eingerueckte sub-zeile unter einer provider-zeile: laufende oder frisch
 *  abgeschlossene einzelanfrage (quelle: 1s-poll + sse-completed). */
function LiveRequestRow({
  tr,
  now,
  t,
  formatNumber,
  formatDuration,
  formatTps,
  locale,
}: {
  tr: TrackedRequest;
  now: number;
  t: (key: TranslationKey, params?: Record<string, string | number>) => string;
  formatNumber: (v: number) => string;
  formatDuration: (ms: number) => string;
  formatTps: (v: number | null, locale?: string) => string;
  locale: string;
}) {
  const c = tr.completed;
  const rates = c ? completedRates(c) : null;
  return (
    <TableRow
      className={cn(
        "bg-muted/40 transition-opacity duration-[2000ms]",
        tr.hiddenAt === null ? "opacity-100" : "opacity-0",
      )}
    >
      <TableCell colSpan={11} className="overflow-hidden py-1.5 pl-10 pr-4 text-xs">
        <div className="flex items-center gap-x-3 whitespace-nowrap">
          <span
            className={cn(
              "h-1.5 w-1.5 shrink-0 rounded-full",
              c ? "bg-emerald-500" : "animate-pulse bg-sky-500",
            )}
          />
          {c ? (
            <>
              <span className="shrink-0 font-medium text-emerald-600 dark:text-emerald-400">{t("completed")}</span>
              <span className="min-w-0 max-w-56 truncate font-medium">{tr.entry.model}</span>
              <span className="min-w-0 max-w-32 truncate text-muted-foreground">{tr.entry.key_name}</span>
              <span className="shrink-0">
                {t("prefill_tokens")}:{" "}
                <span className="font-mono tabular-nums">{formatNumber(c.prompt_tokens)}</span>
              </span>
              <span className="shrink-0">
                {t("decode_tokens")}:{" "}
                <span className="font-mono tabular-nums">{formatNumber(c.completion_tokens)}</span>
              </span>
              <span className="shrink-0">
                {t("prefill_tps")}:{" "}
                <span className="font-mono tabular-nums">{formatTps(rates!.prefill, locale)}</span>
              </span>
              <span className="shrink-0">
                {t("decode_tps")}:{" "}
                <span className="font-mono tabular-nums">{formatTps(rates!.decode, locale)}</span>
              </span>
              <span className="ml-auto shrink-0 pl-3 font-mono tabular-nums">{formatDuration(c.duration_ms)}</span>
            </>
          ) : (
            <>
              <span className="min-w-0 max-w-56 truncate font-medium">{tr.entry.model}</span>
              <span className="min-w-0 max-w-32 truncate text-muted-foreground">{tr.entry.key_name}</span>
              <span className="shrink-0">
                {t("running_for", { d: formatDuration(Math.max(0, now - tr.entry.started_at_ms)) })}
              </span>
              <span className="ml-auto shrink-0 pl-3 text-muted-foreground">
                {t("ttft")}:{" "}
                {tr.entry.first_byte_ms !== null ? formatDuration(tr.entry.first_byte_ms) : t("first_token_pending")}
              </span>
            </>
          )}
        </div>
      </TableCell>
    </TableRow>
  );
}
