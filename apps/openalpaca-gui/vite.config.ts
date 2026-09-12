import { fileURLToPath } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react-swc";
import { defineConfig } from "vitest/config";

// https://vite.dev/config/ — tuned for the Tauri shell:
//   * relative `base` so the bundle loads from the `tauri://` asset protocol
//   * fixed dev port (tauri.conf.json `devUrl` points at 1420) and no auto-open
//   * `static/` as the public dir so `favicon.png` keeps its path
export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: "./",
  publicDir: "static",
  // Don't wipe the Rust compiler's output during `tauri dev`.
  clearScreen: false,
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    port: 1420,
    strictPort: true,
    open: false,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // All three Tauri webviews (WKWebView, WebView2, webkit2gtk) handle ES2022.
    target: "es2022",
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    css: true,
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
  },
});
