import { useCallback, useEffect, useState } from "react";
import { api, Provider, Model, Fallback } from "@/lib/api";
import { useToast } from "@/components/ui/toast";
import { useLanguage } from "@/lib/i18n";

/**
 * Geteilte daten-quelle der providers-seite: laedt providers, models und
 * fallbacks und stellt ein reload bereit. Jede sektion (provider/modelle/
 * fallbacks) nutzt dieselben daten, hat aber eigene form-/fehler-zustaende.
 */
export function useProvidersData() {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [models, setModels] = useState<Model[]>([]);
  const [fallbacks, setFallbacks] = useState<Fallback[]>([]);
  const toast = useToast();
  const { t } = useLanguage();

  const load = useCallback(() => {
    api
      .get<{ providers: Provider[] }>("/providers")
      .then((r) => setProviders(r.providers))
      .catch((e) => toast.error(e instanceof Error ? e.message : t("load_failed")));
    api
      .get<{ models: Model[] }>("/models")
      .then((r) => setModels(r.models))
      .catch((e) => toast.error(e instanceof Error ? e.message : t("load_failed")));
    api
      .get<{ fallbacks: Fallback[] }>("/fallbacks")
      .then((r) => setFallbacks(r.fallbacks))
      .catch((e) => toast.error(e instanceof Error ? e.message : t("load_failed")));
  }, [toast, t]);

  useEffect(() => {
    load();
  }, [load]);

  return { providers, models, fallbacks, load };
}
