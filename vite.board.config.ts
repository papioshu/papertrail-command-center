import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Boards on iPhone/iPad (D050): BoardCanvas built as a page the hub serves at /board/, talking to the hub over HTTP.
export default defineConfig({
  plugins: [react()],
  base: "/board/",
  resolve: {
    alias: [
      { find: "@excalidraw/mermaid-to-excalidraw", replacement: "/src/stubs/mermaid-to-excalidraw.ts" },
      { find: /^\.\.\/api$/, replacement: "/src/web/hubApi.ts" },
    ],
  },
  build: { target: "es2020", outDir: "dist-board", emptyOutDir: true, rollupOptions: { input: "board.html" }, chunkSizeWarningLimit: 4000 },
});
