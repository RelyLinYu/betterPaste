import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { fileURLToPath } from "node:url";

const r = (p: string) => fileURLToPath(new URL(p, import.meta.url));

// Two entry points: the hotkey overlay and the settings window.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "chrome120",
    minify: "oxc",
    sourcemap: false,
    emptyOutDir: true,
    rollupOptions: {
      input: {
        overlay: r("./overlay.html"),
        settings: r("./settings.html"),
      },
    },
  },
});
