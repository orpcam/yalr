import { useState } from "react";
import { api, Redirect } from "@/lib/api";
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
import { TriangleAlert } from "lucide-react";
import { useToast } from "@/components/ui/toast";
import { useConfirm } from "@/lib/confirm";
import { useLanguage } from "@/lib/i18n";

interface Props {
  redirects: Redirect[];
  modelNames: string[];
  refresh: () => void;
}

export function RedirectsSection({ redirects, modelNames, refresh }: Props) {
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [note, setNote] = useState("");
  const { t, formatDateTime } = useLanguage();
  const toast = useToast();
  const confirm = useConfirm();

  const createRedirect = async (e: React.FormEvent) => {
    e.preventDefault();
    if (from === to) {
      toast.error(t("redirect_source_equals_target"));
      return;
    }
    try {
      await api.post("/redirects", {
        model_name: from,
        redirect_model_name: to,
        note: note.trim() || undefined,
      });
      setFrom("");
      setTo("");
      setNote("");
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const removeRedirect = async (r: Redirect) => {
    const ok = await confirm({
      title: t("confirm_delete_redirect", {
        model: r.model_name,
        target: r.redirect_model_name,
      }),
      destructive: true,
      confirmLabel: t("delete"),
      cancelLabel: t("cancel"),
    });
    if (!ok) return;
    try {
      await api.del(`/redirects/${r.id}`);
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("redirects")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <p className="text-sm text-muted-foreground">{t("redirect_hint")}</p>
        <form onSubmit={createRedirect} className="flex flex-wrap items-center gap-2 sm:items-end sm:gap-4">
          <div className="w-full space-y-2 sm:w-auto">
            <Label>{t("redirect_from")}</Label>
            <Select className="w-full sm:w-48" value={from} onChange={(e) => setFrom(e.target.value)} required>
              <option value="">{t("select_placeholder")}</option>
              {modelNames.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </Select>
          </div>
          <div className="w-full space-y-2 sm:w-auto">
            <Label>{t("redirect_to")}</Label>
            <Select className="w-full sm:w-48" value={to} onChange={(e) => setTo(e.target.value)} required>
              <option value="">{t("select_placeholder")}</option>
              {modelNames
                .filter((n) => n !== from)
                .map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
            </Select>
          </div>
          <div className="w-full space-y-2 sm:w-auto">
            <Label>{t("redirect_note")}</Label>
            <Input
              className="w-full sm:w-56"
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder={t("redirect_note")}
            />
          </div>
          <Button type="submit">{t("add_redirect")}</Button>
        </form>

        <Table className="min-w-[640px]">
          <TableHeader>
            <TableRow>
              <TableHead>{t("model")}</TableHead>
              <TableHead>→ {t("redirect_to")}</TableHead>
              <TableHead>{t("redirect_note")}</TableHead>
              <TableHead>{t("active_since_header")}</TableHead>
              <TableHead className="text-right">{t("actions")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {redirects.length === 0 && (
              <TableRow>
                <TableCell colSpan={5} className="p-6 text-center text-muted-foreground">
                  {t("no_redirects")}
                </TableCell>
              </TableRow>
            )}
            {redirects.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="font-medium">{r.model_name}</TableCell>
                <TableCell className="whitespace-nowrap">
                  {r.redirect_model_name}
                  {!r.target_has_route && (
                    <span title={t("redirect_target_no_route")} className="ml-1.5 inline-flex align-middle">
                      <TriangleAlert className="h-3.5 w-3.5 text-amber-500" />
                    </span>
                  )}
                </TableCell>
                <TableCell className="text-muted-foreground">{r.note || "–"}</TableCell>
                <TableCell className="whitespace-nowrap">
                  {t("active_since", { date: formatDateTime(r.created_at) })}
                </TableCell>
                <TableCell className="text-right whitespace-nowrap">
                  <Button variant="destructive" size="sm" onClick={() => removeRedirect(r)}>
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
