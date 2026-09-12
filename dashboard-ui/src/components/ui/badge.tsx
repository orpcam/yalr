import * as React from "react";
import { cn } from "@/lib/utils";

type BadgeVariant = "default" | "success" | "error" | "outline";

const badgeVariants: Record<BadgeVariant, string> = {
  default: "bg-primary text-primary-foreground",
  success: "bg-emerald-500/15 text-emerald-500 border border-emerald-500/20",
  error: "bg-red-500/15 text-red-500 border border-red-500/20",
  outline: "border border-border text-foreground",
};

export function Badge({
  className,
  variant = "default",
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & { variant?: BadgeVariant }) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-semibold",
        badgeVariants[variant],
        className
      )}
      {...props}
    />
  );
}
