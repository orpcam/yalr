import { useState } from "react";
import { api, Provider, Model } from "@/lib/api";
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
import { parseCapabilities } from "./capabilities";

interface Props {
  providers: Provider[];
  models: Model[];
  refresh: () => void;
}

export function ModelsSection({ providers, models, refresh }: Props) {
  const [mProviderId, setMProviderId] = useState("");
  const [mName, setMName] = useState("");
  const [mUpstream, setMUpstream] = useState("");
  const [mInputPrice, setMInputPrice] = useState("");
  const [mOutputPrice, setMOutputPrice] = useState("");
  const [mQuantization, setMQuantization] = useState("");
  const [mNotes, setMNotes] = useState("");
  const [mLink, setMLink] = useState("");
  const [mCapabilities, setMCapabilities] = useState("");

  const [editingModelId, setEditingModelId] = useState<string | null>(null);
  const [editModel, setEditModel] = useState({
    provider_id: "",
    upstream_model: "",
    input_price: "",
    output_price: "",
    quantization: "",
    notes: "",
    link: "",
    capabilities: "",
  });

  const [error, setError] = useState<string | null>(null);
  const { t } = useLanguage();
  const toast = useToast();
  const confirm = useConfirm();

  const createModel = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    const caps = parseCapabilities(mCapabilities);
    if (caps === "invalid") {
      setError(t("capabilities_invalid"));
      return;
    }
    try {
      await api.post("/models", {
        provider_id: mProviderId,
        model_name: mName,
        upstream_model: mUpstream || mName,
        input_price_per_million: mInputPrice ? parseFloat(mInputPrice) : 0,
        output_price_per_million: mOutputPrice ? parseFloat(mOutputPrice) : 0,
        quantization: mQuantization.trim() || null,
        notes: mNotes.trim() || null,
        link: mLink.trim() || null,
        capabilities: caps,
      });
      setMName("");
      setMUpstream("");
      setMInputPrice("");
      setMOutputPrice("");
      setMQuantization("");
      setMNotes("");
      setMLink("");
      setMCapabilities("");
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const startEditModel = (m: Model) => {
    setEditingModelId(m.id);
    setEditModel({
      provider_id: m.provider_id,
      upstream_model: m.upstream_model,
      input_price: String(m.input_price_per_million),
      output_price: String(m.output_price_per_million),
      quantization: m.quantization ?? "",
      notes: m.notes ?? "",
      link: m.link ?? "",
      capabilities: m.capabilities ? JSON.stringify(m.capabilities, null, 2) : "",
    });
  };

  const saveModel = async (id: string) => {
    setError(null);
    const caps = parseCapabilities(editModel.capabilities);
    if (caps === "invalid") {
      setError(t("capabilities_invalid"));
      return;
    }
    try {
      await api.put(`/models/${id}`, {
        provider_id: editModel.provider_id || null,
        upstream_model: editModel.upstream_model.trim() || null,
        input_price_per_million: editModel.input_price ? parseFloat(editModel.input_price) : null,
        output_price_per_million: editModel.output_price ? parseFloat(editModel.output_price) : null,
        quantization: editModel.quantization.trim() || null,
        notes: editModel.notes.trim() || null,
        link: editModel.link.trim() || null,
        capabilities: caps,
      });
      setEditingModelId(null);
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  const removeModel = async (m: Model) => {
    const ok = await confirm({
      title: t("confirm_delete_model", { name: m.model_name }),
      destructive: true,
      confirmLabel: t("delete"),
      cancelLabel: t("cancel"),
    });
    if (!ok) return;
    try {
      await api.del(`/models/${m.id}`);
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("model")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <form onSubmit={createModel} className="flex flex-wrap items-end gap-4">
          <div className="space-y-2">
            <Label>{t("provider")}</Label>
            <Select className="w-44" value={mProviderId} onChange={(e) => setMProviderId(e.target.value)} required>
              <option value="">{t("select_placeholder")}</option>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </Select>
          </div>
          <div className="space-y-2">
            <Label>{t("model_name_gateway")}</Label>
            <Input className="w-44" placeholder="gpt-4o" value={mName} onChange={(e) => setMName(e.target.value)} required />
          </div>
          <div className="space-y-2">
            <Label>{t("upstream_model")}</Label>
            <Input className="w-52" placeholder={t("upstream_placeholder")} value={mUpstream} onChange={(e) => setMUpstream(e.target.value)} />
          </div>
          <div className="space-y-2">
            <Label>{t("input_price")}</Label>
            <Input className="w-28" type="number" step="0.0001" min="0" value={mInputPrice} onChange={(e) => setMInputPrice(e.target.value)} />
          </div>
          <div className="space-y-2">
            <Label>{t("output_price")}</Label>
            <Input className="w-28" type="number" step="0.0001" min="0" value={mOutputPrice} onChange={(e) => setMOutputPrice(e.target.value)} />
          </div>
          <div className="space-y-2">
            <Label>{t("quantization")}</Label>
            <Input className="w-28" placeholder="FP8" value={mQuantization} onChange={(e) => setMQuantization(e.target.value)} />
          </div>
          <div className="space-y-2 grow">
            <Label>{t("notes")}</Label>
            <Input placeholder={t("notes_placeholder")} value={mNotes} onChange={(e) => setMNotes(e.target.value)} />
          </div>
          <div className="space-y-2 grow">
            <Label>{t("link")}</Label>
            <Input placeholder={t("link_placeholder")} value={mLink} onChange={(e) => setMLink(e.target.value)} />
          </div>
          <div className="space-y-2 grow">
            <Label>{t("capabilities")}</Label>
            <textarea
              className="w-full rounded-md border border-border bg-transparent p-2 font-mono text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              rows={3}
              placeholder={t("capabilities_placeholder")}
              value={mCapabilities}
              onChange={(e) => setMCapabilities(e.target.value)}
            />
          </div>
          <Button type="submit">{t("add")}</Button>
        </form>

        {error && <p className="text-sm text-destructive">{error}</p>}

        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t("model")}</TableHead>
              <TableHead>{t("upstream_model")}</TableHead>
              <TableHead>{t("provider")}</TableHead>
              <TableHead>{t("quantization")}</TableHead>
              <TableHead className="text-right">{t("input_price")}</TableHead>
              <TableHead className="text-right">{t("output_price")}</TableHead>
              <TableHead className="text-right">{t("actions")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {models.length === 0 && (
              <TableRow>
                <TableCell colSpan={7} className="p-6 text-center text-muted-foreground">
                  {t("no_models")}
                </TableCell>
              </TableRow>
            )}
            {models.map((m) =>
              editingModelId === m.id ? (
                <TableRow key={m.id}>
                  <TableCell className="font-medium">
                    {m.model_name}
                    <Input
                      className="mt-1 h-7 w-44 px-2 py-0.5 text-xs"
                      value={editModel.notes}
                      placeholder={t("notes_placeholder")}
                      onChange={(e) => setEditModel((s) => ({ ...s, notes: e.target.value }))}
                    />
                    <Input
                      className="mt-1 h-7 w-44 px-2 py-0.5 text-xs"
                      value={editModel.link}
                      placeholder={t("link_placeholder")}
                      onChange={(e) => setEditModel((s) => ({ ...s, link: e.target.value }))}
                    />
                    <textarea
                      className="mt-1 w-44 rounded-md border border-border bg-transparent p-1 font-mono text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                      rows={3}
                      placeholder={t("capabilities_placeholder")}
                      value={editModel.capabilities}
                      onChange={(e) => setEditModel((s) => ({ ...s, capabilities: e.target.value }))}
                    />
                  </TableCell>
                  <TableCell>
                    <Input
                      className="h-7 w-40 px-2 py-0.5 text-xs"
                      value={editModel.upstream_model}
                      onChange={(e) => setEditModel((s) => ({ ...s, upstream_model: e.target.value }))}
                    />
                  </TableCell>
                  <TableCell>
                    <Select
                      className="h-7 w-36 px-2 py-0.5 text-xs"
                      value={editModel.provider_id}
                      onChange={(e) => setEditModel((s) => ({ ...s, provider_id: e.target.value }))}
                    >
                      {providers.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.name}
                        </option>
                      ))}
                    </Select>
                  </TableCell>
                  <TableCell>
                    <Input
                      className="h-7 w-20 px-2 py-0.5 text-xs"
                      value={editModel.quantization}
                      onChange={(e) => setEditModel((s) => ({ ...s, quantization: e.target.value }))}
                    />
                  </TableCell>
                  <TableCell className="text-right">
                    <Input
                      className="h-7 w-20 px-2 py-0.5 text-right text-xs"
                      type="number"
                      step="0.0001"
                      min="0"
                      value={editModel.input_price}
                      onChange={(e) => setEditModel((s) => ({ ...s, input_price: e.target.value }))}
                    />
                  </TableCell>
                  <TableCell className="text-right">
                    <Input
                      className="h-7 w-20 px-2 py-0.5 text-right text-xs"
                      type="number"
                      step="0.0001"
                      min="0"
                      value={editModel.output_price}
                      onChange={(e) => setEditModel((s) => ({ ...s, output_price: e.target.value }))}
                    />
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex justify-end gap-2">
                      <Button variant="outline" size="sm" onClick={() => setEditingModelId(null)}>
                        {t("cancel")}
                      </Button>
                      <Button size="sm" onClick={() => saveModel(m.id)}>
                        {t("save")}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ) : (
                <TableRow key={m.id}>
                  <TableCell className="font-medium">
                    {m.model_name}
                    {m.notes && (
                      <span className="block max-w-xs truncate text-xs font-normal text-muted-foreground" title={m.notes}>
                        {m.notes}
                      </span>
                    )}
                    {m.link && (
                      <a
                        href={m.link}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="block max-w-xs truncate text-xs font-normal text-blue-500 hover:underline"
                        title={m.link}
                      >
                        {m.link}
                      </a>
                    )}
                    {m.capabilities && (
                      <span
                        className="block max-w-xs truncate font-mono text-xs font-normal text-muted-foreground"
                        title={JSON.stringify(m.capabilities)}
                      >
                        {Object.entries(m.capabilities)
                          .map(([k, v]) => (typeof v === "boolean" ? (v ? k : `${k}: false`) : `${k}: ${JSON.stringify(v)}`))
                          .join(" · ")}
                      </span>
                    )}
                  </TableCell>
                  <TableCell className="text-muted-foreground">{m.upstream_model}</TableCell>
                  <TableCell>{m.provider_name}</TableCell>
                  <TableCell>{m.quantization || "–"}</TableCell>
                  <TableCell className="text-right">${m.input_price_per_million}</TableCell>
                  <TableCell className="text-right">${m.output_price_per_million}</TableCell>
                  <TableCell className="text-right">
                    <div className="flex justify-end gap-2">
                      <Button variant="outline" size="sm" onClick={() => startEditModel(m)}>
                        {t("edit")}
                      </Button>
                      <Button variant="destructive" size="sm" onClick={() => removeModel(m)}>
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
