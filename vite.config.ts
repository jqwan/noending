import { configDefaults, defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Rust sources and build outputs are watched by Tauri, not Vite.
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2021",
    minify: "esbuild",
    sourcemap: false,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("node_modules")) {
            if (id.includes("@xterm")) {
              return "vendor-xterm";
            }
            if (
              id.includes("react-markdown") ||
              id.includes("remark-gfm") ||
              id.includes("micromark") ||
              id.includes("mdast") ||
              id.includes("unist") ||
              id.includes("vfile") ||
              id.includes("bail") ||
              id.includes("trough") ||
              id.includes("is-plain-obj") ||
              id.includes("unified") ||
              id.includes("decode-named-character-reference") ||
              id.includes("character-entities") ||
              id.includes("property-information") ||
              id.includes("html-void-elements") ||
              id.includes("ccount") ||
              id.includes("markdown")
            ) {
              return "vendor-markdown";
            }
            if (
              id.includes("/react/") ||
              id.includes("/react-dom/") ||
              id.includes("/scheduler/") ||
              id.includes("react-is")
            ) {
              return "vendor-react";
            }
          }

        },
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    // Local probes and downloaded fixtures are not application test suites.
    exclude: [...configDefaults.exclude, "**/.tmp/**"],
  },
});
