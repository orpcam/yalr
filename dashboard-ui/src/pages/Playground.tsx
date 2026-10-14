import { useEffect, useMemo, useState } from "react";
import { api, type Model, type Redirect } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { useLanguage, type TranslationKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { ChatPanel, type Exchange } from "./playground/ChatPanel";
import { ParamsPanel } from "./playground/ParamsPanel";
import { RawView } from "./playground/RawView";
import { SpeechPanel } from "./playground/SpeechPanel";
import {
  DEFAULT_PARAMS,
  applyParamsPatch,
  buildModelOptions,
  providersForSelection,
  type ParamsState,
} from "./playground/helpers";

type Tab = "chat" | "speech" | "raw";

const TABS: { id: Tab; labelKey: TranslationKey }[] = [
  { id: "chat", labelKey: "pg_tab_chat" },
  { id: "speech", labelKey: "pg_tab_speech" },
  { id: "raw", labelKey: "pg_tab_raw" },
];

export default function Playground() {
  const { t } = useLanguage();
  const toast = useToast();
  const [tab, setTab] = useState<Tab>("chat");
  const [models, setModels] = useState<Model[]>([]);
  const [redirects, setRedirects] = useState<Redirect[]>([]);
  const [params, setParams] = useState<ParamsState>(DEFAULT_PARAMS);
  const [speechModel, setSpeechModel] = useState("");
  const [exchange, setExchange] = useState<Exchange | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.get<{ models: Model[] }>("/models").then((r) => r.models),
      api.get<Redirect[]>("/redirects").catch(() => [] as Redirect[]),
    ])
      .then(([ms, rs]) => {
        if (cancelled) return;
        setModels(ms);
        setRedirects(rs);
        // erstes modell vorauswaehlen
        const first = buildModelOptions(ms, rs)[0]?.name ?? "";
        setParams((p) => (p.model ? p : { ...p, model: first }));
        setSpeechModel((m) => m || first);
      })
      .catch((e) => {
        if (!cancelled) toast.error(e instanceof Error ? e.message : t("load_failed"));
      });
    return () => {
      cancelled = true;
    };
    // nur einmal beim mount laden
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const modelOptions = useMemo(() => buildModelOptions(models, redirects), [models, redirects]);
  const providerOptions = useMemo(
    () => providersForSelection(models, redirects, params.model),
    [models, redirects, params.model]
  );

  const updateParams = (patch: Partial<ParamsState>) =>
    setParams((p) => applyParamsPatch(p, patch, models, redirects));

  return (
    <div className="min-w-0 space-y-4">
      <h1 className="text-2xl font-bold">{t("nav_playground")}</h1>

      <div role="tablist" className="flex gap-1 overflow-x-auto border-b">
        {TABS.map((tb) => (
          <button
            key={tb.id}
            type="button"
            role="tab"
            aria-selected={tab === tb.id}
            onClick={() => setTab(tb.id)}
            className={cn(
              "-mb-px whitespace-nowrap border-b-2 px-3 py-2 text-sm transition-colors",
              tab === tb.id
                ? "border-primary font-medium text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground"
            )}
          >
            {t(tb.labelKey)}
          </button>
        ))}
      </div>

      {/* alle tabs bleiben gemountet, damit verlauf und laufende streams erhalten bleiben */}
      <div className={cn("grid min-w-0 gap-4 lg:grid-cols-[320px_minmax(0,1fr)]", tab !== "chat" && "hidden")}>
        <ParamsPanel
          value={params}
          onChange={updateParams}
          modelOptions={modelOptions}
          providerOptions={providerOptions}
        />
        <ChatPanel params={params} onExchange={setExchange} />
      </div>
      <div className={cn(tab !== "speech" && "hidden")}>
        <SpeechPanel modelOptions={modelOptions} model={speechModel} onModelChange={setSpeechModel} />
      </div>
      <div className={cn(tab !== "raw" && "hidden")}>
        <RawView exchange={exchange} />
      </div>
    </div>
  );
}
