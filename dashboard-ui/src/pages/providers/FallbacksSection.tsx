import { useState } from "react";
import { api, Fallback } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
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

interface Props {
  fallbacks: Fallback[];
  modelNames: string[];
  refresh: () => void;
}

export function FallbacksSection({ fallbacks, modelNames, refresh }: Props) {
  const [fModel, setFModel] = useState("");
  const [fFallback, setFFallback] = useState("");
  const [error, setError] = useState<string | null>(null);
  const { t } = useLanguage();
  const toast = useToast();
  const confirm = useConfirm();

  const createFallback = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      await api.post("/fallbacks", {
        model_name: fModel,
        fallback_model_name: fFallback,
      });
      setFModel("");
      setFFallback("");
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const removeFallback = async (f: Fallback) => {
    const ok = await confirm({
      title: t("confirm_delete_fallback", {
        model: f.model_name,
        fallback: f.fallback_model_name,
      }),
      destructive: true,
      confirmLabel: t("delete"),
      cancelLabel: t("cancel"),
    });
    if (!ok) return;
    try {
      await api.del(`/fallbacks/${f.id}`);
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("fallback_chains")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <p className="text-sm text-muted-foreground">{t("fallback_hint")}</p>
        <form onSubmit={createFallback} className="flex flex-wrap items-end gap-4">
          <div className="space-y-2">
            <Label>{t("model")}</Label>
            <Select className="w-48" value={fModel} onChange={(e) => setFModel(e.target.value)} required>
              <option value="">{t("select_placeholder")}</option>
              {modelNames.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </Select>
          </div>
          <div className="space-y-2">
            <Label>{t("fallback_to")}</Label>
            <Select className="w-48" value={fFallback} onChange={(e) => setFFallback(e.target.value)} required>
              <option value="">{t("select_placeholder")}</option>
              {modelNames
                .filter((n) => n !== fModel)
                .map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
            </Select>
          </div>
          <Button type="submit">{t("add")}</Button>
        </form>

        {error && <p className="text-sm text-destructive">{error}</p>}

        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t("model")}</TableHead>
              <TableHead>→ {t("fallback_to")}</TableHead>
              <TableHead className="text-right">{t("actions")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {fallbacks.length === 0 && (
              <TableRow>
                <TableCell colSpan={3} className="p-6 text-center text-muted-foreground">
                  {t("no_fallbacks")}
                </TableCell>
              </TableRow>
            )}
            {fallbacks.map((f) => (
              <TableRow key={f.id}>
                <TableCell className="font-medium">{f.model_name}</TableCell>
                <TableCell>{f.fallback_model_name}</TableCell>
                <TableCell className="text-right">
                  <Button variant="destructive" size="sm" onClick={() => removeFallback(f)}>
                    {t("delete")}
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}
