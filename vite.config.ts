import { resolve } from "node:path";
import { defineConfig } from "vite";

// Two tiny pages, no framework. Tauri serves them from dist/ in release and
// from the dev server in `tauri dev`.
export default defineConfig({
  root: "ui",
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: ["safari16", "chrome110"],
    rollupOptions: {
      input: {
        overlay: resolve(import.meta.dirname, "ui/overlay.html"),
        settings: resolve(import.meta.dirname, "ui/settings.html"),
      },
    },
  },
});
