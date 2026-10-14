import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label, Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { useLanguage } from "@/lib/i18n";
import { parseExtraBody } from "./buildRequest";
import { TEXTAREA_CLASS, type ModelOption, type ParamsState } from "./helpers";
import type { ReasoningEffort, ReasoningMode } from "./types";

export function ModelSelect({
  id,
  value,
  options,
  onChange,
}: {
  id: string;
  value: string;
  options: ModelOption[];
  onChange: (v: string) => void;
}) {
  const { t } = useLanguage();
  return (
    <Select id={id} value={value} onChange={(e) => onChange(e.target.value)} disabled={options.length === 0}>
      {options.length === 0 && <option value="">{t("pg_no_models")}</option>}
      {options.map((o) => (
        <option key={o.name} value={o.name}>
          {o.redirectTo ? `${o.name} ${t("pg_redirect_to", { target: o.redirectTo })}` : o.name}
        </option>
      ))}
    </Select>
  );
}

export function ParamsPanel({
  value,
  onChange,
  modelOptions,
  providerOptions,
}: {
  value: ParamsState;
  onChange: (patch: Partial<ParamsState>) => void;
  modelOptions: ModelOption[];
  providerOptions: string[];
}) {
  const { t } = useLanguage();
  const extraError = parseExtraBody(value.extraBody).error;

  return (
    <Card className="min-w-0">
      <CardHeader className="p-4">
        <CardTitle className="text-base">{t("pg_parameters")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4 p-4 pt-0">
        <div className="space-y-1.5">
          <Label htmlFor="pg-model">{t("pg_model")}</Label>
          <ModelSelect id="pg-model" value={value.model} options={modelOptions} onChange={(model) => onChange({ model })} />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="pg-provider">{t("pg_provider")}</Label>
          <Select id="pg-provider" value={value.provider} onChange={(e) => onChange({ provider: e.target.value })}>
            <option value="">{t("pg_provider_auto")}</option>
            {providerOptions.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </Select>
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="pg-system">{t("pg_system_prompt")}</Label>
          <textarea
            id="pg-system"
            rows={3}
            className={TEXTAREA_CLASS}
            value={value.systemPrompt}
            onChange={(e) => onChange({ systemPrompt: e.target.value })}
          />
        </div>
        <div className="grid grid-cols-3 gap-2">
          {(
            [
              ["temperature", "pg_temperature", "0.01"],
              ["maxTokens", "pg_max_tokens", "1"],
              ["topP", "pg_top_p", "0.01"],
            ] as const
          ).map(([field, labelKey]) => (
            <div key={field} className="min-w-0 space-y-1.5">
              <Label htmlFor={`pg-${field}`} className="block truncate">
                {t(labelKey)}
              </Label>
              <Input
                id={`pg-${field}`}
                inputMode="decimal"
                placeholder={t("pg_not_set")}
                value={value[field]}
                onChange={(e) => onChange({ [field]: e.target.value })}
              />
            </div>
          ))}
        </div>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            className="h-4 w-4"
            checked={value.stream}
            onChange={(e) => onChange({ stream: e.target.checked })}
          />
          {t("pg_stream")}
        </label>
        <div className="grid grid-cols-2 gap-2">
          <div className="min-w-0 space-y-1.5">
            <Label htmlFor="pg-reasoning">{t("pg_reasoning")}</Label>
            <Select
              id="pg-reasoning"
              value={value.reasoning}
              onChange={(e) => onChange({ reasoning: e.target.value as ReasoningMode })}
            >
              <option value="default">{t("pg_reasoning_default")}</option>
              <option value="on">{t("pg_reasoning_on")}</option>
              <option value="off">{t("pg_reasoning_off")}</option>
            </Select>
          </div>
          {value.reasoning === "on" && (
            <div className="min-w-0 space-y-1.5">
              <Label htmlFor="pg-effort">{t("pg_effort")}</Label>
              <Select
                id="pg-effort"
                value={value.effort}
                onChange={(e) => onChange({ effort: e.target.value as ReasoningEffort | "" })}
              >
                <option value="">{t("pg_effort_none")}</option>
                <option value="low">{t("pg_effort_low")}</option>
                <option value="medium">{t("pg_effort_medium")}</option>
                <option value="high">{t("pg_effort_high")}</option>
              </Select>
            </div>
          )}
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="pg-extra">{t("pg_extra_body")}</Label>
          <textarea
            id="pg-extra"
            rows={4}
            spellCheck={false}
            placeholder='{"seed": 42}'
            className={cn(TEXTAREA_CLASS, "font-mono text-xs", extraError && "border-destructive")}
            value={value.extraBody}
            onChange={(e) => onChange({ extraBody: e.target.value })}
            aria-invalid={extraError ? true : undefined}
          />
          {extraError ? (
            <p className="break-words text-xs text-destructive">{extraError}</p>
          ) : (
            <p className="text-xs text-muted-foreground">{t("pg_extra_body_hint")}</p>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
