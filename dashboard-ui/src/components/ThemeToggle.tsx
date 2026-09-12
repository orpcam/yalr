import { useTheme } from "@/lib/theme";
import { Button } from "@/components/ui/button";
import { useLanguage } from "@/lib/i18n";

export function ThemeToggle() {
  const { theme, toggle } = useTheme();
  const { t } = useLanguage();
  return (
    <Button variant="ghost" className="w-full justify-start" onClick={toggle}>
      {theme === "dark" ? t("theme_light") : t("theme_dark")}
    </Button>
  );
}
