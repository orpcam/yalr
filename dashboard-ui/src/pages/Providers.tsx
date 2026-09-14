import { useMemo } from "react";
import { useProvidersData } from "./providers/useProvidersData";
import { ProvidersSection } from "./providers/ProvidersSection";
import { ModelsSection } from "./providers/ModelsSection";
import { FallbacksSection } from "./providers/FallbacksSection";
import { RedirectsSection } from "./providers/RedirectsSection";
import { useLanguage } from "@/lib/i18n";

export default function Providers() {
  const { providers, models, fallbacks, redirects, load } = useProvidersData();
  const { t } = useLanguage();

  const modelNames = useMemo(
    () => [...new Set(models.map((m) => m.model_name))],
    [models]
  );

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">{t("nav_providers")}</h1>
      <ProvidersSection providers={providers} refresh={load} />
      <ModelsSection providers={providers} models={models} refresh={load} />
      <RedirectsSection redirects={redirects} modelNames={modelNames} refresh={load} />
      <FallbacksSection fallbacks={fallbacks} modelNames={modelNames} refresh={load} />
    </div>
  );
}
