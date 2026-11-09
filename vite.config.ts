import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // Boards: drop Excalidraw's Mermaid converter (diagram libraries) from the bundle (src/stubs).
  resolve: { alias: { "@excalidraw/mermaid-to-excalidraw": "/src/stubs/mermaid-to-excalidraw.ts" } },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022", chunkSizeWarningLimit: 400 },
});
