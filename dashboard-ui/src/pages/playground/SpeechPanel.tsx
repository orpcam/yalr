import { useEffect, useRef, useState } from "react";
import { Download, Square, Volume2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label, Select } from "@/components/ui/select";
import { useLanguage } from "@/lib/i18n";
import { RoutingInfoView } from "./ChatPanel";
import { NetworkError, PlaygroundError, audioExtension, sendSpeech } from "./client";
import { TEXTAREA_CLASS, parseOptionalNumber, type ModelOption } from "./helpers";
import { ModelSelect } from "./ParamsPanel";
import type { RoutingInfo } from "./types";

const FORMATS = ["mp3", "wav", "opus", "flac"] as const;

export function SpeechPanel({
  modelOptions,
  model,
  onModelChange,
}: {
  modelOptions: ModelOption[];
  model: string;
  onModelChange: (m: string) => void;
}) {
  const { t } = useLanguage();
  const [text, setText] = useState("");
  const [voice, setVoice] = useState("alloy");
  const [format, setFormat] = useState<(typeof FORMATS)[number]>("mp3");
  const [speed, setSpeed] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [routing, setRouting] = useState<RoutingInfo | undefined>();
  const [audio, setAudio] = useState<{ url: string; ext: string } | null>(null);
  const urlRef = useRef<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  // object-URL und laufenden request beim unmount freigeben
  useEffect(
    () => () => {
      if (urlRef.current) URL.revokeObjectURL(urlRef.current);
      abortRef.current?.abort();
    },
    []
  );

  const generate = async () => {
    setError(null);
    if (!model) return setError(t("pg_no_model"));
    if (!text.trim()) return setError(t("pg_speech_empty"));
    const sp = parseOptionalNumber(speed);
    if (sp === null) return setError(t("pg_invalid_number", { field: t("pg_speed") }));
    const body: Record<string, unknown> = { model, input: text, voice: voice.trim() || "alloy", response_format: format };
    if (sp !== undefined) body.speed = sp;

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    setBusy(true);
    try {
      const res = await sendSpeech(body, { signal: controller.signal });
      const url = URL.createObjectURL(res.blob);
      if (urlRef.current) URL.revokeObjectURL(urlRef.current);
      urlRef.current = url;
      setAudio({ url, ext: audioExtension(res.contentType, format) });
      setRouting(res.routing);
    } catch (e) {
      if (controller.signal.aborted) return;
      if (e instanceof PlaygroundError) {
        setError(`${t("pg_error_status", { status: e.status })}: ${e.message}`);
        setRouting(e.routing);
      } else if (e instanceof NetworkError) {
        setError(t("pg_network_error", { msg: e.message }));
        setRouting(undefined);
      } else {
        setError(e instanceof Error ? e.message : String(e));
        setRouting(undefined);
      }
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null;
        setBusy(false);
      }
    }
  };

  return (
    <Card className="min-w-0 max-w-3xl">
      <CardContent className="space-y-4 p-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="min-w-0 space-y-1.5">
            <Label htmlFor="pg-sp-model">{t("pg_model")}</Label>
            <ModelSelect id="pg-sp-model" value={model} options={modelOptions} onChange={onModelChange} />
          </div>
          <div className="min-w-0 space-y-1.5">
            <Label htmlFor="pg-sp-voice">{t("pg_voice")}</Label>
            <Input id="pg-sp-voice" value={voice} onChange={(e) => setVoice(e.target.value)} />
          </div>
          <div className="min-w-0 space-y-1.5">
            <Label htmlFor="pg-sp-format">{t("pg_format")}</Label>
            <Select
              id="pg-sp-format"
              value={format}
              onChange={(e) => setFormat(e.target.value as (typeof FORMATS)[number])}
            >
              {FORMATS.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </Select>
          </div>
          <div className="min-w-0 space-y-1.5">
            <Label htmlFor="pg-sp-speed">{t("pg_speed")}</Label>
            <Input
              id="pg-sp-speed"
              inputMode="decimal"
              placeholder={t("pg_not_set")}
              value={speed}
              onChange={(e) => setSpeed(e.target.value)}
            />
          </div>
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="pg-sp-text">{t("pg_speech_text")}</Label>
          <textarea
            id="pg-sp-text"
            rows={5}
            className={TEXTAREA_CLASS}
            placeholder={t("pg_speech_placeholder")}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button onClick={() => void generate()} disabled={busy}>
            <Volume2 className="mr-1.5 h-4 w-4" />
            {busy ? t("pg_generating") : t("pg_generate")}
          </Button>
          {busy && (
            <Button variant="destructive" onClick={() => abortRef.current?.abort()}>
              <Square className="mr-1.5 h-4 w-4" />
              {t("pg_stop")}
            </Button>
          )}
        </div>
        {error && <p className="break-words text-sm text-destructive">{error}</p>}
        {audio && (
          <div className="flex flex-wrap items-center gap-3">
            <audio controls src={audio.url} className="w-full max-w-md" />
            <a
              href={audio.url}
              download={`speech.${audio.ext}`}
              className="inline-flex items-center text-sm underline-offset-2 hover:underline"
            >
              <Download className="mr-1 h-4 w-4" />
              {t("pg_download")}
            </a>
          </div>
        )}
        {routing && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
            <RoutingInfoView routing={routing} />
          </div>
        )}
      </CardContent>
    </Card>
  );
}
