import { useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api, LogEntry, VirtualKey, LiveEvent } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ColumnToggle } from "@/components/ColumnToggle";
import { ElapsedTimer } from "@/components/ElapsedTimer";
import { useToast } from "@/components/ui/toast";
import { ColumnDef, useColumnVisibility } from "@/lib/columns";
import { tps } from "@/lib/format";
import { useLanguage, useCurrency } from "@/lib/i18n";

function statusBadge(status: number) {
  if (status >= 200 && status < 300) return <Badge variant="success">{status}</Badge>;
  if (status >= 400) return <Badge variant="error">{status}</Badge>;
  return <Badge variant="outline">{status}</Badge>;
}

interface InFlight {
  request_id: string;
  startedAt: number;
  key_name: string;
  provider: string;
  provider_name: string;
  model: string;
  is_stream: boolean;
  firstByteMs: number | null;
}

const PAGE_SIZES = [25, 50, 100, 250];

export default function Requests() {
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [keys, setKeys] = useState<VirtualKey[]>([]);
  const [keyName, setKeyName] = useState("");
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [hours, setHours] = useState(24);
  const [live, setLive] = useState(true);
  const [inFlight, setInFlight] = useState<InFlight[]>([]);
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState(50);
  const [total, setTotal] = useState(0);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadedOnce, setLoadedOnce] = useState(false);
  const { t, formatDateTime, formatNumber, formatDuration, locale } = useLanguage();
  const { formatCost } = useCurrency();
  const toast = useToast();
  const filtersRef = useRef({ keyName, provider, model, statusFilter, hours });
  const pageRef = useRef(page);
  const pageSizeRef = useRef(pageSize);

  // latest-refs: der SSE-handler liest filter/page/page-size, ohne bei jeder
  // aenderung die verbindung neu aufbauen zu muessen
  useEffect(() => {
    filtersRef.current = { keyName, provider, model, statusFilter, hours };
  }, [keyName, provider, model, statusFilter, hours]);
  useEffect(() => {
    pageRef.current = page;
  }, [page]);
  useEffect(() => {
    pageSizeRef.current = pageSize;
  }, [pageSize]);

  // Live-Infos fuer laufende requests: pro column-id ein renderer,
  // der den in-flight-zustand statt der fertigen log-daten anzeigt
  const liveCell = (r: InFlight, colId: string): React.ReactNode => {
    switch (colId) {
      case "time":
        return (
          <span className="inline-flex items-center gap-2">
            <span className="inline-block h-2 w-2 animate-pulse rounded-full bg-emerald-400" />
            {formatDateTime(new Date(r.startedAt))}
          </span>
        );
      case "key":
        return r.key_name || "–";
      case "provider_name":
        return r.provider_name || "–";
      case "provider":
        return r.provider || "–";
      case "model":
        return r.model || "–";
      case "stream":
        return r.is_stream ? "✓" : "";
      case "prompt_tokens":
        return "–";
      case "completion_tokens":
      case "total_tokens":
        return r.firstByteMs !== null ? (
          <span className="text-xs text-muted-foreground">
            {t("first_token", { ms: formatDuration(r.firstByteMs) })}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">–</span>
        );
      case "cost":
        return "–";
      case "ttft":
        return r.firstByteMs !== null ? formatDuration(r.firstByteMs) : "–";
      case "prefill":
        return "–";
      case "decode":
        return "–";
      case "duration":
        return <ElapsedTimer startedAt={r.startedAt} format={formatDuration} />;
      default:
        return null;
    }
  };

  const columns: ColumnDef<LogEntry>[] = useMemo(
    () => [
      {
        id: "time",
        label: t("time"),
        className: "pl-4 text-muted-foreground",
        render: (log) => formatDateTime(log.timestamp),
      },
      {
        id: "key",
        label: t("key"),
        className: "font-medium",
        render: (log) => (
          <Link to={`/requests/${log.id}`} className="hover:underline">
            {log.key_name || "–"}
          </Link>
        ),
      },
      {
        id: "provider_name",
        label: t("provider"),
        render: (log) => log.provider_name || "–",
      },
      {
        id: "provider",
        label: t("type"),
        className: "text-muted-foreground",
        render: (log) => log.provider || "–",
      },
      {
        id: "model",
        label: t("model"),
        render: (log) => log.model || "–",
      },
      {
        id: "status",
        label: t("status"),
        render: (log) => statusBadge(log.status),
      },
      {
        id: "stream",
        label: t("stream"),
        render: (log) => (log.is_stream ? "✓" : ""),
      },
      {
        id: "prompt_tokens",
        label: t("prompt_tokens"),
        className: "text-right",
        render: (log) => (log.prompt_tokens > 0 ? formatNumber(log.prompt_tokens) : "–"),
      },
      {
        id: "completion_tokens",
        label: t("completion_tokens"),
        className: "text-right",
        render: (log) => (log.completion_tokens > 0 ? formatNumber(log.completion_tokens) : "–"),
      },
      {
        id: "total_tokens",
        label: t("total_tokens"),
        className: "text-right",
        render: (log) => {
          const total = log.prompt_tokens + log.completion_tokens;
          return total > 0 ? formatNumber(total) : "–";
        },
      },
      {
        id: "cost",
        label: t("cost"),
        className: "text-right",
        render: (log) => formatCost(log.cost_usd),
      },
      {
        id: "ttft",
        label: "TTFT",
        className: "text-right",
        render: (log) => (log.first_byte_ms > 0 ? formatDuration(log.first_byte_ms) : "–"),
      },
      {
        id: "prefill",
        label: t("prefill_tps"),
        className: "text-right",
        render: (log) => (log.prompt_tokens > 0 ? tps(log.prompt_tokens, log.first_byte_ms, locale) : "–"),
      },
      {
        id: "decode",
        label: t("decode_tps"),
        className: "text-right",
        render: (log) => {
          const decodeMs = log.duration_ms - log.first_byte_ms;
          return log.completion_tokens > 0 && decodeMs > 0
            ? tps(log.completion_tokens, decodeMs, locale)
            : "–";
        },
      },
      {
        id: "duration",
        label: t("duration"),
        className: "text-right",
        render: (log) => formatDuration(log.duration_ms),
      },
    ],
    [t, formatDateTime, formatDuration, formatNumber, formatCost, locale]
  );

  const { hidden, toggle, reset, visibleColumns } = useColumnVisibility("requests", columns);

  useEffect(() => {
    api
      .get<{ keys: VirtualKey[] }>("/keys")
      .then((r) => setKeys(r.keys))
      .catch((e) => toast.error(e instanceof Error ? e.message : t("load_failed")));
  }, [toast, t]);

  useEffect(() => {
    const params = new URLSearchParams({
      hours: String(hours),
      limit: String(pageSize),
      offset: String(page * pageSize),
    });
    if (keyName) params.set("key_name", keyName);
    if (provider) params.set("provider", provider);
    if (model) params.set("model", model);
    if (statusFilter) params.set("status", statusFilter);
    api
      .get<{ logs: LogEntry[]; total: number }>(`/logs?${params.toString()}`)
      .then((r) => {
        setLogs(r.logs);
        setTotal(r.total);
        setLoadError(null);
        setLoadedOnce(true);
      })
      .catch((e) => {
        setLoadError(e instanceof Error ? e.message : t("load_failed"));
        setLoadedOnce(true);
      });
  }, [keyName, provider, model, statusFilter, hours, page, pageSize, t]);

  // filterwechsel -> zurueck auf seite 1
  const resetPage = () => setPage(0);

  const totalPages = Math.max(1, Math.ceil(total / pageSize));

  // Live-Modus: SSE-Verbindung (ruecksetzen der liste passiert im toggle-handler)
  useEffect(() => {
    if (!live) return;
    const es = new EventSource("/dashboard-api/live");
    es.addEventListener("request_started", (e) => {
      const ev = JSON.parse((e as MessageEvent).data) as Extract<LiveEvent, { type: "request_started" }>;
      setInFlight((prev) => [
        {
          request_id: ev.request_id,
          startedAt: Date.parse(ev.timestamp),
          key_name: ev.key_name,
          provider: ev.provider,
          provider_name: ev.provider_name,
          model: ev.model,
          is_stream: ev.is_stream,
          firstByteMs: null,
        },
        ...prev,
      ]);
    });
    es.addEventListener("first_byte", (e) => {
      const ev = JSON.parse((e as MessageEvent).data) as Extract<LiveEvent, { type: "first_byte" }>;
      setInFlight((prev) =>
        prev.map((r) =>
          r.request_id === ev.request_id ? { ...r, firstByteMs: ev.first_byte_ms } : r
        )
      );
    });
    es.addEventListener("completed", (e) => {
      const ev = JSON.parse((e as MessageEvent).data) as Extract<LiveEvent, { type: "completed" }>;
      setInFlight((prev) => prev.filter((r) => r.request_id !== ev.log.request_id));
      const log = ev.log as LogEntry;
      const f = filtersRef.current;
      if (f.keyName && log.key_name !== f.keyName) return;
      if (f.provider && log.provider !== f.provider) return;
      if (f.model && !log.model.toLowerCase().includes(f.model.toLowerCase())) return;
      if (f.statusFilter) {
        const s = log.status;
        const bucket = s >= 200 && s < 300 ? "200" : s >= 400 && s < 500 ? "400" : "500";
        if (bucket !== f.statusFilter) return;
      }
      // live-prepend nur auf seite 1 (und kap auf page-size)
      if (pageRef.current !== 0) return;
      setLogs((prev) => [log, ...prev].slice(0, pageSizeRef.current));
      setTotal((prev) => prev + 1);
    });
    return () => es.close();
  }, [live]);

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-2xl font-bold">{t("nav_requests")}</h1>
        <div className="flex items-center gap-2">
          <Button
            variant={live ? "default" : "outline"}
            size="sm"
            onClick={() => {
              setLive((l) => {
                const next = !l;
                if (!next) setInFlight([]);
                return next;
              });
            }}
          >
            {live && (
              <span className="mr-2 inline-block h-2 w-2 animate-pulse rounded-full bg-emerald-400" />
            )}
            {t("live")}
          </Button>
          <ColumnToggle columns={columns} hidden={hidden} onToggle={toggle} onReset={reset} />
        </div>
      </div>

      <div className="flex flex-wrap gap-2">
        <Select className="w-48" value={keyName} onChange={(e) => { resetPage(); setKeyName(e.target.value); }}>
          <option value="">{t("all_keys")}</option>
          {keys.map((k) => (
            <option key={k.id} value={k.name}>
              {k.name}
            </option>
          ))}
        </Select>
        <Select className="w-40" value={provider} onChange={(e) => { resetPage(); setProvider(e.target.value); }}>
          <option value="">{t("all_providers")}</option>
          <option value="openai">openai</option>
          <option value="anthropic">anthropic</option>
          <option value="gemini">gemini</option>
          <option value="openai_compat">openai_compat</option>
        </Select>
        <Input
          className="w-44"
          placeholder={t("model_placeholder")}
          value={model}
          onChange={(e) => { resetPage(); setModel(e.target.value); }}
        />
        <Select className="w-36" value={statusFilter} onChange={(e) => { resetPage(); setStatusFilter(e.target.value); }}>
          <option value="">{t("all_status")}</option>
          <option value="200">2xx</option>
          <option value="400">4xx</option>
          <option value="500">5xx</option>
        </Select>
        <Select className="w-36" value={hours} onChange={(e) => { resetPage(); setHours(Number(e.target.value)); }}>
          <option value={1}>{t("last_hour")}</option>
          <option value={24}>{t("last_24h")}</option>
          <option value={168}>{t("last_7d")}</option>
          <option value={720}>{t("last_30d")}</option>
        </Select>
        <Select
          className="w-32"
          value={pageSize}
          onChange={(e) => { resetPage(); setPageSize(Number(e.target.value)); }}
        >
          {PAGE_SIZES.map((s) => (
            <option key={s} value={s}>
              {t("page_size", { n: s })}
            </option>
          ))}
        </Select>
      </div>

      {live && inFlight.length > 0 && (
        <Card>
          <CardContent className="p-0">
            <Table>
              <TableHeader>
                <TableRow>
                  {visibleColumns
                    .filter((col) => col.id !== "status")
                    .map((col) => (
                      <TableHead key={col.id} className={col.className}>
                        {col.label}
                      </TableHead>
                    ))}
                </TableRow>
              </TableHeader>
              <TableBody>
                {inFlight.map((r) => (
                  <TableRow key={r.request_id}>
                    {visibleColumns
                      .filter((col) => col.id !== "status")
                      .map((col) => (
                        <TableCell key={col.id} className={col.className}>
                          {liveCell(r, col.id)}
                        </TableCell>
                      ))}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                {visibleColumns.map((col) => (
                  <TableHead key={col.id} className={col.className}>
                    {col.label}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {!loadedOnce && !loadError && (
                <TableRow>
                  <TableCell colSpan={visibleColumns.length} className="p-8 text-center text-muted-foreground">
                    {t("loading")}
                  </TableCell>
                </TableRow>
              )}
              {loadError && logs.length === 0 && (
                <TableRow>
                  <TableCell colSpan={visibleColumns.length} className="p-8 text-center text-destructive">
                    {loadError}
                  </TableCell>
                </TableRow>
              )}
              {loadedOnce && !loadError && logs.length === 0 && (
                <TableRow>
                  <TableCell colSpan={visibleColumns.length} className="p-8 text-center text-muted-foreground">
                    {t("no_requests")}
                  </TableCell>
                </TableRow>
              )}
              {logs.map((log) => (
                <TableRow key={log.id}>
                  {visibleColumns.map((col) => (
                    <TableCell key={col.id} className={col.className}>
                      {col.render(log)}
                    </TableCell>
                  ))}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-muted-foreground">
        <span>
          {total > 0
            ? t("pagination_range", {
                from: formatNumber(page * pageSize + 1),
                to: formatNumber(Math.min((page + 1) * pageSize, total)),
                total: formatNumber(total),
              })
            : t("no_requests")}
        </span>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={page === 0}
            onClick={() => setPage((p) => Math.max(0, p - 1))}
          >
            ← {t("prev_page")}
          </Button>
          <span>
            {formatNumber(page + 1)} / {formatNumber(totalPages)}
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={page >= totalPages - 1}
            onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
          >
            {t("next_page")} →
          </Button>
        </div>
      </div>
    </div>
  );
}
