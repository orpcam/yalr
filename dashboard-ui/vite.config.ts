import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

const apiTarget = process.env.VITE_API_TARGET ?? "http://localhost:8080";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    outDir: "dist",
  },
  server: {
    // tailscale serve leitet mit dem ts.net-Hostnamen weiter
    allowedHosts: true,
    proxy: {
      "/dashboard-api": apiTarget,
      "/v1": apiTarget,
    },
  },
});
