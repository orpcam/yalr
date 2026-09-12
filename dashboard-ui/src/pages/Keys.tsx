import { useEffect, useRef, useState } from "react";
import { api, ApiError, VirtualKey } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/select";
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
import { useLanguage, useCurrency } from "@/lib/i18n";

// clipboard-api (nuerlich, secure context) zuerst, danach execCommand-fallback
async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall durch auf den fallback
  }
  return false;
}

async function fallbackCopy(text: string): Promise<boolean> {
  // aktuelle Auswahl + Fokus sichern, damit der execCommand-Hack das UI
  // des Nutzers nicht verletz
  const activeElement = document.activeElement as HTMLElement | null;
  const selection = document.getSelection();
  const savedRanges: Range[] = [];
  if (selection) {
    for (let i = 0; i < selection.rangeCount; i++) {
      savedRanges.push(selection.getRangeAt(i));
    }
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  let ok = false;
  try {
    textarea.focus();
    textarea.select();
    if (document.queryCommandSupported("copy")) {
      ok = document.execCommand("copy");
    }
  } catch {
    ok = false;
  } finally {
    textarea.remove();
    if (activeElement) activeElement.focus();
    const sel = document.getSelection();
    if (sel && savedRanges.length > 0) {
      sel.removeAllRanges();
      for (const r of savedRanges) sel.addRange(r);
    }
  }
  return ok;
}

export default function Keys() {
  const [keys, setKeys] = useState<VirtualKey[]>([]);
  const [name, setName] = useState("");
  const [budget, setBudget] = useState("");
  const [createdKey, setCreatedKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [revealError, setRevealError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const copyTimerRef = useRef<number | null>(null);
  const { t, formatDateTime } = useLanguage();
  const { formatCost } = useCurrency();
  const toast = useToast();
  const confirm = useConfirm();

  const load = () => {
    api
      .get<{ keys: VirtualKey[] }>("/keys")
      .then((r) => setKeys(r.keys))
      .catch((e) => setError(e instanceof Error ? e.message : t("load_failed")));
  };

  // eslint-disable-next-line react-hooks/exhaustive-deps -- einmalig beim mount laden; t ist nur fallback-text stabil
  useEffect(load, []);

  const copyKey = async (key: string, id: string) => {
    const ok =
      await copyToClipboard(key) || await fallbackCopy(key);
    if (ok) {
      if (copyTimerRef.current !== null) window.clearTimeout(copyTimerRef.current);
      setCopied(id);
      copyTimerRef.current = window.setTimeout(() => {
        setCopied((c) => (c === id ? null : c));
        copyTimerRef.current = null;
      }, 1500);
    } else {
      // beides fehlgeschlagen (z.B. non-secure context ohne execCommand)
      toast.error(t("copy_failed"));
    }
  };

  const reveal = async (k: VirtualKey) => {
    setRevealError(null);
    // reveal-toggle: "Copied!"-Feedback des betroffenen Keys zurücksetzen
    if (copyTimerRef.current !== null && copied === k.id) {
      window.clearTimeout(copyTimerRef.current);
      copyTimerRef.current = null;
    }
    setCopied((c) => (c === k.id ? null : c));
    if (revealed[k.id]) {
      setRevealed((prev) => {
        const next = { ...prev };
        delete next[k.id];
        return next;
      });
      return;
    }
    try {
      const res = await api.post<{ key: string }>(`/keys/${k.id}/reveal`, {
        key_hint: k.key_hint ?? "",
      });
      setRevealed((prev) => ({ ...prev, [k.id]: res.key }));
    } catch (err) {
      const msg = err instanceof Error ? err.message : "";
      if (err instanceof ApiError && err.status === 403) {
        setRevealError(t("reveal_legacy"));
      } else if (err instanceof ApiError && err.status === 429) {
        setRevealError(t("reveal_rate_limited"));
      } else if (msg.includes("not available") || msg.includes("decryption failed")) {
        setRevealError(t("reveal_unavailable"));
      } else {
        setRevealError(t("error_generic"));
      }
    }
  };

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    // budget validieren: NaN/negativ wuerde still zu null bzw. unsinnigem wert fuehren
    const budgetUsd = budget === "" ? null : parseFloat(budget);
    if (budgetUsd !== null && (!Number.isFinite(budgetUsd) || budgetUsd < 0)) {
      setError(t("budget_invalid"));
      return;
    }
    try {
      const res = await api.post<{ key: string }>("/keys", {
        name,
        budget_cents: budgetUsd !== null ? Math.round(budgetUsd * 100) : null,
      });
      setCreatedKey(res.key);
      setName("");
      setBudget("");
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const toggleEnabled = async (k: VirtualKey) => {
    try {
      await api.post(`/keys/${k.id}`, { enabled: !k.enabled });
      load();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const remove = async (k: VirtualKey) => {
    const ok = await confirm({
      title: t("confirm_delete_key", { name: k.name }),
      destructive: true,
      confirmLabel: t("delete"),
      cancelLabel: t("cancel"),
    });
    if (!ok) return;
    try {
      await api.del(`/keys/${k.id}`);
      load();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">{t("nav_keys")}</h1>

      {createdKey && (
        <Card className="border-emerald-500/30">
          <CardHeader>
            <CardTitle className="text-emerald-500">{t("new_key_created")}</CardTitle>
          </CardHeader>
          <CardContent className="flex items-center gap-4">
            <code className="rounded-md bg-muted px-3 py-2 text-sm">{createdKey}</code>
            <Button
              variant="outline"
              size="sm"
              onClick={() => copyKey(createdKey, "created")}
            >
              {copied === "created" ? t("copied") : t("copy")}
            </Button>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle>{t("create_new_key")}</CardTitle>
        </CardHeader>
        <CardContent>
          <form onSubmit={create} className="flex flex-wrap items-end gap-4">
            <div className="w-full space-y-2 sm:w-auto">
              <Label htmlFor="key-name">{t("name")}</Label>
              <Input
                id="key-name"
                className="w-full sm:w-64"
                placeholder={t("name_placeholder")}
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
              />
            </div>
            <div className="w-full space-y-2 sm:w-auto">
              <Label htmlFor="key-budget">{t("budget_usd_optional")}</Label>
              <Input
                id="key-budget"
                className="w-full sm:w-40"
                type="number"
                step="0.01"
                min="0"
                placeholder={t("unlimited")}
                value={budget}
                onChange={(e) => setBudget(e.target.value)}
              />
            </div>
            <Button type="submit">{t("create")}</Button>
          </form>
        </CardContent>
      </Card>

      {error && <p className="text-sm text-destructive">{error}</p>}
      {revealError && <p className="text-sm text-destructive">{revealError}</p>}

      <Card>
        <CardContent className="p-0">
          <Table className="min-w-[640px] md:min-w-0">
            <TableHeader>
              <TableRow>
                <TableHead className="pl-4">{t("name")}</TableHead>
                <TableHead>{t("key")}</TableHead>
                <TableHead>{t("budget")}</TableHead>
                <TableHead>{t("state")}</TableHead>
                <TableHead>{t("last_used")}</TableHead>
                <TableHead className="pr-4 text-right">{t("actions")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {keys.length === 0 && (
                <TableRow>
                  <TableCell colSpan={6} className="p-8 text-center text-muted-foreground">
                    {t("no_keys")}
                  </TableCell>
                </TableRow>
              )}
              {keys.map((k) => (
                <TableRow key={k.id}>
                  <TableCell className="pl-4 font-medium">{k.name}</TableCell>
                  <TableCell>
                    {revealed[k.id] ? (
                      <div className="flex flex-wrap items-center gap-2">
                        <code className="break-all rounded-md bg-muted px-2 py-1 font-mono text-xs">
                          {revealed[k.id]}
                        </code>
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => copyKey(revealed[k.id], k.id)}
                        >
                          {copied === k.id ? t("copied") : t("copy")}
                        </Button>
                        <Button variant="outline" size="sm" onClick={() => reveal(k)}>
                          {t("hide")}
                        </Button>
                      </div>
                    ) : (
                      <div className="flex items-center gap-2">
                        <code className="text-xs text-muted-foreground">{k.key_prefix}</code>
                        {k.key_hint ? (
                          <Button variant="outline" size="sm" onClick={() => reveal(k)}>
                            {t("show")}
                          </Button>
                        ) : (
                          <span
                            className="cursor-help text-xs text-muted-foreground"
                            title={t("reveal_legacy")}
                          >
                            🔒
                          </span>
                        )}
                      </div>
                    )}
                  </TableCell>
                  <TableCell>
                    {k.budget_cents !== null
                      ? formatCost(k.budget_cents / 100)
                      : t("unlimited")}
                  </TableCell>
                  <TableCell>
                    <Badge variant={k.enabled ? "success" : "error"}>
                      {k.enabled ? t("active") : t("disabled")}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {k.last_used_at ? formatDateTime(k.last_used_at) : t("never")}
                  </TableCell>
                  <TableCell className="pr-4 text-right">
                    <div className="flex justify-end gap-2">
                      <Button variant="outline" size="sm" onClick={() => toggleEnabled(k)}>
                        {k.enabled ? t("deactivate") : t("activate")}
                      </Button>
                      <Button variant="destructive" size="sm" onClick={() => remove(k)}>
                        {t("delete")}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
