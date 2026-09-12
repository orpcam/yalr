import { useState } from "react";
import { api, Provider, DiscoverMetricsResponse, RefreshCapsResponse } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label, Select } from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useToast } from "@/components/ui/toast";
import { useConfirm } from "@/lib/confirm";
import { useLanguage } from "@/lib/i18n";

const DEFAULT_BASE_URLS: Record<string, string> = {
  openai: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com",
  gemini: "https://generativelanguage.googleapis.com/v1beta",
  openai_compat: "",
};

const KINDS = ["openai", "anthropic", "gemini", "openai_compat"] as const;

interface Props {
  providers: Provider[];
  refresh: () => void;
}

export function ProvidersSection({ providers, refresh }: Props) {
  const [pName, setPName] = useState("");
  const [pKind, setPKind] = useState("openai");
  const [pBaseUrl, setPBaseUrl] = useState(DEFAULT_BASE_URLS.openai);
  const [pApiKey, setPApiKey] = useState("");
  const [pMetricsUrl, setPMetricsUrl] = useState("");

  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);
  const [editProv, setEditProv] = useState({
    name: "",
    kind: "openai",
    base_url: "",
    api_key: "",
    metrics_url: "",
    enabled: true,
  });

  const [refreshingId, setRefreshingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { t } = useLanguage();
  const toast = useToast();
  const confirm = useConfirm();

  const createProvider = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      await api.post("/providers", {
        name: pName,
        kind: pKind,
        base_url: pBaseUrl,
        api_key: pApiKey,
        metrics_url: pMetricsUrl || null,
      });
      setPName("");
      setPApiKey("");
      setPMetricsUrl("");
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const startEditProvider = (p: Provider) => {
    setEditingProviderId(p.id);
    setEditProv({
      name: p.name,
      kind: p.kind,
      base_url: p.base_url,
      api_key: "",
      metrics_url: p.metrics_url ?? "",
      enabled: p.enabled,
    });
  };

  const saveProvider = async (id: string) => {
    if (!editProv.name.trim() || !editProv.base_url.trim()) {
      setError(t("error_generic"));
      return;
    }
    setError(null);
    try {
      await api.post(`/providers/${id}`, {
        name: editProv.name.trim(),
        kind: editProv.kind,
        base_url: editProv.base_url.trim(),
        api_key: editProv.api_key.trim() || null,
        metrics_url: editProv.metrics_url.trim(),
        enabled: editProv.enabled,
      });
      setEditingProviderId(null);
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const discoverMetrics = async () => {
    if (!pBaseUrl.trim()) return;
    setError(null);
    try {
      const r = await api.post<DiscoverMetricsResponse>("/providers/discover-metrics", {
        kind: pKind,
        base_url: pBaseUrl,
        api_key: pApiKey,
      });
      if (r.metrics_url) {
        setPMetricsUrl(r.metrics_url);
      } else {
        setPMetricsUrl("");
        toast.error(t("metrics_not_found"));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const refreshCapabilities = async (id: string) => {
    setRefreshingId(id);
    setError(null);
    try {
      const r = await api.post<RefreshCapsResponse>(`/providers/${id}/refresh-capabilities`);
      if (r.metrics_url_discovered) {
        toast.success(t("metrics_discovered", { url: r.metrics_url_discovered }));
      }
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    } finally {
      setRefreshingId(null);
    }
  };

  const removeProvider = async (p: Provider) => {
    const ok = await confirm({
      title: t("confirm_delete_provider", { name: p.name }),
      destructive: true,
      confirmLabel: t("delete"),
      cancelLabel: t("cancel"),
    });
    if (!ok) return;
    try {
      await api.del(`/providers/${p.id}`);
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("provider")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <form onSubmit={createProvider} className="flex flex-wrap items-end gap-4">
          <div className="space-y-2">
            <Label>{t("name")}</Label>
            <Input className="w-44" placeholder="openai-main" value={pName} onChange={(e) => setPName(e.target.value)} required />
          </div>
          <div className="space-y-2">
            <Label>{t("type")}</Label>
            <Select
              className="w-40"
              value={pKind}
              onChange={(e) => {
                setPKind(e.target.value);
                setPBaseUrl(DEFAULT_BASE_URLS[e.target.value] ?? "");
              }}
            >
              {KINDS.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </Select>
          </div>
          <div className="space-y-2">
            <Label>{t("base_url")}</Label>
            <Input className="w-72" value={pBaseUrl} onChange={(e) => setPBaseUrl(e.target.value)} required />
          </div>
          <div className="space-y-2">
            <Label>{t("api_key")}</Label>
            <Input className="w-56" type="password" value={pApiKey} onChange={(e) => setPApiKey(e.target.value)} required />
          </div>
          <div className="space-y-2">
            <Label>{t("metrics_url")}</Label>
            <div className="flex items-center gap-2">
              <Input className="w-64" type="url" placeholder="http://host:port/metrics" value={pMetricsUrl} onChange={(e) => setPMetricsUrl(e.target.value)} />
              <Button type="button" variant="outline" size="sm" onClick={discoverMetrics} title={t("discover_metrics_hint")}>
                {t("discover_metrics")}
              </Button>
            </div>
          </div>
          <Button type="submit">{t("add")}</Button>
        </form>

        {error && <p className="text-sm text-destructive">{error}</p>}

        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t("name")}</TableHead>
              <TableHead>{t("type")}</TableHead>
              <TableHead>{t("base_url")}</TableHead>
              <TableHead>{t("metrics_url")}</TableHead>
              <TableHead className="text-right">{t("actions")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {providers.length === 0 && (
              <TableRow>
                <TableCell colSpan={5} className="p-6 text-center text-muted-foreground">
                  {t("no_providers")}
                </TableCell>
              </TableRow>
            )}
            {providers.map((p) =>
              editingProviderId === p.id ? (
                <TableRow key={p.id}>
                  <TableCell colSpan={4} className="align-top">
                    <div className="space-y-2">
                      <div className="flex flex-wrap items-center gap-2">
                        <Input
                          className="h-7 w-44 px-2 py-0.5 text-xs"
                          value={editProv.name}
                          onChange={(e) => setEditProv((s) => ({ ...s, name: e.target.value }))}
                        />
                        <Select
                          className="h-7 w-32 px-2 py-0.5 text-xs"
                          value={editProv.kind}
                          onChange={(e) => setEditProv((s) => ({ ...s, kind: e.target.value }))}
                        >
                          {KINDS.map((k) => (
                            <option key={k} value={k}>
                              {k}
                            </option>
                          ))}
                        </Select>
                      </div>
                      <Input
                        className="h-7 w-full px-2 py-0.5 text-xs"
                        value={editProv.base_url}
                        onChange={(e) => setEditProv((s) => ({ ...s, base_url: e.target.value }))}
                      />
                      <Input
                        className="h-7 w-full px-2 py-0.5 text-xs"
                        placeholder="http://host:port/metrics"
                        value={editProv.metrics_url}
                        onChange={(e) => setEditProv((s) => ({ ...s, metrics_url: e.target.value }))}
                      />
                      <Input
                        className="h-7 w-64 px-2 py-0.5 text-xs"
                        type="password"
                        placeholder={t("api_key_unchanged")}
                        value={editProv.api_key}
                        onChange={(e) => setEditProv((s) => ({ ...s, api_key: e.target.value }))}
                      />
                      <label className="flex items-center gap-1 text-xs">
                        <input
                          type="checkbox"
                          checked={editProv.enabled}
                          onChange={(e) => setEditProv((s) => ({ ...s, enabled: e.target.checked }))}
                        />
                        {t("active")}
                      </label>
                    </div>
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex justify-end gap-2">
                      <Button variant="outline" size="sm" onClick={() => setEditingProviderId(null)}>
                        {t("cancel")}
                      </Button>
                      <Button size="sm" onClick={() => saveProvider(p.id)}>
                        {t("save")}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ) : (
                <TableRow key={p.id}>
                  <TableCell className="font-medium">{p.name}</TableCell>
                  <TableCell>
                    <Badge variant="outline">{p.kind}</Badge>
                    {!p.enabled && (
                      <Badge variant="error" className="ml-1">{t("disabled")}</Badge>
                    )}
                  </TableCell>
                  <TableCell className="text-muted-foreground">{p.base_url}</TableCell>
                  <TableCell className="text-muted-foreground">{p.metrics_url || "—"}</TableCell>
                  <TableCell className="text-right">
                    <div className="flex justify-end gap-2">
                      <Button variant="outline" size="sm" onClick={() => startEditProvider(p)}>
                        {t("edit")}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={refreshingId !== null}
                        onClick={() => refreshCapabilities(p.id)}
                      >
                        {refreshingId === p.id ? t("sync_caps_busy") : t("sync_caps")}
                      </Button>
                      <Button variant="destructive" size="sm" onClick={() => removeProvider(p)}>
                        {t("delete")}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              )
            )}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}
