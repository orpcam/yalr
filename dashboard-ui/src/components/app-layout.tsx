import { useEffect, useState } from "react";
import { Outlet, NavLink, useNavigate } from "react-router-dom";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { ThemeToggle } from "@/components/ThemeToggle";
import { Network } from "lucide-react";
import { useLanguage, TranslationKey } from "@/lib/i18n";

const navItems: { to: string; labelKey: TranslationKey }[] = [
  { to: "/", labelKey: "nav_overview" },
  { to: "/requests", labelKey: "nav_requests" },
  { to: "/keys", labelKey: "nav_keys" },
  { to: "/providers", labelKey: "nav_providers" },
  { to: "/settings", labelKey: "nav_settings" },
];

export function AppLayout() {
  const [authed, setAuthed] = useState<boolean | null>(null);
  const navigate = useNavigate();
  const { t } = useLanguage();

  useEffect(() => {
    api
      .get("/auth/me")
      .then(() => setAuthed(true))
      .catch(() => setAuthed(false));
  }, []);

  // navigiere als side-effect, nicht waehrend des renderns
  useEffect(() => {
    if (authed === false) navigate("/login", { replace: true });
  }, [authed, navigate]);

  if (authed !== true) {
    return (
      <div className="flex h-screen items-center justify-center">{t("loading")}</div>
    );
  }

  const logout = async () => {
    await api.post("/auth/logout");
    window.location.href = "/login";
  };

  return (
    <div className="flex min-h-screen">
      <aside className="flex w-56 flex-col border-r bg-card">
        <div className="flex items-center gap-2 border-b p-4">
          <Network className="h-6 w-6 shrink-0 text-emerald-500" />
          <span className="text-lg font-bold">YALR</span>
        </div>
        <nav className="flex-1 space-y-1 p-2">
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === "/"}
              className={({ isActive }) =>
                cn(
                  "block rounded-md px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground",
                  isActive && "bg-accent text-accent-foreground font-medium"
                )
              }
            >
              {t(item.labelKey)}
            </NavLink>
          ))}
        </nav>
        <div className="border-t p-2">
          <ThemeToggle />
          <Button variant="ghost" className="w-full justify-start" onClick={logout}>
            {t("logout")}
          </Button>
        </div>
      </aside>
      <main className="flex-1 overflow-auto p-6">
        <Outlet />
      </main>
    </div>
  );
}
