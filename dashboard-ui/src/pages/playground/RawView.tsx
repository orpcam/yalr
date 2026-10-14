import { Copy } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useToast } from "@/components/ui/toast";
import { useLanguage } from "@/lib/i18n";
import type { Exchange } from "./ChatPanel";
import { prettyRaw, routingHeaders } from "./helpers";

async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) return navigator.clipboard.writeText(text);
  // fallback fuer nicht-sichere kontexte (http ohne localhost)
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  const ok = document.execCommand("copy");
  document.body.removeChild(ta);
  if (!ok) throw new Error("copy failed");
}

function Block({ title, text }: { title: string; text: string }) {
  const { t } = useLanguage();
  const toast = useToast();
  return (
    <Card className="min-w-0">
      <CardHeader className="flex flex-row items-center justify-between space-y-0 p-4">
        <CardTitle className="text-base">{title}</CardTitle>
        <Button
          variant="outline"
          size="sm"
          disabled={!text}
          onClick={() =>
            copyText(text)
              .then(() => toast.success(t("copied")))
              .catch(() => toast.error(t("pg_copy_failed")))
          }
        >
          <Copy className="mr-1.5 h-3.5 w-3.5" />
          {t("pg_copy")}
        </Button>
      </CardHeader>
      <CardContent className="p-4 pt-0">
        {text ? (
          <pre className="max-h-[50vh] overflow-auto whitespace-pre-wrap break-all rounded border bg-muted/40 p-3 font-mono text-xs">
            {text}
          </pre>
        ) : (
          <p className="text-sm text-muted-foreground">{t("pg_raw_empty")}</p>
        )}
      </CardContent>
    </Card>
  );
}

export function RawView({ exchange }: { exchange: Exchange | null }) {
  const { t } = useLanguage();
  const request = exchange?.request !== undefined ? JSON.stringify(exchange.request, null, 2) : "";
  let response = "";
  if (exchange?.rawText !== undefined) {
    response = prettyRaw(exchange.rawText);
    // abgebrochene/fehlerhafte antwort: teil-rohtext plus fehlerhinweis
    if (exchange.error) response = response ? `${response}\n\n[${exchange.error}]` : `[${exchange.error}]`;
  }
  else if (exchange?.error) response = exchange.status !== undefined ? `HTTP ${exchange.status}\n${exchange.error}` : exchange.error;
  const headers = exchange?.routing
    ? [
        ...(exchange.status !== undefined ? [`status: ${exchange.status}`] : []),
        ...routingHeaders(exchange.routing).map(([k, v]) => `${k}: ${v}`),
      ].join("\n")
    : "";

  return (
    <div className="grid min-w-0 gap-4 xl:grid-cols-2">
      <Block title={t("pg_raw_request")} text={request} />
      <Block title={t("pg_raw_response")} text={response} />
      <div className="xl:col-span-2">
        <Block title={t("pg_raw_headers")} text={headers} />
      </div>
    </div>
  );
}
