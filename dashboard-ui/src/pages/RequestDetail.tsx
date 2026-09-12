import { useCallback, useEffect, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { api, LogEntry } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { extractRequestText, extractStreamText, prettyJson } from "@/lib/logText";
import { useLanguage, useCurrency } from "@/lib/i18n";

interface LogDetail extends LogEntry {
  error_message: string;
  request_body: string;
  response_body: string;
  request_truncated: boolean;
  response_truncated: boolean;
}

export default function RequestDetail() {
  const { id } = useParams<{ id: string }>();
  const [log, setLog] = useState<LogDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showText, setShowText] = useState(true);
  const { t, formatDateTime, formatDuration } = useLanguage();
  const { formatCost } = useCurrency();

  const load = useCallback(async () => {
    if (!id) return;
    try {
      const log = await api.get<LogDetail>(`/logs/${id}`);
      setLog(log);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : t("load_failed"));
    }
  }, [id, t]);

  // daten beim mount/aendern der id nachladen: klassischer fetch-on-mount,
  // setState passiert asynchron nach dem fetch
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- setState laeuft erst nach dem async fetch, nicht synchron im effect
    void load();
  }, [load]);

  if (error) {
    return (
      <div className="space-y-4">
        <div className="flex flex-wrap items-center gap-4">
          <Link to="/requests">
            <Button variant="outline">{t("back")}</Button>
          </Link>
          <h1 className="text-2xl font-bold">{t("request_details")}</h1>
        </div>
        <p className="text-destructive">{error}</p>
        <Button variant="outline" size="sm" onClick={load}>
          {t("retry")}
        </Button>
      </div>
    );
  }
  if (!log) return <p>{t("loading")}</p>;

  // menschenlesbarer text als default anzeigen, wenn er sich extrahieren
  // laesst; sonst automatisch die roh-ansicht
  const requestText = log.request_body ? extractRequestText(log.request_body) : null;
  const streamText = log.is_stream && log.response_body ? extractStreamText(log.response_body) : null;
  const showRequestText = showText && requestText;
  const showStreamText = showText && streamText;

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center gap-4">
        <Link to="/requests">
          <Button variant="outline">{t("back")}</Button>
        </Link>
        <h1 className="text-2xl font-bold">{t("request_details")}</h1>
        <Badge variant={log.status >= 400 ? "error" : "success"}>{log.status}</Badge>
      </div>

      <div className="grid gap-4 md:grid-cols-3 lg:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("key")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium">{log.key_name || "–"}</CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("provider")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium">
            {log.provider} ({log.provider_name})
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("model")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium break-all">
            {log.model}
            {log.upstream_model && log.upstream_model !== log.model && (
              <span className="text-muted-foreground"> → {log.upstream_model}</span>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("endpoint")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium">{log.endpoint}</CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("tokens")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium">
            {t("tokens_in_out", { in: log.prompt_tokens, out: log.completion_tokens })}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("cost")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium">{formatCost(log.cost_usd)}</CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("duration")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium">
            {formatDuration(log.duration_ms)}
            {log.is_stream && ` (TTFT: ${formatDuration(log.first_byte_ms)})`}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm text-muted-foreground">{t("timestamp")}</CardTitle>
          </CardHeader>
          <CardContent className="font-medium">{formatDateTime(log.timestamp)}</CardContent>
        </Card>
      </div>

      {log.error_message && (
        <Card className="border-destructive">
          <CardHeader>
            <CardTitle className="text-destructive">
              {t("error", { type: log.error_type || "error" })}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="whitespace-pre-wrap break-all text-sm">{log.error_message}</pre>
          </CardContent>
        </Card>
      )}

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 space-y-0">
            <div className="flex items-center gap-2">
              <CardTitle>{t("request_body")}</CardTitle>
              {log.request_truncated && (
                <Badge variant="outline" className="border-yellow-500 text-yellow-500">
                  {t("truncated_warning")}
                </Badge>
              )}
            </div>
            {requestText && (
              <div className="flex gap-1">
                <Button size="sm" variant={showText ? "ghost" : "default"} onClick={() => setShowText(false)}>
                  {t("view_raw")}
                </Button>
                <Button size="sm" variant={showText ? "default" : "ghost"} onClick={() => setShowText(true)}>
                  {t("view_text")}
                </Button>
              </div>
            )}
          </CardHeader>
          <CardContent>
            {showRequestText ? (
              <pre className="max-h-96 overflow-auto whitespace-pre-wrap rounded-md bg-muted p-4 text-sm">
                {requestText.content || "–"}
              </pre>
            ) : (
              <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted p-4 text-xs">
                {log.request_body ? prettyJson(log.request_body) : "–"}
              </pre>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 space-y-0">
            <div className="flex items-center gap-2">
              <CardTitle>{t("response_body")}</CardTitle>
              {log.response_truncated && (
                <Badge variant="outline" className="border-yellow-500 text-yellow-500">
                  {t("truncated_warning")}
                </Badge>
              )}
            </div>
            {streamText && (
              <div className="flex gap-1">
                <Button size="sm" variant={showText ? "ghost" : "default"} onClick={() => setShowText(false)}>
                  {t("view_raw")}
                </Button>
                <Button size="sm" variant={showText ? "default" : "ghost"} onClick={() => setShowText(true)}>
                  {t("view_text")}
                </Button>
              </div>
            )}
          </CardHeader>
          <CardContent>
            {showStreamText ? (
              <div className="max-h-96 space-y-4 overflow-auto rounded-md bg-muted p-4 text-sm">
                {streamText.reasoning && (
                  <div>
                    <p className="mb-1 text-xs font-medium text-muted-foreground">{t("reasoning")}</p>
                    <pre className="whitespace-pre-wrap break-all font-mono text-xs text-muted-foreground">
                      {streamText.reasoning}
                    </pre>
                  </div>
                )}
                <div>
                  <p className="mb-1 text-xs font-medium text-muted-foreground">{t("response_text")}</p>
                  <pre className="whitespace-pre-wrap break-all font-mono text-sm">{streamText.content || "–"}</pre>
                </div>
                {streamText.toolCalls.length > 0 && (
                  <div>
                    <p className="mb-1 text-xs font-medium text-muted-foreground">{t("tool_calls")}</p>
                    {streamText.toolCalls.map((tc, i) => (
                      <pre key={i} className="whitespace-pre-wrap break-all font-mono text-xs">
                        {tc.name}({tc.arguments})
                      </pre>
                    ))}
                  </div>
                )}
              </div>
            ) : (
              <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted p-4 text-xs">
                {log.response_body ? prettyJson(log.response_body) : "–"}
              </pre>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
