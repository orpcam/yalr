import { useState } from "react";
import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label, Select } from "@/components/ui/select";
import { useLanguage, useCurrency, LANGUAGES, CURRENCIES } from "@/lib/i18n";

export default function Settings() {
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { t, language, setLanguage } = useLanguage();
  const { currency, setCurrency } = useCurrency();

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setMessage(null);
    setError(null);
    try {
      await api.post("/settings/password", {
        current_password: currentPassword,
        new_password: newPassword,
      });
      setMessage(t("password_changed"));
      setCurrentPassword("");
      setNewPassword("");
    } catch (err) {
      setError(err instanceof Error ? err.message : t("error_generic"));
    }
  };

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">{t("nav_settings")}</h1>

      <Card className="max-w-md">
        <CardHeader>
          <CardTitle>{t("language")}</CardTitle>
        </CardHeader>
        <CardContent>
          <Select
            className="w-full sm:w-44"
            value={language}
            onChange={(e) => setLanguage(e.target.value as typeof language)}
          >
            {LANGUAGES.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </Select>
        </CardContent>
      </Card>

      <Card className="max-w-md">
        <CardHeader>
          <CardTitle>{t("currency")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-sm text-muted-foreground">{t("currency_description")}</p>
          <Select
            className="w-full sm:w-44"
            value={currency}
            onChange={(e) => setCurrency(e.target.value as typeof currency)}
          >
            {CURRENCIES.map((c) => (
              <option key={c.value} value={c.value}>
                {t(c.value === "USD" ? "currency_usd" : "currency_eur")}
              </option>
            ))}
          </Select>
        </CardContent>
      </Card>

      <Card className="max-w-md">
        <CardHeader>
          <CardTitle>{t("change_admin_password")}</CardTitle>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="current">{t("current_password")}</Label>
              <Input
                id="current"
                type="password"
                value={currentPassword}
                onChange={(e) => setCurrentPassword(e.target.value)}
                required
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="new">{t("new_password")}</Label>
              <Input
                id="new"
                type="password"
                value={newPassword}
                onChange={(e) => setNewPassword(e.target.value)}
                minLength={8}
                required
              />
            </div>
            {message && <p className="text-sm text-emerald-500">{message}</p>}
            {error && <p className="text-sm text-destructive">{error}</p>}
            <Button type="submit">{t("save")}</Button>
          </form>
        </CardContent>
      </Card>

      <Card className="max-w-md">
        <CardHeader>
          <CardTitle>{t("using_gateway")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2 text-sm text-muted-foreground">
          <p>{t("gateway_compat")}</p>
          <pre className="rounded-md bg-muted p-4 text-xs">
{`curl http://localhost:8080/v1/chat/completions \\
  -H "Authorization: Bearer sk-llm-..." \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hi"}],
    "stream": true
  }'`}
          </pre>
          <p>
            {t("gateway_extra", {
              messages: "/v1/messages",
              embeddings: "/v1/embeddings",
              models: "/v1/models",
            })}
          </p>
          <p>
            {t("gateway_override")} <code>x-llm-provider: anthropic</code>
          </p>
        </CardContent>
      </Card>
    </div>
  );
}
