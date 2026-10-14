import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { ImagePlus, Send, Square, Trash2, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useToast } from "@/components/ui/toast";
import { formatTps } from "@/lib/format";
import { useLanguage, type TranslationKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { MAX_BODY_BYTES, buildChatRequest, estimateBodySize } from "./buildRequest";
import { NetworkError, PlaygroundError, StreamInterruptedError, classifyCompletion, sendChat } from "./client";
import {
  TEXTAREA_CLASS,
  formatBytes,
  imageRejection,
  toChatParams,
  toHistory,
  type NumberField,
  type ParamsState,
  type UiMessage,
} from "./helpers";
import { resizeImageFile } from "./image";
import { createMetrics } from "./metrics";
import type { RoutingInfo, Usage } from "./types";

/** Letzter request/response fuer die raw-ansicht */
export interface Exchange {
  request?: unknown;
  rawText?: string;
  status?: number;
  routing?: RoutingInfo;
  error?: string;
}

const FIELD_LABEL: Record<NumberField, TranslationKey> = {
  temperature: "pg_temperature",
  maxTokens: "pg_max_tokens",
  topP: "pg_top_p",
};

/** Kompakte routing-zeile: provider/modell, fallback-badge, versuche, request-link */
export function RoutingInfoView({ routing }: { routing?: RoutingInfo }) {
  const { t } = useLanguage();
  if (!routing) return null;
  const served = [routing.provider, routing.model].filter(Boolean).join(" / ");
  return (
    <>
      {served && (
        <span title={routing.upstreamModel ? `upstream: ${routing.upstreamModel}` : undefined} className="break-all">
          {served}
        </span>
      )}
      {routing.fallback && (
        <Badge variant="outline" className="border-amber-500/40 text-amber-600 dark:text-amber-400">
          {t("pg_fallback")}
        </Badge>
      )}
      {routing.attempts !== undefined && routing.attempts > 1 && (
        <span>{t("pg_attempts", { n: routing.attempts })}</span>
      )}
      {routing.requestId && (
        <Link to={`/requests/${routing.requestId}`} className="break-all underline-offset-2 hover:underline">
          {t("pg_request_link", { id: routing.requestId.slice(0, 8) })}
        </Link>
      )}
    </>
  );
}

function MessageView({ m }: { m: UiMessage }) {
  const { t, formatDuration, locale } = useLanguage();
  const isUser = m.role === "user";
  const mt = m.metrics;
  return (
    <div className={cn("flex", isUser ? "justify-end" : "justify-start")}>
      <div
        className={cn(
          "min-w-0 max-w-[90%] space-y-2 rounded-lg border px-3 py-2 text-sm",
          isUser ? "bg-accent" : "bg-card",
          m.status === "error" && "border-destructive/50"
        )}
      >
        <div className="text-xs font-medium text-muted-foreground">
          {isUser ? t("pg_you") : t("pg_assistant")}
          {m.status === "streaming" && <span className="ml-2 animate-pulse">{t("pg_generating")}</span>}
          {m.status === "stopped" && (
            <Badge variant="outline" className="ml-2">
              {t("pg_stopped")}
            </Badge>
          )}
        </div>
        {m.images && m.images.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {m.images.map((src, i) => (
              <img key={i} src={src} alt="" className="h-20 w-20 rounded border object-cover" />
            ))}
          </div>
        )}
        {m.reasoning && (
          <details className="rounded border bg-muted/40 px-2 py-1 text-xs">
            <summary className="cursor-pointer select-none text-muted-foreground">{t("pg_thinking")}</summary>
            <div className="mt-1 whitespace-pre-wrap break-words text-muted-foreground">{m.reasoning}</div>
          </details>
        )}
        {m.text && <div className="whitespace-pre-wrap break-words">{m.text}</div>}
        {m.unsupported && (
          <div className="space-y-1">
            <div className="text-xs text-amber-600 dark:text-amber-400">
              {t(m.unsupported.reason === "tool_calls" ? "pg_unsupported_tool_calls" : "pg_unsupported_content")}
            </div>
            <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded border bg-muted/40 p-2 font-mono text-xs">
              {m.unsupported.json}
            </pre>
          </div>
        )}
        {m.error && (
          <div className="break-words text-destructive">
            {m.error.status !== undefined && (
              <span className="font-medium">{t("pg_error_status", { status: m.error.status })}: </span>
            )}
            {m.error.message}
          </div>
        )}
        {m.streamErrors?.map((e, i) => (
          <div key={i} className="break-words text-destructive">
            {t("pg_stream_error", { msg: e })}
          </div>
        ))}
        {m.truncated && <div className="text-xs text-amber-600 dark:text-amber-400">{t("pg_truncated")}</div>}
        {!isUser && m.status !== "streaming" && (mt || m.routing) && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t pt-1.5 text-xs text-muted-foreground">
            {mt?.ttftMs !== undefined && <span>{t("pg_m_ttft", { v: formatDuration(mt.ttftMs) })}</span>}
            {mt && <span>{t("pg_m_total", { v: formatDuration(mt.totalMs) })}</span>}
            {mt?.completionTokens !== undefined && (
              <span>
                {mt.estimated ? "≈" : ""}
                {t("pg_m_tokens", { n: mt.completionTokens })}
              </span>
            )}
            {mt?.tokensPerSec !== undefined && (
              <span>
                {mt.estimated ? "≈" : ""}
                {formatTps(mt.tokensPerSec, locale)}
              </span>
            )}
            <RoutingInfoView routing={m.routing} />
          </div>
        )}
      </div>
    </div>
  );
}

export function ChatPanel({
  params,
  onExchange,
}: {
  params: ParamsState;
  onExchange: (e: Exchange) => void;
}) {
  const { t } = useLanguage();
  const toast = useToast();
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [input, setInput] = useState("");
  const [images, setImages] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const nextId = useRef(1);
  const listRef = useRef<HTMLDivElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // laufenden request beim verlassen der seite abbrechen
  useEffect(() => () => abortRef.current?.abort(), []);

  // neue tokens -> ans ende scrollen
  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const patch = (id: number, fn: (m: UiMessage) => UiMessage) =>
    setMessages((prev) => prev.map((m) => (m.id === id ? fn(m) : m)));

  const addFiles = async (files: FileList | null) => {
    if (!files) return;
    for (const f of Array.from(files)) {
      const rej = imageRejection(f.type);
      if (rej) {
        toast.error(
          t("pg_image_error", { name: f.name, msg: t(rej === "gif" ? "pg_image_gif" : "pg_image_not_image") })
        );
        continue;
      }
      try {
        const url = await resizeImageFile(f);
        setImages((prev) => [...prev, url]);
      } catch (e) {
        toast.error(t("pg_image_error", { name: f.name, msg: e instanceof Error ? e.message : String(e) }));
      }
    }
  };

  const send = async () => {
    if (busy) return;
    const text = input.trim();
    if (!text && images.length === 0) return;
    setSendError(null);
    if (!params.model) {
      setSendError(t("pg_no_model"));
      return;
    }
    const { params: cp, invalid } = toChatParams(params);
    if (invalid.length > 0) {
      setSendError(invalid.map((f) => t("pg_invalid_number", { field: t(FIELD_LABEL[f]) })).join(" "));
      return;
    }
    const userMsg: UiMessage = {
      id: nextId.current++,
      role: "user",
      text,
      images: images.length ? images : undefined,
    };
    const { body, error } = buildChatRequest(cp, toHistory([...messages, userMsg]));
    if (error) {
      setSendError(error);
      return;
    }
    const size = estimateBodySize(body);
    if (size > MAX_BODY_BYTES) {
      setSendError(t("pg_body_too_large", { size: formatBytes(size), max: formatBytes(MAX_BODY_BYTES) }));
      return;
    }

    const aid = nextId.current++;
    setMessages((prev) => [...prev, userMsg, { id: aid, role: "assistant", text: "", status: "streaming" }]);
    setInput("");
    setImages([]);
    setBusy(true);
    onExchange({ request: body });

    const controller = new AbortController();
    abortRef.current = controller;
    const metrics = createMetrics(performance.now());
    let usage: Usage | undefined;

    try {
      const res = await sendChat(body, {
        signal: controller.signal,
        provider: cp.provider,
        onEvent: (e) => {
          if (e.type === "content") {
            metrics.delta(e.text.length);
            patch(aid, (m) => ({ ...m, text: m.text + e.text }));
          } else if (e.type === "reasoning") {
            metrics.delta(e.text.length);
            patch(aid, (m) => ({ ...m, reasoning: (m.reasoning ?? "") + e.text }));
          } else if (e.type === "usage") {
            usage = e.usage;
          } else if (e.type === "error") {
            const msg = e.message;
            patch(aid, (m) => ({ ...m, streamErrors: [...(m.streamErrors ?? []), msg] }));
          }
        },
      });
      const exchange: Exchange = { request: body, rawText: res.rawText, status: res.status, routing: res.routing };
      let content: string | undefined;
      let reasoning: string | undefined;
      let unsupported: UiMessage["unsupported"];
      if (res.json !== undefined) {
        const c = classifyCompletion(res.json);
        if (c.kind === "error") {
          const message = c.message;
          patch(aid, (m) => ({ ...m, status: "error", error: { status: res.status, message }, routing: res.routing }));
          onExchange({ ...exchange, error: message });
          return;
        }
        content = c.kind === "ok" ? c.content : "";
        reasoning = c.reasoning || undefined;
        usage = c.usage;
        if (c.kind === "unsupported") unsupported = { reason: c.reason, json: c.json };
      }
      const summary = metrics.summary(usage, performance.now());
      patch(aid, (m) => ({
        ...m,
        text: content ?? m.text,
        reasoning: reasoning ?? m.reasoning,
        status: "done",
        routing: res.routing,
        metrics: summary,
        truncated: res.truncated,
        unsupported,
      }));
      onExchange(exchange);
    } catch (e) {
      // nach den headern abgebrochen: routing + bisheriger rohtext bleiben erhalten
      const interrupted = e instanceof StreamInterruptedError ? e : undefined;
      const routing = e instanceof PlaygroundError ? e.routing : undefined;
      if (controller.signal.aborted) {
        const summary = metrics.summary(usage, performance.now());
        patch(aid, (m) => ({ ...m, status: "stopped", metrics: summary, routing: routing ?? m.routing }));
        onExchange({
          request: body,
          rawText: interrupted?.rawText,
          status: interrupted?.status,
          routing,
          error: "aborted",
        });
      } else if (interrupted) {
        const summary = metrics.summary(usage, performance.now());
        const message = t("pg_stream_interrupted", { msg: interrupted.message });
        patch(aid, (m) => ({
          ...m,
          status: "error",
          error: { status: interrupted.status, message },
          routing: interrupted.routing,
          metrics: summary,
        }));
        onExchange({
          request: body,
          rawText: interrupted.rawText,
          status: interrupted.status,
          routing: interrupted.routing,
          error: message,
        });
      } else if (e instanceof PlaygroundError) {
        patch(aid, (m) => ({ ...m, status: "error", error: { status: e.status, message: e.message }, routing: e.routing }));
        onExchange({ request: body, status: e.status, routing: e.routing, error: e.message });
      } else {
        const msg =
          e instanceof NetworkError
            ? t("pg_network_error", { msg: e.message })
            : e instanceof Error
              ? e.message
              : String(e);
        patch(aid, (m) => ({ ...m, status: "error", error: { message: msg } }));
        onExchange({ request: body, error: msg });
      }
    } finally {
      if (abortRef.current === controller) abortRef.current = null;
      setBusy(false);
    }
  };

  const stop = () => abortRef.current?.abort();

  const clear = () => {
    abortRef.current?.abort();
    setMessages([]);
    setSendError(null);
  };

  return (
    <Card className="flex min-w-0 flex-col">
      <CardContent className="flex min-h-0 flex-1 flex-col gap-3 p-3 sm:p-4">
        <div ref={listRef} className="max-h-[60vh] min-h-[200px] flex-1 space-y-3 overflow-y-auto overflow-x-hidden">
          {messages.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">{t("pg_empty_chat")}</p>
          ) : (
            messages.map((m) => <MessageView key={m.id} m={m} />)
          )}
        </div>

        {images.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {images.map((src, i) => (
              <div key={i} className="relative">
                <img src={src} alt="" className="h-16 w-16 rounded border object-cover" />
                <button
                  type="button"
                  aria-label={t("pg_remove_image")}
                  title={t("pg_remove_image")}
                  className="absolute -right-1.5 -top-1.5 rounded-full border bg-card p-0.5 text-muted-foreground hover:text-foreground"
                  onClick={() => setImages((prev) => prev.filter((_, j) => j !== i))}
                >
                  <X className="h-3 w-3" />
                </button>
              </div>
            ))}
          </div>
        )}

        {sendError && <p className="break-words text-sm text-destructive">{sendError}</p>}

        <textarea
          rows={3}
          className={TEXTAREA_CLASS}
          placeholder={t("pg_input_placeholder")}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <div className="flex flex-wrap items-center gap-2">
          <input
            ref={fileRef}
            type="file"
            accept="image/*"
            multiple
            className="hidden"
            onChange={(e) => {
              void addFiles(e.target.files);
              e.target.value = "";
            }}
          />
          <Button variant="outline" size="sm" onClick={() => fileRef.current?.click()} disabled={busy}>
            <ImagePlus className="mr-1.5 h-4 w-4" />
            {t("pg_attach")}
          </Button>
          <Button variant="ghost" size="sm" onClick={clear} disabled={messages.length === 0 && !busy}>
            <Trash2 className="mr-1.5 h-4 w-4" />
            {t("pg_clear")}
          </Button>
          <div className="ml-auto">
            {busy ? (
              <Button variant="destructive" onClick={stop}>
                <Square className="mr-1.5 h-4 w-4" />
                {t("pg_stop")}
              </Button>
            ) : (
              <Button onClick={() => void send()} disabled={!input.trim() && images.length === 0}>
                <Send className="mr-1.5 h-4 w-4" />
                {t("pg_send")}
              </Button>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
