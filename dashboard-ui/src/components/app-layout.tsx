import { useEffect, useState } from "react";
import { Outlet, NavLink, useNavigate, useLocation } from "react-router-dom";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { ThemeToggle } from "@/components/ThemeToggle";
import { Menu, Network, PanelLeft } from "lucide-react";
import { useLanguage, TranslationKey } from "@/lib/i18n";

const SIDEBAR_STORAGE_KEY = "llm-gw:sidebar";

function initialSidebarHidden(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_STORAGE_KEY) === "hidden";
  } catch {
    return false;
  }
}

const navItems: { to: string; labelKey: TranslationKey }[] = [
  { to: "/", labelKey: "nav_overview" },
  { to: "/requests", labelKey: "nav_requests" },
  { to: "/keys", labelKey: "nav_keys" },
  { to: "/providers", labelKey: "nav_providers" },
  { to: "/settings", labelKey: "nav_settings" },
];

export function AppLayout() {
  const [authed, setAuthed] = useState<boolean | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [sidebarHidden, setSidebarHidden] = useState<boolean>(initialSidebarHidden);
  const navigate = useNavigate();
  const location = useLocation();
  const { t } = useLanguage();

  useEffect(() => {
    api
      .get("/auth/me")
      .then(() => setAuthed(true))
      .catch(() => setAuthed(false));
  }, []);

  // Drawer bei Route-Wechsel schliessen (render-phase, kein Effect)
  const [lastPath, setLastPath] = useState(location.pathname);
  if (lastPath !== location.pathname) {
    setLastPath(location.pathname);
    if (sidebarOpen) setSidebarOpen(false);
  }

  // body-scroll sperren, solange der mobile drawer offen ist;
  // resize von <768px auf >=768px schliesst den drawer, damit der
  // scroll-lock sauber geloescht wird
  useEffect(() => {
    document.body.style.overflow = sidebarOpen ? "hidden" : "";
    const onResize = () => {
      if (window.innerWidth >= 768 && sidebarOpen) setSidebarOpen(false);
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      document.body.style.overflow = "";
    };
  }, [sidebarOpen]);

  // Desktop-Ausblendung persistieren
  useEffect(() => {
    try {
      localStorage.setItem(SIDEBAR_STORAGE_KEY, sidebarHidden ? "hidden" : "visible");
    } catch {
      // localStorage nicht verfügbar -> ignorieren
    }
  }, [sidebarHidden]);

  // escape schliesst den offenen mobile drawer
  useEffect(() => {
    if (!sidebarOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSidebarOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sidebarOpen]);

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
      {sidebarOpen && (
        <div
          className="fixed inset-0 z-40 bg-black/50 md:hidden"
          onClick={() => setSidebarOpen(false)}
          aria-hidden
        />
      )}
      <aside
        {...(sidebarOpen ? { role: "dialog", "aria-modal": "true" as const } : {})}
        className={cn(
          "flex flex-col border-r bg-card",
          // mobil: off-canvas drawer; ab md: statische sidebar wie bisher
          // (solange sie nicht ausgeblendet ist)
          "fixed inset-y-0 left-0 z-50 w-64 transition-transform duration-200",
          !sidebarHidden && "md:static md:w-56 md:translate-x-0",
          sidebarOpen ? "translate-x-0" : "-translate-x-full"
        )}
      >
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
              onClick={() => setSidebarOpen(false)}
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
      <main className="flex-1 overflow-auto p-4 sm:p-6">
        <div className="mb-4 flex items-center gap-2">
          {/* Hamburger: mobil immer sichtbar; Desktop nur, wenn Sidebar ausgeblendet */}
          <button
            type="button"
            aria-label={t("show_sidebar")}
            title={t("show_sidebar")}
            className={cn(
              "inline-flex h-11 w-11 items-center justify-center rounded-md text-foreground hover:bg-accent hover:text-accent-foreground",
              sidebarHidden ? "md:inline-flex" : "md:hidden"
            )}
            onClick={() => {
              const isDesktop = window.matchMedia("(min-width: 768px)").matches;
              if (isDesktop) setSidebarHidden(false);
              else setSidebarOpen(true);
            }}
          >
            <Menu className="h-5 w-5" />
          </button>
          {/* Desktop: Sidebar ausblenden, nur wenn sie sichtbar ist */}
          {!sidebarHidden && (
            <button
              type="button"
              aria-label={t("hide_sidebar")}
              title={t("hide_sidebar")}
              className="hidden h-11 w-11 items-center justify-center rounded-md text-foreground hover:bg-accent hover:text-accent-foreground md:inline-flex"
              onClick={() => setSidebarHidden(true)}
            >
              <PanelLeft className="h-5 w-5" />
            </button>
          )}
          <span className="flex items-center gap-2 md:hidden">
            <Network className="h-6 w-6 shrink-0 text-emerald-500" />
            <span className="text-lg font-bold">YALR</span>
          </span>
        </div>
        <Outlet />
      </main>
    </div>
  );
}
